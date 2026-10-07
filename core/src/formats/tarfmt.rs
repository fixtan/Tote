//! tar / tar.gz / tgz / 単体の .gz。
//!
//! tar は前から順に読むしかないので、一覧にも全体の走査が必要（gz の場合は伸長しながら）。

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::MultiGzDecoder;

use super::{Backend, Listing, Sink, is_tar_header, read_up_to};
use crate::view::{Entry, decode_name, set_stamp, unix_stamp};
use crate::{Error, io_err};

pub(crate) struct TarBackend {
    pub gzip: bool,
}

enum Content {
    Tar(Box<dyn Read>),
    /// 単体の .gz（名前・更新日時・展開後サイズ）
    Single { reader: Box<dyn Read>, name: String, mtime: i64, size: u64 },
}

fn gz_isize(path: &Path) -> u64 {
    // gzipの末尾4バイトは展開後サイズ（4GiBで折り返す）
    let Ok(mut f) = File::open(path) else { return 0 };
    let mut b = [0u8; 4];
    if f.seek(SeekFrom::End(-4)).is_ok() && f.read_exact(&mut b).is_ok() { u64::from(u32::from_le_bytes(b)) } else { 0 }
}

fn open_content(path: &Path, gzip: bool) -> Result<Content, Error> {
    let file = File::open(path).map_err(io_err(path))?;
    if !gzip {
        return Ok(Content::Tar(Box::new(BufReader::new(file))));
    }
    let mut dec = MultiGzDecoder::new(BufReader::new(file));
    let mut head = vec![0u8; 512];
    let n = read_up_to(&mut dec, &mut head).map_err(|e| Error::Archive(format!("gzipを読めません: {e}")))?;
    head.truncate(n);
    let (fname, mtime) = match dec.header() {
        Some(h) => (h.filename().map(|b| decode_name(b, &String::from_utf8_lossy(b))), i64::from(h.mtime())),
        None => (None, 0),
    };
    let is_tar = is_tar_header(&head);
    let rest: Box<dyn Read> = Box::new(Cursor::new(head).chain(dec));
    if is_tar {
        return Ok(Content::Tar(rest));
    }
    let name = fname.filter(|n| !n.is_empty()).unwrap_or_else(|| {
        let stem = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let lower = stem.to_ascii_lowercase();
        if lower.ends_with(".gz") { stem[..stem.len() - 3].to_string() } else { format!("{stem}.out") }
    });
    Ok(Content::Single { reader: rest, name, mtime, size: gz_isize(path) })
}

fn tar_entry(index: usize, e: &tar::Entry<'_, impl Read>) -> Entry {
    let raw = e.path_bytes();
    let name = decode_name(&raw, &String::from_utf8_lossy(&raw));
    let ty = e.header().entry_type();
    let mut out = Entry::new(index, &name, ty.is_dir());
    out.size = e.size();
    out.symlink = !(ty.is_dir() || ty.is_file() || ty.is_contiguous() || ty.is_gnu_sparse());
    out.method = "tar".to_string();
    if let Ok(m) = e.header().mtime() {
        set_stamp(&mut out, unix_stamp(m as i64));
    }
    out
}

impl Backend for TarBackend {
    fn list(&self, path: &Path, _pw: Option<&str>) -> Result<Listing, Error> {
        match open_content(path, self.gzip)? {
            Content::Tar(r) => {
                let mut ar = tar::Archive::new(r);
                let mut entries = Vec::new();
                let it = ar.entries().map_err(|e| Error::Archive(e.to_string()))?;
                for (i, e) in it.enumerate() {
                    let e = e.map_err(|e| Error::Archive(e.to_string()))?;
                    entries.push(tar_entry(i, &e));
                }
                Ok(Listing { entries, comment: String::new(), format: if self.gzip { "tar.gz" } else { "tar" } })
            }
            Content::Single { name, mtime, size, .. } => {
                let mut e = Entry::new(0, &name, false);
                e.size = size;
                e.method = "gzip".to_string();
                if mtime > 0 {
                    set_stamp(&mut e, unix_stamp(mtime));
                }
                Ok(Listing { entries: vec![e], comment: String::new(), format: "gz" })
            }
        }
    }

    fn walk(&self, path: &Path, _entries: &[Entry], _pw: Option<&str>, sink: &mut dyn Sink) -> Result<(), Error> {
        match open_content(path, self.gzip)? {
            Content::Tar(r) => {
                let mut ar = tar::Archive::new(r);
                let it = ar.entries().map_err(|e| Error::Archive(e.to_string()))?;
                for (i, e) in it.enumerate() {
                    let mut e = e.map_err(|e| Error::Archive(e.to_string()))?;
                    if sink.wants(i) {
                        sink.file(i, &mut e);
                        if sink.remaining() == 0 {
                            break;
                        }
                    }
                }
                Ok(())
            }
            Content::Single { mut reader, .. } => {
                if sink.wants(0) {
                    sink.file(0, &mut reader);
                }
                Ok(())
            }
        }
    }
}

