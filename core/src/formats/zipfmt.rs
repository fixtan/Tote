//! ZIP。一覧は中央ディレクトリだけを読み、展開は必要な項目へ直接シークする。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use zip::ZipArchive;

use super::{Backend, Listing, Sink};
use crate::view::{Entry, decode_name, local_stamp, set_stamp};
use crate::{Error, io_err};

pub(crate) struct ZipBackend;

fn open(path: &Path) -> Result<ZipArchive<BufReader<File>>, Error> {
    let file = File::open(path).map_err(io_err(path))?;
    Ok(ZipArchive::new(BufReader::new(file))?)
}

impl Backend for ZipBackend {
    fn list(&self, path: &Path, _pw: Option<&str>) -> Result<Listing, Error> {
        let mut ar = open(path)?;
        let mut entries = Vec::with_capacity(ar.len());
        for i in 0..ar.len() {
            let f = ar.by_index_raw(i)?;
            let decoded = decode_name(f.name_raw(), f.name());
            let mut e = Entry::new(i, &decoded, f.is_dir());
            e.symlink = f.is_symlink();
            e.size = f.size();
            e.packed = f.compressed_size();
            e.crc32 = f.crc32();
            e.method = f.compression().to_string();
            e.encrypted = f.encrypted();
            if let Some(d) = f.last_modified() {
                set_stamp(
                    &mut e,
                    local_stamp(i32::from(d.year()), d.month(), d.day(), d.hour(), d.minute(), d.second()),
                );
            }
            entries.push(e);
        }
        let comment = decode_name(ar.comment(), &String::from_utf8_lossy(ar.comment()));
        Ok(Listing { entries, comment, format: "ZIP" })
    }

    fn walk(&self, path: &Path, entries: &[Entry], pw: Option<&str>, sink: &mut dyn Sink) -> Result<(), Error> {
        let mut ar = open(path)?;
        for e in entries {
            if !sink.wants(e.index) {
                continue;
            }
            let opened = match (e.encrypted, pw) {
                (true, Some(pw)) => ar.by_index_decrypt(e.index, pw.as_bytes()),
                _ => ar.by_index(e.index),
            };
            match opened {
                Ok(mut f) => sink.file(e.index, &mut f),
                Err(err) => sink.fail(e.index, format!("読み出せません: {err}")),
            }
            if sink.remaining() == 0 {
                break;
            }
        }
        Ok(())
    }

    fn check_password(&self, path: &Path, _entries: &[Entry], wanted: &[usize], pw: &str) -> Result<(), Error> {
        let mut ar = open(path)?;
        // 暗号化された項目ごとに鍵の検査値があるので、1つ開けば合否が分かる（全部同じパスワードで作られる前提）
        let Some(&i) = wanted.first() else { return Ok(()) };
        match ar.by_index_decrypt(i, pw.as_bytes()) {
            Ok(_) => Ok(()),
            Err(zip::result::ZipError::InvalidPassword) => Err(Error::WrongPassword),
            Err(e) => Err(e.into()),
        }
    }
}
