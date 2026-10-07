//! アーカイブの閲覧と、選択した項目だけの展開（ZIP / 7z / tar.gz / gz / cab / lzh / rar）。
//!
//! - 一覧は展開せずに中身だけを読む（可能な形式では中央ディレクトリだけ。tar.gz などは全体を走査する）。
//! - 古い日本語環境の書庫はファイル名が Shift_JIS のままなので、生のバイト列から復元する。
//! - 展開は「選択した項目を、dest 直下に名前だけ残して」置く（WinRARのドラッグ展開と同じ）。
//!   既存の名前と衝突したら上書きせず `name (1)` で避ける。`..` を含むパスは展開しない。
//! - 形式ごとの読み出しは `formats` にあり、ここは形式に依らない計画（どこへ置くか）と書き出しを担当する。

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use crate::formats::{self, Sink};
use crate::{Error, io_err};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub index: usize,
    /// `/` 区切り。先頭・末尾の `/` は付けない。
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// 格納サイズ。固体圧縮などで個別に分からない形式は 0。
    pub packed: u64,
    /// "YYYY-MM-DD HH:MM"（書庫に格納されたローカル時刻のまま）
    pub modified: Option<String>,
    pub crc32: u32,
    pub method: String,
    pub encrypted: bool,
    /// `..` を含むなど、そのままでは展開できないパスなら false
    pub safe: bool,
    /// 実行形式・スクリプトなど、開く前に注意したい種類
    pub risky: bool,
    /// シンボリックリンク・特殊ファイル（展開しない）
    pub symlink: bool,
    /// 展開時に復元する更新日時
    #[serde(skip)]
    pub mtime: Option<SystemTime>,
}

