//! tote-core: ZIP作成のコアロジック。
//!
//! 方針:
//! - 入力が「フォルダ1つだけ」のときは、その中身をZIPのルート直下に入れる
//!   （展開すると同名フォルダが二重にならない）。
//! - 入力が複数、またはファイル単体のときは、各項目をルート直下に入れる。
//! - 出力先は入力と同じ場所。既存ファイルは上書きせず `name (1).zip` のように避ける。

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("入力がありません")]
    NoInput,
    #[error("見つかりません: {0}")]
    NotFound(PathBuf),
    #[error("入力が別々のフォルダにあります。出力先を決められません")]
    MixedParents,
    #[error("ドライブ直下は指定できません。中のフォルダやファイルを選んでください")]
    RootInput,
    #[error("I/Oエラー ({path}): {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("ZIPエラー: {0}")]
    Zip(#[from] zip::result::ZipError),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io { path: path.to_path_buf(), source }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// 出力先を明示する場合。Noneなら入力から自動決定。
    pub output: Option<PathBuf>,
    /// Deflateの圧縮レベル（0-9）。Noneで既定。
    pub level: Option<i64>,
}

impl Default for Options {
    fn default() -> Self {
        Self { output: None, level: None }
    }
}

#[derive(Debug)]
pub struct Summary {
    pub output: PathBuf,
    pub files: usize,
    pub dirs: usize,
    pub skipped: Vec<PathBuf>,
}

/// 入力から出力ZIPのパスを決める（衝突回避は含まない）。
fn default_output_stem(inputs: &[PathBuf]) -> Result<(PathBuf, String), Error> {
    let first = &inputs[0];
    if first.parent().is_none() {
        return Err(Error::RootInput);
    }
    let parent = first
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    for p in &inputs[1..] {
        let pp = p
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        if pp != parent {
            return Err(Error::MixedParents);
        }
    }

    let stem = if inputs.len() == 1 {
        if first.is_dir() {
            file_name(first)
        } else {
            first
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| file_name(first))
        }
    } else {
        // 複数選択: 親フォルダ名。親がルート等で名前がなければ "archive"。
        let abs = fs::canonicalize(&parent).unwrap_or(parent.clone());
        abs.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "archive".to_string())
    };
    Ok((parent, stem))
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".to_string())
}

