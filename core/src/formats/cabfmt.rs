//! CAB（Windowsキャビネット）。名前で直接読み出せる。
//!
//! 制限: 日本語ファイル名が Shift_JIS のCABは、ライブラリ側でUTF-8として読まれるため文字化けする。
//! 複数ファイルにまたがるキャビネット（分割CAB）は未対応。

use std::fs::File;
use std::path::Path;

use super::{Backend, Listing, Sink};
use crate::view::{Entry, local_stamp, set_stamp};
use crate::{Error, io_err};

pub(crate) struct CabBackend;

fn open(path: &Path) -> Result<cab::Cabinet<File>, Error> {
    let file = File::open(path).map_err(io_err(path))?;
    cab::Cabinet::new(file).map_err(|e| Error::Archive(e.to_string()))
}

impl Backend for CabBackend {
    fn list(&self, path: &Path) -> Result<Listing, Error> {
        let cabinet = open(path)?;
        let mut entries = Vec::new();
        for folder in cabinet.folder_entries() {
            let method = format!("{:?}", folder.compression_type());
            for f in folder.file_entries() {
                let mut e = Entry::new(entries.len(), f.name(), false);
                e.size = u64::from(f.uncompressed_size());
                e.method = method.clone();
                if let Some(dt) = f.datetime() {
                    set_stamp(
                        &mut e,
                        local_stamp(dt.year(), u8::from(dt.month()), dt.day(), dt.hour(), dt.minute(), dt.second()),
                    );
                }
                entries.push(e);
            }
        }
        Ok(Listing { entries, comment: String::new(), format: "CAB" })
    }

    fn walk(&self, path: &Path, entries: &[Entry], sink: &mut dyn Sink) -> Result<(), Error> {
        let mut cabinet = open(path)?;
        // 名前は書庫内の元の表記（`\` 区切り）で読み出す
        let names: Vec<String> =
            cabinet.folder_entries().flat_map(|f| f.file_entries().map(|e| e.name().to_string()).collect::<Vec<_>>()).collect();
        for e in entries {
            if !sink.wants(e.index) {
                continue;
            }
            let Some(name) = names.get(e.index) else {
                sink.fail(e.index, "書庫から見つかりませんでした".to_string());
                continue;
            };
            match cabinet.read_file(name) {
                Ok(mut r) => sink.file(e.index, &mut r),
                Err(err) => sink.fail(e.index, format!("読み出せません: {err}")),
            }
            if sink.remaining() == 0 {
                break;
            }
        }
        Ok(())
    }
}
