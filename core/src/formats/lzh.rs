//! LZH / LHA。日本語ファイル名（Shift_JIS）は、ライブラリの文字列化を使わず生のバイト列から自前で復元する。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use delharc::LhaDecodeReader;

use super::{Backend, Listing, Sink};
use crate::view::{Entry, decode_name, local_stamp, set_stamp, unix_stamp};
use crate::{Error, io_err};

pub(crate) struct LzhBackend;

const EXT_FILENAME: u8 = 0x01;
const EXT_PATH: u8 = 0x02;

fn open(path: &Path) -> Result<LhaDecodeReader<BufReader<File>>, Error> {
    let file = File::open(path).map_err(io_err(path))?;
    LhaDecodeReader::new(BufReader::new(file)).map_err(|e| Error::Archive(e.to_string()))
}

/// 生のバイト列（区切りは 0xFF か `/`）を文字列にする。`\` は後で `/` に直す。
fn decode_path_bytes(data: &[u8]) -> String {
    let parts: Vec<String> = data
        .split(|&c| c == 0xFF || c == b'/')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let p = match p.iter().position(|&c| c == 0) {
                Some(i) => &p[..i],
                None => p,
            };
            decode_name(p, &String::from_utf8_lossy(p))
        })
        .collect();
    parts.join("/")
}

/// ヘッダーから「ディレクトリ + ファイル名」を取り出す。
fn header_path(h: &delharc::LhaHeader) -> String {
    let mut dir: Option<&[u8]> = None;
    let mut name: Option<&[u8]> = None;
    for ext in h.iter_extra() {
        match ext {
            [EXT_FILENAME, data @ ..] => name = Some(data),
            [EXT_PATH, data @ ..] => dir = Some(data),
            _ => {}
        }
    }
    let mut out = String::new();
    if let Some(d) = dir {
        out.push_str(&decode_path_bytes(d));
    }
    let name_bytes: &[u8] = match name {
        Some(n) => n,
        None => &h.filename,
    };
    let n = decode_path_bytes(name_bytes);
    if !n.is_empty() {
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&n);
    }
    // ディレクトリだけのヘッダーは末尾が区切りになる
    if name.is_none() && h.filename.is_empty() && !out.is_empty() {
        out.push('/');
    }
    out
}

fn make_entry(index: usize, h: &delharc::LhaHeader) -> Entry {
    let raw = header_path(h);
    let mut e = Entry::new(index, &raw, h.is_directory());
    e.size = h.original_size;
    e.packed = h.compressed_size;
    e.method = String::from_utf8_lossy(&h.compression).into_owned();
    let ts = h.parse_last_modified();
    if let Some(n) = ts.to_naive_utc() {
        if ts.is_utc() {
            set_stamp(&mut e, unix_stamp(n.and_utc().timestamp()));
        } else {
            // MS-DOS形式（時差なし）は、ローカル時刻としてそのまま扱う
            let s = n.to_string(); // "YYYY-MM-DD HH:MM:SS"
            if let Some(v) = parse_naive(&s) {
                set_stamp(&mut e, local_stamp(v.0, v.1, v.2, v.3, v.4, v.5));
            }
        }
    }
    e
}

fn parse_naive(s: &str) -> Option<(i32, u8, u8, u8, u8, u8)> {
    let (d, t) = s.split_once(' ')?;
    let mut dp = d.split('-');
    let mut tp = t.split(':');
    Some((
        dp.next()?.parse().ok()?,
        dp.next()?.parse().ok()?,
        dp.next()?.parse().ok()?,
        tp.next()?.parse().ok()?,
        tp.next()?.parse().ok()?,
        tp.next()?.split('.').next()?.parse().ok()?,
    ))
}

impl Backend for LzhBackend {
    fn list(&self, path: &Path) -> Result<Listing, Error> {
        let mut reader = open(path)?;
        let mut entries = Vec::new();
        loop {
            let e = make_entry(entries.len(), reader.header());
            entries.push(e);
            if !reader.next_file().map_err(|e| Error::Archive(e.to_string()))? {
                break;
            }
        }
        Ok(Listing { entries, comment: String::new(), format: "LZH" })
    }

    fn walk(&self, path: &Path, _entries: &[Entry], sink: &mut dyn Sink) -> Result<(), Error> {
        let mut reader = open(path)?;
        let mut i = 0;
        loop {
            if sink.wants(i) {
                if reader.is_decoder_supported() {
                    sink.file(i, &mut reader);
                } else {
                    sink.fail(i, "未対応の圧縮方式です".to_string());
                }
                if sink.remaining() == 0 {
                    break;
                }
            }
            let more = reader.next_file().map_err(|e| Error::Archive(e.to_string()))?;
            if !more {
                break;
            }
            i += 1;
        }
        Ok(())
    }
}