/// `dir/stem.zip` が存在すれば `dir/stem (1).zip`, `(2)` ... と空きを探す。
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 1.. {
        let cand = dir.join(format!("{stem} ({n}).{ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    unreachable!()
}

fn mtime_of(meta: &fs::Metadata) -> DateTime {
    // ZIPの時刻はタイムゾーン情報を持たず、Windows系ツールはローカル時刻で格納する。
    // ローカルオフセットが取れなければUTCで代用する。
    meta.modified()
        .ok()
        .map(time::OffsetDateTime::from)
        .map(|utc| {
            let off = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
            utc.to_offset(off)
        })
        .and_then(|t| {
            DateTime::from_date_and_time(
                u16::try_from(t.year()).ok()?,
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second(),
            )
            .ok()
        })
        .unwrap_or_default()
}

struct Ctx<'a> {
    zip: ZipWriter<BufWriter<File>>,
    opts: SimpleFileOptions,
    used: HashSet<String>,
    out_abs: PathBuf,
    files: usize,
    dirs: usize,
    skipped: Vec<PathBuf>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl Ctx<'_> {
    /// ZIP内の名前が重複したら "name (1).ext" 形式で避ける（ルート直下の同名衝突用）。
    fn claim(&mut self, name: String) -> String {
        if self.used.insert(name.clone()) {
            return name;
        }
        let (base, ext) = match name.rfind('.') {
            Some(i) if i > 0 && !name[i..].contains('/') => (name[..i].to_string(), name[i..].to_string()),
            _ => (name.clone(), String::new()),
        };
        for n in 1.. {
            let cand = format!("{base} ({n}){ext}");
            if self.used.insert(cand.clone()) {
                return cand;
            }
        }
        unreachable!()
    }

    fn add_dir_entry(&mut self, zip_path: &str, meta: &fs::Metadata) -> Result<(), Error> {
        let o = self.opts.last_modified_time(mtime_of(meta));
        self.zip.add_directory(format!("{zip_path}/"), o)?;
        self.dirs += 1;
        Ok(())
    }

    fn add_file(&mut self, zip_path: &str, path: &Path, meta: &fs::Metadata) -> Result<(), Error> {
        let o = self.opts.last_modified_time(mtime_of(meta)).large_file(meta.len() >= 0xFFFF_FFFF);
        self.zip.start_file(zip_path, o)?;
        let mut f = File::open(path).map_err(io_err(path))?;
        io::copy(&mut f, &mut self.zip).map_err(io_err(path))?;
        self.files += 1;
        Ok(())
    }

    /// `path` をZIP内の `zip_path` として追加（フォルダなら再帰）。
    fn add_tree(&mut self, path: &Path, zip_path: &str) -> Result<(), Error> {
        let meta = fs::symlink_metadata(path).map_err(io_err(path))?;
        if meta.file_type().is_symlink() {
            self.skipped.push(path.to_path_buf());
            return Ok(());
        }
        if meta.is_dir() {
            self.add_dir_entry(zip_path, &meta)?;
            self.add_children(path, zip_path)?;
        } else if meta.is_file() {
            // 出力ZIP自身を取り込まない
            if fs::canonicalize(path).map(|p| p == self.out_abs).unwrap_or(false) {
                return Ok(());
            }
            self.add_file(zip_path, path, &meta)?;
        } else {
            self.skipped.push(path.to_path_buf());
        }
        Ok(())
    }

    /// フォルダの中身を `prefix` 配下に追加。prefixが空ならルート直下。
    fn add_children(&mut self, dir: &Path, prefix: &str) -> Result<(), Error> {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .map_err(io_err(dir))?
            .collect::<Result<_, _>>()
            .map_err(io_err(dir))?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let zp = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let zp = if prefix.is_empty() { self.claim(zp) } else { zp };
            self.add_tree(&e.path(), &zp)?;
        }
        Ok(())
    }
}

