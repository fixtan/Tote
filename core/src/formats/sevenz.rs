//! 7z。ブロック単位で復号するので、固体でない書庫は必要なブロックだけ展開できる。

use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;

use sevenz_rust2::{Archive, ArchiveEntry, BlockDecoder, EncoderMethod, Error as SzError, Password};

use super::{Backend, Listing, Sink};
use crate::view::{Entry, system_stamp, set_stamp};
use crate::{Error, io_err};

pub(crate) struct SevenZBackend;

fn map_err(e: SzError) -> Error {
    match e {
        SzError::PasswordRequired | SzError::MaybeBadPassword(_) => {
            Error::Unsupported("パスワード付きの7zは未対応です".to_string())
        }
        other => Error::Archive(other.to_string()),
    }
}

fn open(path: &Path) -> Result<Archive, Error> {
    Archive::open(path).map_err(map_err)
}

fn block_has_aes(archive: &Archive, block: usize) -> bool {
    archive.blocks[block].coders.iter().any(|c| c.encoder_method_id() == EncoderMethod::ID_AES256_SHA256)
}

fn method_name(archive: &Archive, block: usize) -> String {
    let names: Vec<&str> = archive.blocks[block]
        .ordered_coder_iter()
        .filter_map(|(_, c)| EncoderMethod::by_id(c.encoder_method_id()).map(|m| m.name()))
        .collect();
    names.join("+")
}

/// 7z のヘッダーでは、フォルダ名や空ファイル（データを持たない項目）が、データを持つファイルの間に
/// 混ざっていることが多い。ライブラリのブロック復号は「ブロックの項目は連続している」前提なので、
/// 混ざっていると後ろのファイルを取りこぼす。そこで、ブロックごとに「データを持つ項目を先頭に寄せる」
/// 形に並べ替える（`files` を並べ替えるだけで、復号に使う他の情報は変わらない）。
///
/// 戻り値は、並べ替え後の位置 → 元の位置（`list` が返す `Entry::index`）の対応表。
fn group_stream_files(archive: &mut Archive) -> Vec<usize> {
    let n = archive.files.len();
    let mut order: Vec<usize> = (0..n).collect();
    for b in 0..archive.blocks.len() {
        let first = archive.stream_map.block_first_file_index[b];
        let members: Vec<usize> =
            (first..n).take_while(|&i| archive.stream_map.file_block_index[i] == Some(b)).collect();
        let (streams, empties): (Vec<usize>, Vec<usize>) =
            members.iter().partition(|&&i| archive.files[i].has_stream);
        for (k, &orig) in streams.iter().chain(empties.iter()).enumerate() {
            order[first + k] = orig;
        }
    }
    let old = archive.files.clone();
    for (new_i, &orig) in order.iter().enumerate() {
        archive.files[new_i] = old[orig].clone();
    }
    order
}

impl Backend for SevenZBackend {
    fn list(&self, path: &Path) -> Result<Listing, Error> {
        let archive = open(path)?;
        let mut entries = Vec::with_capacity(archive.files.len());
        for (i, f) in archive.files.iter().enumerate() {
            let mut e = Entry::new(i, f.name(), f.is_directory());
            e.size = f.size();
            e.crc32 = f.crc as u32;
            if f.has_last_modified_date {
                set_stamp(&mut e, system_stamp(std::time::SystemTime::from(f.last_modified_date)));
            }
            // Unix由来の属性（上位16bitがst_mode）でシンボリックリンクを見分ける
            let attr = f.windows_attributes;
            e.symlink = f.has_windows_attributes && attr & 0x8000 != 0 && (attr >> 16) & 0o170000 == 0o120000;
            if let Some(Some(b)) = archive.stream_map.file_block_index.get(i) {
                e.encrypted = block_has_aes(&archive, *b);
                e.method = method_name(&archive, *b);
            }
            entries.push(e);
        }
        Ok(Listing { entries, comment: String::new(), format: "7z" })
    }

    fn walk(&self, path: &Path, entries: &[Entry], sink: &mut dyn Sink) -> Result<(), Error> {
        let mut archive = open(path)?;
        let empty_outside: Vec<usize> = archive
            .stream_map
            .file_block_index
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_none())
            .map(|(i, _)| i)
            .collect();
        let order = group_stream_files(&mut archive);
        let file = File::open(path).map_err(io_err(path))?;
        let mut source = BufReader::new(file);
        let password = Password::empty();

        // データを持たない項目（空ファイル）
        for i in empty_outside {
            if sink.wants(i) {
                sink.file(i, &mut io::empty());
            }
        }

        let base = archive.files.as_ptr() as usize;
        let stride = std::mem::size_of::<ArchiveEntry>();
        let n_blocks = archive.blocks.len();
        for block in 0..n_blocks {
            if sink.remaining() == 0 {
                break;
            }
            let first = archive.stream_map.block_first_file_index[block];
            let dec = BlockDecoder::new(1, block, &archive, &password, &mut source);
            let count = dec.entry_count();
            if !(first..first + count).any(|i| sink.wants(order[i])) {
                continue; // このブロックに欲しい項目が無ければ復号しない
            }
            let r = dec.for_each_entries(&mut |entry: &ArchiveEntry, reader: &mut dyn io::Read| {
                let pos = (entry as *const ArchiveEntry as usize - base) / stride;
                let idx = order[pos];
                if sink.wants(idx) {
                    sink.file(idx, reader);
                }
                // 読み残しがあると後続の項目がずれるので、必ず読み切る
                let _ = io::copy(reader, &mut io::sink());
                // このブロックにまだ欲しい項目があれば続ける
                Ok((pos + 1..first + count).any(|i| sink.wants(order[i])))
            });
            if let Err(e) = r {
                let reason = map_err(e).to_string();
                for i in first..first + count {
                    if sink.wants(order[i]) {
                        sink.fail(order[i], reason.clone());
                    }
                }
            }
        }
        let _ = entries;
        Ok(())
    }
}
