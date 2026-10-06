//! ZIPの閲覧と、選択した項目だけの展開。
//!
//! - 一覧は展開せずに中央ディレクトリだけを読む（高速・安全に中身を確認できる）。
//! - 古い日本語環境のZIPはファイル名が Shift_JIS のままなので、raw のバイト列から復元する。
//! - 展開は「選択した項目を、dest 直下に名前だけ残して」置く（WinRARのドラッグ展開と同じ）。
//!   既存の名前と衝突したら上書きせず `name (1)` で避ける。`..` を含むパスは展開しない。

use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use zip::ZipArchive;

use crate::{Error, io_err};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub index: usize,
    /// `/` 区切り。先頭・末尾の `/` は付けない。
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub packed: u64,
    /// "YYYY-MM-DD HH:MM"（ZIPに格納されたローカル時刻のまま）
    pub modified: Option<String>,
    pub crc32: u32,
    pub method: String,
    pub encrypted: bool,
    /// `..` を含むなど、そのままでは展開できないパスなら false
    pub safe: bool,
    /// 実行形式・スクリプトなど、開く前に注意したい種類
    pub risky: bool,
    pub symlink: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveInfo {
    pub entries: Vec<Entry>,
    pub total_size: u64,
    pub total_packed: u64,
    pub comment: String,
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

/// ZIPの生ファイル名を文字列に直す。UTF-8 → Shift_JIS(CP932) → CP437 の順に試す。
pub fn decode_name(raw: &[u8], cp437_fallback: &str) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    let (s, _, had_errors) = encoding_rs::SHIFT_JIS.decode(raw);
    if !had_errors {
        return s.into_owned();
    }
    cp437_fallback.to_string()
}

/// `\` を `/` にし、先頭の `/`・末尾の `/` を落とす。
fn normalize(name: &str) -> String {
    name.replace('\\', "/").trim_matches('/').to_string()
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
fn safe_components(path: &str) -> Option<Vec<String>> {
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

// ---------------------------------------------------------------- 一覧

fn fmt_modified(dt: Option<zip::DateTime>) -> Option<String> {
    dt.map(|d| format!("{:04}-{:02}-{:02} {:02}:{:02}", d.year(), d.month(), d.day(), d.hour(), d.minute()))
}

fn read_entries<R: io::Read + io::Seek>(ar: &mut ZipArchive<R>) -> Result<Vec<Entry>, Error> {
    let mut out = Vec::with_capacity(ar.len());
    for i in 0..ar.len() {
        let f = ar.by_index_raw(i)?;
        let decoded = decode_name(f.name_raw(), f.name());
        let path = normalize(&decoded);
        let is_dir = f.is_dir() || decoded.ends_with('/') || decoded.ends_with('\\');
        out.push(Entry {
            index: i,
            safe: safe_components(&path).is_some(),
            risky: !is_dir && is_risky(&path),
            symlink: f.is_symlink(),
            path,
            is_dir,
            size: f.size(),
            packed: f.compressed_size(),
            modified: fmt_modified(f.last_modified()),
            crc32: f.crc32(),
            method: f.compression().to_string(),
            encrypted: f.encrypted(),
        });
    }
    Ok(out)
}

/// ZIPの中身を一覧する（展開はしない）。
pub fn list_zip(zip_path: &Path) -> Result<ArchiveInfo, Error> {
    let file = File::open(zip_path).map_err(io_err(zip_path))?;
    let mut ar = ZipArchive::new(BufReader::new(file))?;
    let entries = read_entries(&mut ar)?;
    let total_size = entries.iter().map(|e| e.size).sum();
    let total_packed = entries.iter().map(|e| e.packed).sum();
    let comment = decode_name(ar.comment(), &String::from_utf8_lossy(ar.comment()));
    Ok(ArchiveInfo { entries, total_size, total_packed, comment })
}

// ---------------------------------------------------------------- 展開

fn unique_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (base, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1.. {
        let cand = format!("{base} ({n}){ext}");
        if !dir.join(&cand).exists() {
            return cand;
        }
    }
    unreachable!()
}

fn to_system_time(dt: zip::DateTime) -> Option<SystemTime> {
    let date =
        time::Date::from_calendar_date(i32::from(dt.year()), time::Month::try_from(dt.month()).ok()?, dt.day()).ok()?;
    let t = time::Time::from_hms(dt.hour(), dt.minute(), dt.second()).ok()?;
    let off = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    Some(time::PrimitiveDateTime::new(date, t).assume_offset(off).into())
}

/// `selection` の項目（ファイルまたはフォルダ。フォルダは中身ごと）を `dest` 直下に展開する。
/// `selection` が空なら全部。ZIP内の階層は、選択した項目の位置から下だけが残る。
pub fn extract(zip_path: &Path, selection: &[String], dest: &Path) -> Result<ExtractReport, Error> {
    let file = File::open(zip_path).map_err(io_err(zip_path))?;
    let mut ar = ZipArchive::new(BufReader::new(file))?;
    let entries = read_entries(&mut ar)?;
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
    // 最上位の名前 → 衝突回避後の名前
    let mut top_map: Vec<(String, String)> = Vec::new();

    for e in &entries {
        let Some(rel) = rel_of(&e.path) else { continue };
        let skip = |report: &mut ExtractReport, reason: &str| {
            report.skipped.push(Skipped { path: e.path.clone(), reason: reason.to_string() });
        };
        if !e.safe {
            skip(&mut report, "不正なパス（..を含む）");
            continue;
        }
        if e.symlink {
            skip(&mut report, "シンボリックリンク");
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
                let to = unique_name(dest, &top);
                top_map.push((top.clone(), to.clone()));
                report.items.push(dest.join(&to));
                to
            }
        };
        comps[0] = mapped;
        let mut target = dest.to_path_buf();
        comps.iter().for_each(|c| target.push(c));

        if e.is_dir {
            fs::create_dir_all(&target).map_err(io_err(&target))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        // 同じパスがZIP内に複数ある場合も上書きしない
        let target = if target.exists() {
            let dir = target.parent().unwrap_or(dest).to_path_buf();
            let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            dir.join(unique_name(&dir, &name))
        } else {
            target
        };

        let mut zf = match ar.by_index(e.index) {
            Ok(f) => f,
            Err(err) => {
                skip(&mut report, &format!("読み出せません: {err}"));
                continue;
            }
        };
        let mut out = File::create(&target).map_err(io_err(&target))?;
        if let Err(err) = io::copy(&mut zf, &mut out) {
            drop(out);
            let _ = fs::remove_file(&target);
            skip(&mut report, &format!("展開に失敗: {err}"));
            continue;
        }
        if let Some(t) = zf.last_modified().and_then(to_system_time) {
            let _ = out.set_modified(t);
        }
        report.files += 1;
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
