//! tote-core: ZIP作成と、各種書庫の閲覧・展開のコアロジック。
//!
//! 方針:
//! - 入力が「フォルダ1つだけ」のときは、その中身をZIPのルート直下に入れる
//!   （展開すると同名フォルダが二重にならない）。
//! - 入力が複数、またはファイル単体のときは、各項目をルート直下に入れる。
//! - 出力先は入力と同じ場所。既存ファイルは上書きせず `name (1).zip` のように避ける。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use zip::DateTime;

pub use create::{create, create_zip};

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
    #[error("{0}")]
    Unsupported(String),
    #[error("書庫を読めません: {0}")]
    Archive(String),
}

pub(crate) fn io_err(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io { path: path.to_path_buf(), source }
}

/// 作成できる書庫の形式。形式を足すときは、ここと `create` の分岐、`presets` を増やす。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressFormat {
    #[default]
    Zip,
    SevenZ,
    TarGz,
    Tar,
}

/// 圧縮レベルの選択肢（設定画面・ダイアログにそのまま出す）
#[derive(Debug, Clone, Copy)]
pub struct LevelPreset {
    pub id: &'static str,
    pub label: &'static str,
    /// その形式に渡すレベル値。None は形式の既定
    pub level: Option<i64>,
}

const ZIP_PRESETS: &[LevelPreset] = &[
    LevelPreset { id: "store", label: "圧縮しない（最速）", level: Some(0) },
    LevelPreset { id: "fast", label: "速度優先", level: Some(1) },
    LevelPreset { id: "normal", label: "標準", level: None },
    LevelPreset { id: "best", label: "最高圧縮（遅い）", level: Some(9) },
];

// 7z: 0 は無圧縮（COPY）、1〜9 は LZMA2 のレベル。最高でもメモリを使い過ぎない 7 までにしている
const SEVENZ_PRESETS: &[LevelPreset] = &[
    LevelPreset { id: "store", label: "圧縮しない（最速）", level: Some(0) },
    LevelPreset { id: "fast", label: "速度優先", level: Some(1) },
    LevelPreset { id: "normal", label: "標準", level: None },
    LevelPreset { id: "best", label: "最高圧縮（遅い）", level: Some(7) },
];

const TARGZ_PRESETS: &[LevelPreset] = &[
    LevelPreset { id: "fast", label: "速度優先", level: Some(1) },
    LevelPreset { id: "normal", label: "標準", level: None },
    LevelPreset { id: "best", label: "最高圧縮（遅い）", level: Some(9) },
];

impl CompressFormat {
    pub const ALL: &'static [CompressFormat] =
        &[CompressFormat::Zip, CompressFormat::SevenZ, CompressFormat::TarGz, CompressFormat::Tar];

    pub fn id(self) -> &'static str {
        match self {
            CompressFormat::Zip => "zip",
            CompressFormat::SevenZ => "7z",
            CompressFormat::TarGz => "targz",
            CompressFormat::Tar => "tar",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CompressFormat::Zip => "ZIP",
            CompressFormat::SevenZ => "7z",
            CompressFormat::TarGz => "tar.gz",
            CompressFormat::Tar => "tar（圧縮なし）",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            CompressFormat::Zip => "zip",
            CompressFormat::SevenZ => "7z",
            CompressFormat::TarGz => "tar.gz",
            CompressFormat::Tar => "tar",
        }
    }

    /// 圧縮レベルの選択肢。空ならレベルの概念がない形式
    pub fn presets(self) -> &'static [LevelPreset] {
        match self {
            CompressFormat::Zip => ZIP_PRESETS,
            CompressFormat::SevenZ => SEVENZ_PRESETS,
            CompressFormat::TarGz => TARGZ_PRESETS,
            CompressFormat::Tar => &[],
        }
    }

    /// パスワード（暗号化）を付けられる形式か
    pub fn supports_password(self) -> bool {
        matches!(self, CompressFormat::Zip | CompressFormat::SevenZ)
    }

    /// 固体圧縮（全ファイルをまとめて圧縮して圧縮率を上げる）を選べる形式か
    pub fn supports_solid(self) -> bool {
        matches!(self, CompressFormat::SevenZ)
    }

    /// ファイル名まで暗号化できる形式か
    pub fn supports_name_encryption(self) -> bool {
        matches!(self, CompressFormat::SevenZ)
    }

    pub fn from_id(id: &str) -> Option<CompressFormat> {
        Self::ALL.iter().copied().find(|f| f.id() == id)
    }

    /// プリセットIDからレベル値を引く。その形式に無いIDは既定（None）。
    pub fn level_for(self, preset_id: &str) -> Option<i64> {
        self.presets().iter().find(|p| p.id == preset_id).and_then(|p| p.level)
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// 出力先を明示する場合。Noneなら入力から自動決定。
    pub output: Option<PathBuf>,
    /// 圧縮レベル（形式ごとの値。ZIPは 0-9）。Noneで既定。
    pub level: Option<i64>,
    pub format: CompressFormat,
    /// パスワード（暗号化は AES-256）。対応しない形式では `create` がエラーにする。
    pub password: Option<String>,
    /// 固体圧縮（7z）。既定は true
    pub solid: bool,
    /// ファイル名も暗号化する（7z。パスワードが無いと中身の一覧も見えなくなる）
    pub encrypt_names: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { output: None, level: None, format: CompressFormat::default(), password: None, solid: true, encrypt_names: false }
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
pub(crate) fn default_output_stem(inputs: &[PathBuf]) -> Result<(PathBuf, String), Error> {
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

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".to_string())
}

/// 入力から出力先の候補（同じ場所・空いている名前）を決める。作成ダイアログの初期値用。
pub fn suggest_output(inputs: &[PathBuf], format: CompressFormat) -> Result<PathBuf, Error> {
    if inputs.is_empty() {
        return Err(Error::NoInput);
    }
    let (dir, stem) = default_output_stem(inputs)?;
    Ok(unique_path(&dir, &stem, format.extension()))
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

pub(crate) fn mtime_of(meta: &fs::Metadata) -> DateTime {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
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
        let s = create_zip(&[proj.clone()], &Options { output: Some(out.clone()), ..Options::default() }).unwrap();
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

pub mod append;
mod create;
pub mod formats;
mod plan;
pub mod view;
