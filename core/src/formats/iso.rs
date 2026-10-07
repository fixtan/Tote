//! ISO（ISO 9660 / Joliet / Rock Ridge）。普通のデータCD・DVDのイメージを読む。
//!
//! 名前は Joliet（UCS-2。日本語名はこちら）があればそれを使い、無ければ Rock Ridge の長い名前、
//! それも無ければ素の ISO 9660 名（8.3 形式。`;1` の版番号は落とす）を使う。
//! 制限: UDF（大きめのDVD・Blu-ray のイメージ）、マルチセッション、raw セクタ形式（2352バイト/セクタ）は未対応。

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use super::{Backend, Listing, Sink};
use crate::view::{Entry, unix_stamp};
use crate::{Error, io_err};

pub(crate) struct IsoBackend;

/// 1つの項目の中身がある場所（連続した領域の並び。4GiB超のファイルは複数に分かれる）
type Extents = Vec<(u64, u64)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Names {
    Joliet,
    Primary,
}

struct Volume {
    names: Names,
    root_lba: u64,
    root_len: u64,
    block: u64,
    label: String,
}

fn bad(msg: &str) -> Error {
    Error::Archive(format!("ISOを読めません: {msg}"))
}

fn u32le(b: &[u8], at: usize) -> u64 {
    u64::from(u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]))
}

fn u16le(b: &[u8], at: usize) -> u64 {
    u64::from(u16::from_le_bytes([b[at], b[at + 1]]))
}

/// ボリューム記述子（セクタ16〜）を読んで、使う名前の種類とルートの位置を決める。
fn read_volume(f: &mut File, file_len: u64) -> Result<Volume, Error> {
    let mut primary: Option<Volume> = None;
    let mut joliet: Option<Volume> = None;
    let mut sector = [0u8; 2048];
    for n in 0..64u64 {
        let at = (16 + n) * 2048;
        if at + 2048 > file_len {
            break;
        }
        f.seek(SeekFrom::Start(at)).map_err(|e| bad(&e.to_string()))?;
        f.read_exact(&mut sector).map_err(|e| bad(&e.to_string()))?;
        if &sector[1..6] != b"CD001" {
            break;
        }
        match sector[0] {
            255 => break,
            1 | 2 => {
                let is_joliet = sector[0] == 2 && matches!(&sector[88..91], b"%/@" | b"%/C" | b"%/E");
                let block = u16le(&sector, 128);
                let root = &sector[156..190];
                let vol = Volume {
                    names: if is_joliet { Names::Joliet } else { Names::Primary },
                    root_lba: u32le(root, 2),
                    root_len: u32le(root, 10),
                    block: if block >= 512 { block } else { 2048 },
                    label: label_of(&sector[40..72], is_joliet),
                };
                if is_joliet {
                    joliet.get_or_insert(vol);
                } else if sector[0] == 1 {
                    primary.get_or_insert(vol);
                }
            }
            _ => {}
        }
    }
    match (joliet, primary) {
        (Some(mut j), Some(p)) => {
            // Joliet の記述子が空のラベルなら、素の記述子のほうを使う
            if j.label.is_empty() {
                j.label = p.label;
            }
            Ok(j)
        }
        (Some(j), None) => Ok(j),
        (None, Some(p)) => Ok(p),
        (None, None) => Err(bad("ISO 9660 のボリューム記述子が見つかりません（UDF のみのイメージは未対応です）")),
    }
}

fn label_of(raw: &[u8], joliet: bool) -> String {
    if joliet {
        ucs2(raw).trim().to_string()
    } else {
        String::from_utf8_lossy(raw).trim().to_string()
    }
}

