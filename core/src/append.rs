//! 既存のZIPへファイルを追加する（ビューアにドロップされたとき）。
//!
//! 追加方式は2つ:
//! - `Safe`: 同じフォルダに一時ファイルを作り、元の項目を生のままコピー（再圧縮なし）＋新しい項目を書いて、
//!   最後に元ファイルと差し替える。途中で落ちても元のZIPは無傷。大きいZIPはコピー分だけ時間がかかる。
//! - `Fast`: ZIPの末尾（中央ディレクトリの位置）からその場で書き足す。速いが、書き込み中に落ちるとZIPが壊れる。
//!   同名の項目を置き換える必要があるときは、その場追記では消せないので `Safe` に切り替える。

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::{Error, io_err, mtime_of};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppendMode {
    #[default]
    Safe,
    Fast,
}

impl AppendMode {
    /// 設定ファイルの値（"safe" / "fast"）から。知らない値は安全側。
    pub fn from_id(id: &str) -> AppendMode {
        if id == "fast" { AppendMode::Fast } else { AppendMode::Safe }
    }
}

#[derive(Debug)]
pub struct AppendSummary {
    pub files: usize,
    pub dirs: usize,
    /// 同名のため置き換えた既存ファイルの数
    pub replaced: usize,
    pub skipped: Vec<PathBuf>,
    /// 実際に使った方式（Fast を頼まれても置き換えがあれば Safe になる）
    pub mode_used: AppendMode,
}

struct Item {
    name: String,
    path: PathBuf,
    meta: fs::Metadata,
}

impl Item {
    fn is_dir(&self) -> bool {
        self.meta.is_dir()
    }
}

/// ZIP内のフォルダ指定を整える（`a//b/` → `a/b`）。`..` は拒否。
fn clean_dir(dir: &str) -> Result<String, Error> {
    let parts: Vec<&str> = dir.split(['/', '\\']).filter(|s| !s.is_empty() && *s != ".").collect();
    if parts.contains(&"..") {
        return Err(Error::Unsupported("追加先のパスが不正です".into()));
    }
    Ok(parts.join("/"))
}

fn base_name(p: &Path) -> Result<String, Error> {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| Error::Unsupported(format!("追加できないパスです: {}", p.display())))
}