impl Entry {
    /// 名前（区切りは `/` でも `\` でもよい）から、安全性などを判定した既定値つきのエントリを作る。
    pub(crate) fn new(index: usize, raw_name: &str, is_dir_hint: bool) -> Entry {
        let is_dir = is_dir_hint || raw_name.ends_with('/') || raw_name.ends_with('\\');
        let mut path = normalize(raw_name);
        if path == "." {
            path.clear(); // 7z などのルート自身
        }
        Entry {
            index,
            // 空のパス（tar の "./" など、書庫のルート自身）は展開するものが無いだけで、危険ではない
            safe: path.is_empty() || safe_components(&path).is_some(),
            risky: !is_dir && is_risky(&path),
            path,
            is_dir,
            size: 0,
            packed: 0,
            modified: None,
            crc32: 0,
            method: String::new(),
            encrypted: false,
            symlink: false,
            mtime: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveInfo {
    pub entries: Vec<Entry>,
    pub total_size: u64,
    pub total_packed: u64,
    pub comment: String,
    /// "ZIP" "7z" "tar.gz" など、画面に出す形式名
    pub format: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractReport {
    /// dest 直下に作られた項目（ドラッグ元として渡す対象）
    pub items: Vec<PathBuf>,
    pub files: usize,
    pub skipped: Vec<Skipped>,
}

// ---------------------------------------------------------------- 名前の扱い

const RISKY_EXT: &[&str] = &[
    "exe", "com", "scr", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msi", "msp", "lnk",
    "jar", "hta", "cpl", "reg", "dll", "pif", "sys", "inf", "gadget", "msc", "url",
];

pub fn is_risky(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => RISKY_EXT.contains(&ext.to_ascii_lowercase().as_str()),
        _ => false,
    }
}

/// 書庫内の生の名前を文字列に直す。UTF-8 → Shift_JIS(CP932) → `fallback` の順に試す。
pub fn decode_name(raw: &[u8], fallback: &str) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    let (s, _, had_errors) = encoding_rs::SHIFT_JIS.decode(raw);
    if !had_errors {
        return s.into_owned();
    }
    fallback.to_string()
}

/// `\` を `/` にし、先頭の `./`・`/` と末尾の `/` を落とす。
pub(crate) fn normalize(name: &str) -> String {
    let mut s = name.replace('\\', "/");
    loop {
        let t = s.trim_start_matches('/');
        match t.strip_prefix("./") {
            Some(rest) => s = rest.to_string(),
            None => {
                s = t.to_string();
                break;
            }
        }
    }
    s.trim_end_matches('/').to_string()
}

fn is_reserved(stem: &str) -> bool {
    let up = stem.to_ascii_uppercase();
    matches!(up.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (up.len() == 4 && (up.starts_with("COM") || up.starts_with("LPT")) && up.as_bytes()[3].is_ascii_digit())
}

/// 1要素をWindowsでも作れる名前にする。
fn sanitize_component(c: &str) -> String {
    let mut s: String =
        c.chars().map(|ch| if ch.is_control() || "<>:\"|?*".contains(ch) { '_' } else { ch }).collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    let stem = s.split('.').next().unwrap_or("");
    if is_reserved(stem) {
        s.insert(0, '_');
    }
    s
}

/// 展開してよいパスなら、サニタイズ済みの要素列を返す。`..` を含めば None。
pub(crate) fn safe_components(path: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => continue,
            ".." => return None,
            _ => {
                let s = sanitize_component(c);
                if !s.is_empty() {
                    out.push(s);
                }
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

// ---------------------------------------------------------------- 日時

fn local_offset() -> time::UtcOffset {
    time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC)
}

/// 書庫に入っていたローカル時刻（年月日時分秒）→ (表示用の文字列, 復元用の時刻)
pub(crate) fn local_stamp(y: i32, mo: u8, d: u8, h: u8, mi: u8, s: u8) -> (Option<String>, Option<SystemTime>) {
    let text = format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}");
    let t = (|| {
        let date = time::Date::from_calendar_date(y, time::Month::try_from(mo).ok()?, d).ok()?;
        let t = time::Time::from_hms(h, mi, s).ok()?;
        Some(SystemTime::from(time::PrimitiveDateTime::new(date, t).assume_offset(local_offset())))
    })();
    (Some(text), t)
}

/// UNIX秒（UTC）→ (ローカル時刻の表示用文字列, 復元用の時刻)
pub(crate) fn unix_stamp(secs: i64) -> (Option<String>, Option<SystemTime>) {
    let Ok(dt) = time::OffsetDateTime::from_unix_timestamp(secs) else { return (None, None) };
    let l = dt.to_offset(local_offset());
    let text = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        l.year(),
        u8::from(l.month()),
        l.day(),
        l.hour(),
        l.minute()
    );
    (Some(text), Some(SystemTime::from(dt)))
}

/// `SystemTime`（UTC基準）→ (ローカル時刻の表示用文字列, 復元用の時刻)
pub(crate) fn system_stamp(t: SystemTime) -> (Option<String>, Option<SystemTime>) {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => unix_stamp(d.as_secs() as i64),
        Err(_) => (None, None),
    }
}

/// 一覧用の日時と復元用の時刻を、エントリに設定する。
pub(crate) fn set_stamp(e: &mut Entry, stamp: (Option<String>, Option<SystemTime>)) {
    e.modified = stamp.0;
    e.mtime = stamp.1;
}

// ---------------------------------------------------------------- 一覧

/// 書庫の中身を一覧する（展開はしない）。形式は中身（先頭バイト）で判定し、だめなら拡張子で判定する。
pub fn list(path: &Path) -> Result<ArchiveInfo, Error> {
    let kind = formats::detect(path)?;
    let listing = formats::backend(kind).list(path)?;
    let entries = listing.entries;
    let total_size = entries.iter().map(|e| e.size).sum();
    let mut total_packed: u64 = entries.iter().map(|e| e.packed).sum();
    if total_packed == 0 {
        // 固体圧縮などで個別の格納サイズが無い形式は、書庫ファイル自体の大きさを出す
        total_packed = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    Ok(ArchiveInfo { entries, total_size, total_packed, comment: listing.comment, format: listing.format.to_string() })
}

/// ZIPの一覧（互換用。`list` と同じ）。
pub fn list_zip(zip_path: &Path) -> Result<ArchiveInfo, Error> {
    list(zip_path)
}

// ---------------------------------------------------------------- 展開

fn unique_name(dir: &Path, name: &str, planned: &HashSet<PathBuf>) -> String {
    let taken = |n: &str| dir.join(n).exists() || planned.contains(&dir.join(n));
    if !taken(name) {
        return name.to_string();
    }
    let (base, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1.. {
        let cand = format!("{base} ({n}){ext}");
        if !taken(&cand) {
            return cand;
        }
    }
    unreachable!()
}

/// 展開前に決めておく1ファイルぶんの予定
struct Target {
    path: PathBuf,
    mtime: Option<SystemTime>,
}

/// 展開の書き出し側。形式ごとの読み出し（formats）から呼ばれる。
struct Writer<'a> {
    entries: &'a [Entry],
    targets: HashMap<usize, Target>,
    done: HashSet<usize>,
    report: &'a mut ExtractReport,
    fatal: Option<Error>,
}

impl Sink for Writer<'_> {
    fn wants(&self, index: usize) -> bool {
        self.fatal.is_none() && self.targets.contains_key(&index) && !self.done.contains(&index)
    }

    fn remaining(&self) -> usize {
        if self.fatal.is_some() { 0 } else { self.targets.len() - self.done.len() }
    }

    fn file(&mut self, index: usize, data: &mut dyn Read) {
        let Some(t) = self.targets.get(&index) else { return };
        self.done.insert(index);
        let target = t.path.clone();
        let mtime = t.mtime;
        if let Some(parent) = target.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                self.fatal = Some(Error::Io { path: parent.to_path_buf(), source: e });
                return;
            }
        }
        let mut out = match File::create(&target) {
            Ok(f) => f,
            Err(e) => {
                self.fatal = Some(Error::Io { path: target, source: e });
                return;
            }
        };
        if let Err(err) = io::copy(data, &mut out) {
            drop(out);
            let _ = fs::remove_file(&target);
            self.report.skipped.push(Skipped {
                path: self.entries[index].path.clone(),
                reason: format!("展開に失敗: {err}"),
            });
            return;
        }
        if let Some(t) = mtime {
            let _ = out.set_modified(t);
        }
        self.report.files += 1;
    }

    fn fail(&mut self, index: usize, reason: String) {
        if self.targets.contains_key(&index) && self.done.insert(index) {
            self.report.skipped.push(Skipped { path: self.entries[index].path.clone(), reason });
        }
    }
}