fn ucs2(raw: &[u8]) -> String {
    let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

/// `FOO.TXT;1` → `FOO.TXT`、`README.;1` → `README`
fn strip_version(name: &str) -> String {
    let n = match name.rfind(';') {
        Some(i) if name[i + 1..].chars().all(|c| c.is_ascii_digit()) => &name[..i],
        _ => name,
    };
    n.strip_suffix('.').filter(|s| !s.is_empty()).unwrap_or(n).to_string()
}

struct Record {
    name: String,
    is_dir: bool,
    symlink: bool,
    lba: u64,
    len: u64,
    /// 4GiB超のファイルの途中の断片（まだ続きがある）
    more: bool,
    stamp: Option<i64>,
}

/// Rock Ridge の項目から、長い名前と「シンボリックリンクか」を取り出す。
fn rock_ridge(mut su: &[u8]) -> (Option<String>, bool) {
    let mut name: Option<Vec<u8>> = None;
    let mut link = false;
    while su.len() >= 4 {
        let (sig, len) = (&su[0..2], su[2] as usize);
        if len < 4 || len > su.len() {
            break;
        }
        match sig {
            b"NM" if len >= 5 => {
                let flags = su[4];
                // bit1 = "."、bit2 = ".." を指す名前（使わない）
                if flags & 0b110 == 0 {
                    name.get_or_insert_with(Vec::new).extend_from_slice(&su[5..len]);
                }
            }
            b"SL" => link = true,
            b"PX" if len >= 12 => {
                let mode = u32le(su, 4);
                if mode & 0o170000 == 0o120000 {
                    link = true;
                }
            }
            _ => {}
        }
        su = &su[len..];
    }
    (name.map(|n| String::from_utf8_lossy(&n).into_owned()), link)
}

fn parse_record(rec: &[u8], names: Names) -> Option<Record> {
    if rec.len() < 34 {
        return None;
    }
    let len_fi = rec[32] as usize;
    if 33 + len_fi > rec.len() {
        return None;
    }
    let id = &rec[33..33 + len_fi];
    if id == [0] || id == [1] {
        return None; // "." と ".."
    }
    let flags = rec[25];
    let (mut name, mut symlink) = match names {
        Names::Joliet => (strip_version(&ucs2(id)), false),
        Names::Primary => (strip_version(&String::from_utf8_lossy(id)), false),
    };
    if names == Names::Primary {
        let pad = if len_fi % 2 == 0 { 1 } else { 0 };
        let start = 33 + len_fi + pad;
        if start < rec.len() {
            let (nm, link) = rock_ridge(&rec[start..]);
            if let Some(n) = nm.filter(|n| !n.is_empty()) {
                name = n;
            }
            symlink = link;
        }
    }
    if name.is_empty() {
        return None;
    }
    let stamp = (|| {
        let d = &rec[18..25];
        let date = time::Date::from_calendar_date(1900 + i32::from(d[0]), time::Month::try_from(d[1]).ok()?, d[2]).ok()?;
        let t = time::Time::from_hms(d[3], d[4], d[5]).ok()?;
        let off = time::UtcOffset::from_whole_seconds(i32::from(d[6] as i8) * 900).ok()?;
        Some(time::PrimitiveDateTime::new(date, t).assume_offset(off).unix_timestamp())
    })();
    Some(Record { name, is_dir: flags & 0b10 != 0, symlink, lba: u32le(rec, 2), len: u32le(rec, 10), more: flags & 0x80 != 0, stamp })
}

struct Scan {
    entries: Vec<Entry>,
    extents: Vec<Extents>,
    label: String,
}

struct Walker<'a> {
    f: &'a mut File,
    vol: &'a Volume,
    file_len: u64,
    seen: HashSet<u64>,
    out: Scan,
}

