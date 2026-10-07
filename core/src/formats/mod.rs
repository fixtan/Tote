//! 書庫形式ごとの読み出し。
//!
//! 新しい形式を足す手順（例: ISO）:
//!   1. このフォルダに `xxx.rs` を作り、`Backend`（`list` と `walk`）を実装する。
//!   2. `Kind` に項目を足し、`detect`（先頭バイト）・`from_extension`・`backend` に1行ずつ足す。
//!   3. 画面側（一覧・展開・ドラッグ）は `view::list` / `view::extract` を通るので変更不要。
//!
//! `walk` は、`sink.wants(index)` が true の項目だけを `sink.file(index, reader)` に渡す。
//! `index` は `list` が返した `Entry::index` と同じ順序・同じ番号にすること。
//! 固体圧縮の書庫は、前から順に読むしかないので、`sink.remaining() == 0` になった時点で打ち切る。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::view::Entry;
use crate::{Error, io_err};

mod cabfmt;
mod iso;
mod lzh;
mod rar;
mod sevenz;
mod tarfmt;
mod zipfmt;

/// 展開の書き出し側（`view::extract` が実装する）
pub trait Sink {
    /// この項目を読み出したいか
    fn wants(&self, index: usize) -> bool;
    /// まだ読み出していない対象の数。0 になったら `walk` は打ち切ってよい
    fn remaining(&self) -> usize;
    /// 項目の中身を渡す（読み切らなくてもよい）
    fn file(&mut self, index: usize, data: &mut dyn Read);
    /// 項目を読み出せなかった
    fn fail(&mut self, index: usize, reason: String);
}

pub(crate) struct Listing {
    pub entries: Vec<Entry>,
    pub comment: String,
    /// 画面に出す形式名（"ZIP", "tar.gz" など）
    pub format: &'static str,
}

pub(crate) trait Backend {
    fn list(&self, path: &Path) -> Result<Listing, Error>;
    fn walk(&self, path: &Path, entries: &[Entry], sink: &mut dyn Sink) -> Result<(), Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    SevenZ,
    Rar,
    /// gzip（中身が tar なら tar.gz、そうでなければ単体の .gz）
    Gzip,
    Tar,
    Cab,
    Lzh,
    /// ISO 9660 / Joliet / Rock Ridge（UDF は未対応）
    Iso,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Zip => "ZIP",
            Kind::SevenZ => "7z",
            Kind::Rar => "RAR",
            Kind::Gzip => "gzip",
            Kind::Tar => "tar",
            Kind::Cab => "CAB",
            Kind::Lzh => "LZH",
            Kind::Iso => "ISO",
        }
    }

    /// 小文字の拡張子（ドットなし）から。
    pub fn from_extension(ext: &str) -> Option<Kind> {
        Some(match ext {
            "zip" => Kind::Zip,
            "7z" => Kind::SevenZ,
            "rar" => Kind::Rar,
            "gz" | "tgz" => Kind::Gzip,
            "tar" => Kind::Tar,
            "cab" => Kind::Cab,
            "lzh" | "lha" => Kind::Lzh,
            "iso" => Kind::Iso,
            _ => return None,
        })
    }
}

/// 形式を判定する。先頭バイトを優先し、分からなければ拡張子で決める。
pub fn detect(path: &Path) -> Result<Kind, Error> {
    let mut f = File::open(path).map_err(io_err(path))?;
    let len = f.metadata().map_err(io_err(path))?.len();
    let mut head = vec![0u8; 512];
    let n = read_up_to(&mut f, &mut head).map_err(io_err(path))?;
    head.truncate(n);

    let by_magic = if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") || head.starts_with(b"PK\x07\x08") {
        Some(Kind::Zip)
    } else if head.starts_with(&[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C]) {
        Some(Kind::SevenZ)
    } else if head.starts_with(b"Rar!\x1a\x07") {
        Some(Kind::Rar)
    } else if head.starts_with(&[0x1f, 0x8b]) {
        Some(Kind::Gzip)
    } else if head.starts_with(b"MSCF") {
        Some(Kind::Cab)
    } else if head.len() > 7 && &head[2..5] == b"-l" && matches!(head[5], b'h' | b'z') && head[7] == b'-' {
        Some(Kind::Lzh)
    } else if is_tar_header(&head) {
        Some(Kind::Tar)
    } else if len > 0x8006 && has_iso_magic(&mut f) {
        Some(Kind::Iso)
    } else {
        None
    };
    if let Some(k) = by_magic {
        return Ok(k);
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    Kind::from_extension(&ext)
        .ok_or_else(|| Error::Unsupported("対応していない形式、または壊れた書庫です".to_string()))
}

/// 先頭512バイトが tar のヘッダーか。チェックサム（148..156バイト目、8進数）で判定するので、
/// 古い形式（"ustar" の印が無い v7 tar）も拾える。
pub(crate) fn is_tar_header(head: &[u8]) -> bool {
    if head.len() < 512 || head[..512].iter().all(|&b| b == 0) {
        return false;
    }
    let field: String = head[148..156].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect();
    let Ok(want) = u32::from_str_radix(field.trim(), 8) else { return false };
    let sum: u32 = head[..512].iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { u32::from(b) }).sum();
    sum == want
}

fn has_iso_magic(f: &mut File) -> bool {
    let mut m = [0u8; 5];
    f.seek(SeekFrom::Start(0x8001)).is_ok() && f.read_exact(&mut m).is_ok() && &m == b"CD001"
}

/// `buf` が埋まるか EOF まで読む。
pub(crate) fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

pub(crate) fn backend(kind: Kind) -> &'static dyn Backend {
    match kind {
        Kind::Zip => &zipfmt::ZipBackend,
        Kind::SevenZ => &sevenz::SevenZBackend,
        Kind::Rar => &rar::RarBackend,
        Kind::Gzip => &tarfmt::TarBackend { gzip: true },
        Kind::Tar => &tarfmt::TarBackend { gzip: false },
        Kind::Cab => &cabfmt::CabBackend,
        Kind::Lzh => &lzh::LzhBackend,
        Kind::Iso => &iso::IsoBackend,
    }
}