/// 同じ呼び出しの中で名前が被ったら `name (1).ext` にして避ける
fn claim(used: &mut HashSet<String>, name: String) -> String {
    if used.insert(name.clone()) {
        return name;
    }
    let (base, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    for n in 1.. {
        let cand = format!("{base} ({n}){ext}");
        if used.insert(cand.clone()) {
            return cand;
        }
    }
    unreachable!()
}

fn walk(path: &Path, name: String, out: &mut Vec<Item>, skipped: &mut Vec<PathBuf>, self_abs: &Path) -> Result<(), Error> {
    let meta = fs::symlink_metadata(path).map_err(io_err(path))?;
    if meta.file_type().is_symlink() {
        skipped.push(path.to_path_buf());
    } else if meta.is_dir() {
        out.push(Item { name: name.clone(), path: path.to_path_buf(), meta });
        let mut kids: Vec<_> = fs::read_dir(path).map_err(io_err(path))?.collect::<Result<_, _>>().map_err(io_err(path))?;
        kids.sort_by_key(|e| e.file_name());
        for k in kids {
            let kn = format!("{name}/{}", k.file_name().to_string_lossy());
            walk(&k.path(), kn, out, skipped, self_abs)?;
        }
    } else if meta.is_file() {
        // 書庫自身を自分の中へ入れない
        if fs::canonicalize(path).map(|p| p == self_abs).unwrap_or(false) {
            return Ok(());
        }
        out.push(Item { name, path: path.to_path_buf(), meta });
    } else {
        skipped.push(path.to_path_buf());
    }
    Ok(())
}

fn file_options(level: Option<i64>) -> SimpleFileOptions {
    let mut fo = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    match level {
        Some(0) => fo = fo.compression_method(CompressionMethod::Stored),
        Some(l) => fo = fo.compression_level(Some(l)),
        None => {}
    }
    fo
}

fn write_items<W: Write + io::Seek>(zip: &mut ZipWriter<W>, items: &[Item], skip_dirs: &HashSet<String>, level: Option<i64>) -> Result<(usize, usize), Error> {
    let base = file_options(level);
    let (mut files, mut dirs) = (0, 0);
    for it in items {
        let o = base.last_modified_time(mtime_of(&it.meta));
        if it.is_dir() {
            if skip_dirs.contains(&it.name) {
                continue; // 同名のフォルダ項目は既にある
            }
            zip.add_directory(format!("{}/", it.name), o)?;
            dirs += 1;
        } else {
            zip.start_file(&it.name, o.large_file(it.meta.len() >= 0xFFFF_FFFF))?;
            let mut f = File::open(&it.path).map_err(io_err(&it.path))?;
            io::copy(&mut f, zip).map_err(io_err(&it.path))?;
            files += 1;
        }
    }
    Ok((files, dirs))
}

/// `inputs` を `zip_path` の中の `dest_dir`（空ならルート）へ追加する。同名のファイルは置き換える。
pub fn add_to_zip(zip_path: &Path, dest_dir: &str, inputs: &[PathBuf], mode: AppendMode, level: Option<i64>) -> Result<AppendSummary, Error> {
    if inputs.is_empty() {
        return Err(Error::NoInput);
    }
    let dest = clean_dir(dest_dir)?;
    let self_abs = fs::canonicalize(zip_path).map_err(io_err(zip_path))?;

    // 追加する項目を洗い出す
    let mut used = HashSet::new();
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    for p in inputs {
        let n = claim(&mut used, base_name(p)?);
        let name = if dest.is_empty() { n } else { format!("{dest}/{n}") };
        walk(p, name, &mut items, &mut skipped, &self_abs)?;
    }

    // 既存の項目と突き合わせる
    let existing = File::open(zip_path).map_err(io_err(zip_path))?;
    let mut archive = ZipArchive::new(existing).map_err(|e| Error::Archive(e.to_string()))?;
    let mut index: HashMap<String, (usize, bool)> = HashMap::new();
    for i in 0..archive.len() {
        let f = archive.by_index_raw(i)?;
        index.insert(f.name().trim_end_matches('/').to_string(), (i, f.is_dir()));
    }
    let mut replace: HashSet<usize> = HashSet::new();
    let mut have_dirs: HashSet<String> = HashSet::new();
    for it in &items {
        if let Some(&(i, was_dir)) = index.get(&it.name) {
            match (was_dir, it.is_dir()) {
                (true, true) => {
                    have_dirs.insert(it.name.clone());
                }
                (false, false) => {
                    replace.insert(i);
                }
                _ => return Err(Error::Unsupported(format!("同じ名前のフォルダとファイルがあるため追加できません: {}", it.name))),
            }
        }
    }

    let used_mode = if mode == AppendMode::Fast && replace.is_empty() { AppendMode::Fast } else { AppendMode::Safe };
    let replaced = replace.len();

    let (files, dirs) = match used_mode {
        AppendMode::Fast => {
            drop(archive);
            let f = OpenOptions::new().read(true).write(true).open(zip_path).map_err(io_err(zip_path))?;
            let mut w = ZipWriter::new_append(f)?;
            let r = write_items(&mut w, &items, &have_dirs, level)?;
            let f = w.finish()?;
            f.sync_all().map_err(io_err(zip_path))?;
            r
        }
        AppendMode::Safe => {
            let dir = zip_path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let tmp = dir.join(format!(".{}.tote-{}.tmp", base_name(zip_path)?, std::process::id()));
            let res = (|| -> Result<(usize, usize), Error> {
                let out = File::create(&tmp).map_err(io_err(&tmp))?;
                let mut w = ZipWriter::new(BufWriter::new(out));
                if !archive.comment().is_empty() {
                    let _ = w.set_raw_comment(archive.comment().into());
                }
                for i in 0..archive.len() {
                    if replace.contains(&i) {
                        continue;
                    }
                    w.raw_copy_file(archive.by_index_raw(i)?)?;
                }
                let r = write_items(&mut w, &items, &have_dirs, level)?;
                let mut bw = w.finish()?;
                bw.flush().map_err(io_err(&tmp))?;
                bw.into_inner().map_err(|e| io_err(&tmp)(e.into_error()))?.sync_all().map_err(io_err(&tmp))?;
                Ok(r)
            })();
            match res {
                Ok(r) => {
                    drop(archive);
                    if let Err(e) = fs::rename(&tmp, zip_path) {
                        let _ = fs::remove_file(&tmp);
                        return Err(Error::Io { path: zip_path.to_path_buf(), source: e });
                    }
                    r
                }
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    return Err(e);
                }
            }
        }
    };

    Ok(AppendSummary { files, dirs, replaced, skipped, mode_used: used_mode })
}