/// `selection` の項目（ファイルまたはフォルダ。フォルダは中身ごと）を `dest` 直下に展開する。
/// `selection` が空なら全部。書庫内の階層は、選択した項目の位置から下だけが残る。
pub fn extract(archive_path: &Path, selection: &[String], dest: &Path) -> Result<ExtractReport, Error> {
    let kind = formats::detect(archive_path)?;
    let backend = formats::backend(kind);
    let entries = backend.list(archive_path)?.entries;
    fs::create_dir_all(dest).map_err(io_err(dest))?;

    let sel: Vec<String> = selection.iter().map(|s| normalize(s)).filter(|s| !s.is_empty()).collect();

    // 各エントリについて「dest からの相対パス」を決める
    let rel_of = |path: &str| -> Option<String> {
        if sel.is_empty() {
            return Some(path.to_string());
        }
        for s in &sel {
            if path == s || path.starts_with(&format!("{s}/")) {
                let parent_len = s.rfind('/').map(|i| i + 1).unwrap_or(0);
                return Some(path[parent_len..].to_string());
            }
        }
        None
    };

    let mut report = ExtractReport { items: Vec::new(), files: 0, skipped: Vec::new() };
    let mut top_map: Vec<(String, String)> = Vec::new(); // 最上位の名前 → 衝突回避後の名前
    let mut planned: HashSet<PathBuf> = HashSet::new();
    let mut file_targets: HashSet<PathBuf> = HashSet::new();
    let mut targets: HashMap<usize, Target> = HashMap::new();
    let mut dirs: Vec<PathBuf> = Vec::new();

    // ---- 計画: どの項目をどこへ置くか（ここではまだ書かない）
    for e in &entries {
        if e.path.is_empty() {
            continue; // 書庫のルート自身（"./"）
        }
        let Some(rel) = rel_of(&e.path) else { continue };
        let skip = |report: &mut ExtractReport, reason: &str| {
            report.skipped.push(Skipped { path: e.path.clone(), reason: reason.to_string() });
        };
        if !e.safe {
            skip(&mut report, "不正なパス（..を含む）");
            continue;
        }
        if e.symlink {
            skip(&mut report, "リンクなどの特殊ファイル");
            continue;
        }
        if e.encrypted && !e.is_dir {
            skip(&mut report, "パスワード付き（未対応）");
            continue;
        }
        let Some(mut comps) = safe_components(&rel) else {
            skip(&mut report, "不正なパス");
            continue;
        };

        // 最上位の名前は、既存と被らないように決める（同じ最上位は同じ名前に）
        let top = comps[0].clone();
        let mapped = match top_map.iter().find(|(from, _)| *from == top) {
            Some((_, to)) => to.clone(),
            None => {
                let to = unique_name(dest, &top, &planned);
                planned.insert(dest.join(&to));
                top_map.push((top.clone(), to.clone()));
                report.items.push(dest.join(&to));
                to
            }
        };
        comps[0] = mapped;
        let mut target = dest.to_path_buf();
        comps.iter().for_each(|c| target.push(c));

        if e.is_dir {
            dirs.push(target);
            continue;
        }
        // 同じパスが書庫内に複数ある場合も上書きしない
        let target = if file_targets.contains(&target) || target.exists() {
            let dir = target.parent().unwrap_or(dest).to_path_buf();
            let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            dir.join(unique_name(&dir, &name, &planned))
        } else {
            target
        };
        planned.insert(target.clone());
        file_targets.insert(target.clone());
        targets.insert(e.index, Target { path: target, mtime: e.mtime });
    }

    for d in &dirs {
        fs::create_dir_all(d).map_err(io_err(d))?;
    }
    if targets.is_empty() {
        return Ok(report);
    }

    // ---- 書き出し
    let mut writer = Writer { entries: &entries, targets, done: HashSet::new(), report: &mut report, fatal: None };
    let walked = backend.walk(archive_path, &entries, &mut writer);
    if let Some(e) = writer.fatal.take() {
        return Err(e);
    }
    if let Err(e) = walked {
        // 途中で読めなくなった（壊れた書庫など）。未処理の分は理由つきでスキップ扱いにする
        let reason = format!("読み出せません: {e}");
        let pending: Vec<usize> = writer.targets.keys().copied().filter(|i| !writer.done.contains(i)).collect();
        for i in pending {
            writer.fail(i, reason.clone());
        }
    }
    // 読み出し側が通らなかった分（形式側の取りこぼし）も理由を付ける
    let pending: Vec<usize> = writer.targets.keys().copied().filter(|i| !writer.done.contains(i)).collect();
    for i in pending {
        writer.fail(i, "書庫から見つかりませんでした".to_string());
    }
    Ok(report)
}

// ---------------------------------------------------------------- 一時フォルダ

/// ドラッグ展開や「開く」用の一時フォルダを作る。
pub fn scratch_dir(prefix: &str) -> io::Result<PathBuf> {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(prefix);
    let dir = root.join(format!("{nanos}-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// `prefix` 配下で、`max_age` より古い一時フォルダを消す（起動時の掃除用）。
pub fn cleanup_scratch(prefix: &str, max_age: std::time::Duration) {
    let root = std::env::temp_dir().join(prefix);
    let Ok(rd) = fs::read_dir(&root) else { return };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}
