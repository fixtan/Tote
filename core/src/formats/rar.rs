//! RAR（展開専用）。unrar のC++ライブラリ（unrar_sys）を使う。
//!
//! ライセンス: UnRARのソースは「RARの圧縮アルゴリズムを再現するために使ってはならない」という条件付き。
//! 展開だけなら利用できる。配布物には NOTICE にその旨を載せること。
//!
//! unrar はファイルへ書き出す形式なので、一時ファイルへ展開してから `sink` に渡す。

use std::fs::{self, File};
use std::path::Path;

use unrar::Archive;

use super::{Backend, Listing, Sink};
use crate::view::{Entry, local_stamp, scratch_dir, set_stamp};
use crate::Error;

pub(crate) struct RarBackend;

fn map_err(e: unrar::error::UnrarError) -> Error {
    let msg = e.to_string();
    let lower = msg.to_ascii_lowercase();
    if lower.contains("password") || lower.contains("encrypt") {
        Error::Unsupported("パスワード付きのRARは未対応です".to_string())
    } else {
        Error::Archive(msg)
    }
}

/// DOS形式（上位16bitが日付、下位16bitが時刻）
fn dos_stamp(t: u32) -> (Option<String>, Option<std::time::SystemTime>) {
    let date = t >> 16;
    let time = t & 0xFFFF;
    let (y, mo, d) = (1980 + ((date >> 9) & 0x7F) as i32, ((date >> 5) & 0xF) as u8, (date & 0x1F) as u8);
    let (h, mi, s) = (((time >> 11) & 0x1F) as u8, ((time >> 5) & 0x3F) as u8, ((time & 0x1F) * 2) as u8);
    if mo == 0 || d == 0 {
        return (None, None);
    }
    local_stamp(y, mo, d, h, mi, s)
}

fn make_entry(index: usize, h: &unrar::FileHeader) -> Entry {
    let name = h.filename.to_string_lossy().into_owned();
    let mut e = Entry::new(index, &name, h.is_directory());
    e.size = h.unpacked_size;
    e.crc32 = h.file_crc;
    e.encrypted = h.is_encrypted();
    e.method = format!("RAR m{}", h.method.saturating_sub(0x30));
    set_stamp(&mut e, dos_stamp(h.file_time));
    e
}

impl Backend for RarBackend {
    fn list(&self, path: &Path) -> Result<Listing, Error> {
        let p = path.to_path_buf();
        let it = Archive::new(&p).open_for_listing().map_err(map_err)?;
        let mut entries = Vec::new();
        for h in it {
            let h = h.map_err(map_err)?;
            let e = make_entry(entries.len(), &h);
            entries.push(e);
        }
        Ok(Listing { entries, comment: String::new(), format: "RAR" })
    }

    fn walk(&self, path: &Path, entries: &[Entry], sink: &mut dyn Sink) -> Result<(), Error> {
        let p = path.to_path_buf();
        let tmp = scratch_dir("tote-rar").map_err(|e| Error::Archive(e.to_string()))?;
        let result = (|| -> Result<(), Error> {
            let mut archive = Archive::new(&p).open_for_processing().map_err(map_err)?;
            let mut i = 0usize;
            while let Some(cursor) = archive.read_header().map_err(map_err)? {
                let idx = i;
                i += 1;
                let is_file = entries.get(idx).is_some_and(|e| !e.is_dir);
                if is_file && sink.wants(idx) {
                    let out = tmp.join(idx.to_string());
                    match cursor.extract_to(&out) {
                        Ok(next) => {
                            archive = next;
                            match File::open(&out) {
                                Ok(mut f) => sink.file(idx, &mut f),
                                Err(e) => sink.fail(idx, format!("読み出せません: {e}")),
                            }
                            let _ = fs::remove_file(&out);
                        }
                        Err(e) => {
                            // この項目で書庫の読み出しが続けられなくなった
                            sink.fail(idx, map_err(e).to_string());
                            return Ok(());
                        }
                    }
                } else {
                    archive = cursor.skip().map_err(map_err)?;
                }
                if sink.remaining() == 0 {
                    break;
                }
            }
            Ok(())
        })();
        let _ = fs::remove_dir_all(&tmp);
        result
    }
}