impl Walker<'_> {
    fn read_dir(&mut self, lba: u64, len: u64) -> Result<Vec<u8>, Error> {
        let at = lba.checked_mul(self.vol.block).ok_or_else(|| bad("位置が不正です"))?;
        if at.checked_add(len).is_none_or(|end| end > self.file_len) {
            return Err(bad("フォルダの位置がファイルの外を指しています（壊れているか、未対応のイメージです）"));
        }
        let mut buf = vec![0u8; len as usize];
        self.f.seek(SeekFrom::Start(at)).map_err(|e| bad(&e.to_string()))?;
        self.f.read_exact(&mut buf).map_err(|e| bad(&e.to_string()))?;
        Ok(buf)
    }

    fn walk_dir(&mut self, lba: u64, len: u64, prefix: &str, depth: usize) -> Result<(), Error> {
        if depth > 64 || !self.seen.insert(lba) {
            return Ok(()); // 循環や極端な深さは打ち切る
        }
        let data = self.read_dir(lba, len)?;
        let block = self.vol.block as usize;
        let mut pos = 0usize;
        let mut pending: Option<(Record, Extents)> = None;
        let mut kids: Vec<(Record, Extents)> = Vec::new();
        while pos < data.len() {
            let l = data[pos] as usize;
            if l == 0 {
                // レコードはセクタをまたがない。残りは詰め物なので次のセクタへ
                pos = (pos / block + 1) * block;
                continue;
            }
            if pos + l > data.len() {
                break;
            }
            if let Some(r) = parse_record(&data[pos..pos + l], self.vol.names) {
                match pending.take() {
                    Some((first, mut ex)) if first.name == r.name && !r.is_dir => {
                        ex.push((r.lba, r.len));
                        if r.more {
                            pending = Some((first, ex));
                        } else {
                            kids.push((first, ex));
                        }
                    }
                    other => {
                        if let Some(p) = other {
                            kids.push(p);
                        }
                        let ex = vec![(r.lba, r.len)];
                        if r.more && !r.is_dir {
                            pending = Some((r, ex));
                        } else {
                            kids.push((r, ex));
                        }
                    }
                }
            }
            pos += l;
        }
        if let Some(p) = pending {
            kids.push(p);
        }
        kids.sort_by(|a, b| a.0.name.cmp(&b.0.name));

        for (r, ex) in kids {
            let path = if prefix.is_empty() { r.name.clone() } else { format!("{prefix}/{}", r.name) };
            let mut e = Entry::new(self.out.entries.len(), &path, r.is_dir);
            e.symlink = r.symlink;
            if let Some(s) = r.stamp {
                crate::view::set_stamp(&mut e, unix_stamp(s));
            }
            if !r.is_dir {
                e.size = ex.iter().map(|x| x.1).sum();
                e.packed = e.size;
                e.method = "Stored".to_string();
            }
            self.out.entries.push(e);
            self.out.extents.push(if r.is_dir { Vec::new() } else { ex });
            if r.is_dir && !r.symlink {
                self.walk_dir(r.lba, r.len, &path, depth + 1)?;
            }
        }
        Ok(())
    }
}

fn scan(path: &Path) -> Result<Scan, Error> {
    let mut f = File::open(path).map_err(io_err(path))?;
    let file_len = f.metadata().map_err(io_err(path))?.len();
    let vol = read_volume(&mut f, file_len)?;
    let mut w = Walker { f: &mut f, vol: &vol, file_len, seen: HashSet::new(), out: Scan { entries: Vec::new(), extents: Vec::new(), label: vol.label.clone() } };
    w.walk_dir(vol.root_lba, vol.root_len, "", 0)?;
    Ok(w.out)
}

/// 複数の領域をつなげて1本の読み取りとして見せる
struct ExtentReader<'a> {
    f: &'a mut File,
    extents: &'a [(u64, u64)],
    block: u64,
    cur: usize,
    left: u64,
    started: bool,
}

impl Read for ExtentReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if !self.started || self.left == 0 {
                if self.started {
                    self.cur += 1;
                }
                let Some(&(lba, len)) = self.extents.get(self.cur) else { return Ok(0) };
                self.f.seek(SeekFrom::Start(lba * self.block))?;
                self.left = len;
                self.started = true;
                if len == 0 {
                    continue;
                }
            }
            let want = buf.len().min(self.left.min(usize::MAX as u64) as usize);
            let n = self.f.read(&mut buf[..want])?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "イメージが途中で切れています"));
            }
            self.left -= n as u64;
            return Ok(n);
        }
    }
}

impl Backend for IsoBackend {
    fn list(&self, path: &Path) -> Result<Listing, Error> {
        let s = scan(path)?;
        let comment = if s.label.is_empty() { String::new() } else { format!("ボリューム名: {}", s.label) };
        Ok(Listing { entries: s.entries, comment, format: "ISO" })
    }

    fn walk(&self, path: &Path, entries: &[Entry], sink: &mut dyn Sink) -> Result<(), Error> {
        let s = scan(path)?;
        let mut f = File::open(path).map_err(io_err(path))?;
        let file_len = f.metadata().map_err(io_err(path))?.len();
        let block = read_volume(&mut f, file_len)?.block;
        for e in entries {
            if !sink.wants(e.index) {
                continue;
            }
            let Some(ex) = s.extents.get(e.index) else {
                sink.fail(e.index, "書庫から見つかりませんでした".to_string());
                continue;
            };
            if ex.iter().any(|&(lba, len)| lba.checked_mul(block).and_then(|a| a.checked_add(len)).is_none_or(|end| end > file_len)) {
                sink.fail(e.index, "データがイメージの外を指しています（壊れています）".to_string());
                continue;
            }
            let mut r = ExtentReader { f: &mut f, extents: ex, block, cur: 0, left: 0, started: false };
            sink.file(e.index, &mut r);
            if sink.remaining() == 0 {
                break;
            }
        }
        Ok(())
    }
}