/// ZIPを作る。失敗時は作りかけの出力ファイルを削除する。
pub fn create_zip(inputs: &[PathBuf], opts: &Options) -> Result<Summary, Error> {
    if inputs.is_empty() {
        return Err(Error::NoInput);
    }
    for p in inputs {
        if fs::symlink_metadata(p).is_err() {
            return Err(Error::NotFound(p.clone()));
        }
    }

    let output = match &opts.output {
        Some(o) => o.clone(),
        None => {
            let (dir, stem) = default_output_stem(inputs)?;
            unique_path(&dir, &stem, "zip")
        }
    };

    let file = File::create(&output).map_err(io_err(&output))?;
    let out_abs = fs::canonicalize(&output).map_err(io_err(&output))?;

    let mut fo = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    if let Some(l) = opts.level {
        fo = fo.compression_level(Some(l));
    }

    let mut ctx = Ctx {
        zip: ZipWriter::new(BufWriter::new(file)),
        opts: fo,
        used: HashSet::new(),
        out_abs,
        files: 0,
        dirs: 0,
        skipped: Vec::new(),
        _marker: std::marker::PhantomData,
    };

    let result = (|| -> Result<(), Error> {
        let single_dir = inputs.len() == 1 && fs::metadata(&inputs[0]).map(|m| m.is_dir()).unwrap_or(false);
        if single_dir {
            // フォルダ単体: 中身をルート直下へ（二重フォルダ防止）
            ctx.add_children(&inputs[0], "")?;
        } else {
            for p in inputs {
                let name = ctx.claim(file_name(p));
                ctx.add_tree(p, &name)?;
            }
        }
        Ok(())
    })();

    let Ctx { zip, files, dirs, skipped, .. } = ctx;
    let finished = result.and_then(|_| {
        let mut w = zip.finish()?;
        io::Write::flush(&mut w).map_err(io_err(&output))?;
        Ok(())
    });

    match finished {
        Ok(()) => Ok(Summary { output, files, dirs, skipped }),
        Err(e) => {
            let _ = fs::remove_file(&output);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn names(zip_path: &Path) -> Vec<String> {
        let f = File::open(zip_path).unwrap();
        let mut a = zip::ZipArchive::new(f).unwrap();
        let mut v: Vec<String> = (0..a.len()).map(|i| a.by_index(i).unwrap().name().to_string()).collect();
        v.sort();
        v
    }

    fn touch(p: &Path, body: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    #[test]
    fn single_folder_contents_at_root() {
        let t = tempfile::tempdir().unwrap();
        let proj = t.path().join("proj");
        touch(&proj.join("a.txt"), "A");
        touch(&proj.join("sub/b.txt"), "B");
        fs::create_dir_all(proj.join("empty")).unwrap();

        let s = create_zip(&[proj.clone()], &Options::default()).unwrap();
        assert_eq!(s.output, t.path().join("proj.zip"));
        assert_eq!(names(&s.output), vec!["a.txt", "empty/", "sub/", "sub/b.txt"]);
        // "proj/" という二重フォルダが無いこと
        assert!(!names(&s.output).iter().any(|n| n.starts_with("proj")));
    }

    #[test]
    fn multiple_inputs_each_at_root_and_named_after_parent() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("work");
        touch(&base.join("x.txt"), "X");
        touch(&base.join("dir/y.txt"), "Y");
        let s = create_zip(&[base.join("x.txt"), base.join("dir")], &Options::default()).unwrap();
        assert_eq!(s.output, base.join("work.zip"));
        assert_eq!(names(&s.output), vec!["dir/", "dir/y.txt", "x.txt"]);
    }

    #[test]
    fn single_file_uses_stem() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("note.md"), "hi");
        let s = create_zip(&[t.path().join("note.md")], &Options::default()).unwrap();
        assert_eq!(s.output, t.path().join("note.zip"));
        assert_eq!(names(&s.output), vec!["note.md"]);
    }

    #[test]
    fn existing_output_is_not_overwritten() {
        let t = tempfile::tempdir().unwrap();
        let proj = t.path().join("proj");
        touch(&proj.join("a.txt"), "A");
        let s1 = create_zip(&[proj.clone()], &Options::default()).unwrap();
        let s2 = create_zip(&[proj.clone()], &Options::default()).unwrap();
        assert_eq!(s1.output, t.path().join("proj.zip"));
        assert_eq!(s2.output, t.path().join("proj (1).zip"));
    }

    #[test]
    fn output_inside_input_folder_is_not_self_included() {
        let t = tempfile::tempdir().unwrap();
        let proj = t.path().join("proj");
        touch(&proj.join("a.txt"), "A");
        let out = proj.join("out.zip");
        let s = create_zip(&[proj.clone()], &Options { output: Some(out.clone()), level: None }).unwrap();
        assert_eq!(names(&s.output), vec!["a.txt"]);
    }

    #[test]
    fn content_roundtrip_and_japanese_names() {
        let t = tempfile::tempdir().unwrap();
        let proj = t.path().join("資料");
        touch(&proj.join("日本語/メモ.txt"), "こんにちは");
        let s = create_zip(&[proj], &Options::default()).unwrap();
        let mut a = zip::ZipArchive::new(File::open(&s.output).unwrap()).unwrap();
        let mut f = a.by_name("日本語/メモ.txt").unwrap();
        let mut body = String::new();
        f.read_to_string(&mut body).unwrap();
        assert_eq!(body, "こんにちは");
    }

    #[test]
    fn missing_input_errors_and_leaves_no_file() {
        let t = tempfile::tempdir().unwrap();
        let r = create_zip(&[t.path().join("nope")], &Options::default());
        assert!(matches!(r, Err(Error::NotFound(_))));
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 0);
    }

    #[test]
    fn mixed_parents_rejected() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("a/x.txt"), "1");
        touch(&t.path().join("b/y.txt"), "2");
        let r = create_zip(&[t.path().join("a/x.txt"), t.path().join("b/y.txt")], &Options::default());
        assert!(matches!(r, Err(Error::MixedParents)));
    }
}
