//! 書庫ビューアのコマンド（Tauri）。`tote.exe --open <書庫>` で窓が開く（起動は gui.rs）。
//! 圧縮（右クリック/送る）はこのモジュールを通らない＝窓もWebViewも作らず無窓のまま動く。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tote_core::view::{self, ArchiveInfo, Skipped};

/// ドラッグ中に見せる小さなアイコン（OSのドラッグ画像）
const DRAG_IMAGE: &[u8] = include_bytes!("../icons/drag.png");
const SCRATCH_DRAG: &str = "tote-drag";
const SCRATCH_OPEN: &str = "tote-open";

pub struct Viewer {
    zip: PathBuf,
    /// ドラッグ用に展開済みの項目（選択パスのキー → 展開された最上位項目）
    drag_cache: Mutex<HashMap<String, Vec<PathBuf>>>,
}

impl Viewer {
    pub fn new(zip: PathBuf) -> Self {
        Viewer { zip, drag_cache: Mutex::new(HashMap::new()) }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loaded {
    path: String,
    name: String,
    info: ArchiveInfo,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Done {
    dest: String,
    files: usize,
    skipped: Vec<Skipped>,
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// 書庫名から拡張子を除いた名前。`a.tar.gz` は `a`（`.tar` も落とす）。
fn archive_stem(p: &Path) -> String {
    let mut stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "extracted".into());
    let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext == "gz" && stem.to_ascii_lowercase().ends_with(".tar") {
        stem.truncate(stem.len() - 4);
    }
    if stem.is_empty() { "extracted".into() } else { stem }
}

/// 展開先の既定: 書庫と同じ場所の「書庫名」フォルダ（既にあれば `(1)` ...）
fn default_dest(zip: &Path) -> PathBuf {
    let dir = zip.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let stem = archive_stem(zip);
    let first = dir.join(&stem);
    if !first.exists() {
        return first;
    }
    (1..).map(|n| dir.join(format!("{stem} ({n})"))).find(|p| !p.exists()).unwrap_or(first)
}

fn open_with_default(p: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let r = std::process::Command::new("explorer.exe").arg(p).spawn();
    #[cfg(not(windows))]
    let r = std::process::Command::new("xdg-open").arg(p).spawn();
    r.map(|_| ()).map_err(|e| format!("開けませんでした: {e}"))
}

fn drag_key(paths: &[String]) -> String {
    let mut v = paths.to_vec();
    v.sort();
    v.join("\n")
}

/// 展開が1つも成功しなかったときの理由（スキップ理由の先頭）
fn no_result_reason(skipped: &[Skipped]) -> String {
    match skipped.first() {
        Some(s) => format!("展開できませんでした: {}（{}）", s.path, s.reason),
        None => "展開できませんでした".to_string(),
    }
}

async fn run_extract(zip: PathBuf, selection: Vec<String>, dest: PathBuf) -> Result<Done, String> {
    let d = dest.clone();
    let report = tauri::async_runtime::spawn_blocking(move || view::extract(&zip, &selection, &d))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(Done { dest: dest.display().to_string(), files: report.files, skipped: report.skipped })
}

// ---------------------------------------------------------------- コマンド

#[tauri::command]
pub async fn load_archive(state: State<'_, Viewer>) -> Result<Loaded, String> {
    let zip = state.zip.clone();
    let z = zip.clone();
    let info = tauri::async_runtime::spawn_blocking(move || view::list(&z))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(Loaded { name: file_name(&zip), path: zip.display().to_string(), info })
}

/// 展開先をフォルダ選択ダイアログで決める。キャンセルなら None。
async fn pick_dest(app: tauri::AppHandle) -> Result<Option<PathBuf>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("展開先のフォルダを選択").blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    match picked {
        Some(p) => Ok(Some(p.into_path().map_err(|e| e.to_string())?)),
        None => Ok(None),
    }
}

/// すべて展開。展開先は設定（書庫と同じ場所の「書庫名」フォルダ / 毎回選ぶ）に従う。キャンセルなら None。
#[tauri::command]
pub async fn extract_all(app: tauri::AppHandle, state: State<'_, Viewer>) -> Result<Option<Done>, String> {
    let zip = state.zip.clone();
    let dest = if crate::config::load().extract_dest == "ask" {
        match pick_dest(app).await? {
            Some(d) => d,
            None => return Ok(None),
        }
    } else {
        default_dest(&zip)
    };
    run_extract(zip, Vec::new(), dest).await.map(Some)
}

/// 展開先をフォルダ選択ダイアログで決めて、選択した項目を展開する。キャンセルなら None。
#[tauri::command]
pub async fn extract_selected(
    app: tauri::AppHandle,
    state: State<'_, Viewer>,
    paths: Vec<String>,
) -> Result<Option<Done>, String> {
    let Some(dest) = pick_dest(app).await? else { return Ok(None) };
    run_extract(state.zip.clone(), paths, dest).await.map(Some)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Added {
    files: usize,
    dirs: usize,
    replaced: usize,
    skipped: usize,
    /// その場追記を選んでいたが、同名の置き換えがあったため安全な方式に切り替えた
    fell_back: bool,
}

/// 自分がドラッグ出しのために展開した一時ファイルか（自分の窓へ戻された場合に取り込まないため）
fn is_own_scratch(p: &Path) -> bool {
    let tmp = std::env::temp_dir();
    [SCRATCH_DRAG, SCRATCH_OPEN].iter().any(|s| p.starts_with(tmp.join(s)))
}

/// ドロップされたファイル・フォルダを、開いているZIPの `dest`（ZIP内のフォルダ。ルートは空）へ追加する。
/// 追加方式は設定（appendMode）に従う。対象がなければ None。
#[tauri::command]
pub async fn add_files(state: State<'_, Viewer>, paths: Vec<String>, dest: String) -> Result<Option<Added>, String> {
    let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).filter(|p| !is_own_scratch(p)).collect();
    if inputs.is_empty() {
        return Ok(None);
    }
    let zip = state.zip.clone();
    let cfg = crate::config::load();
    let mode = tote_core::append::AppendMode::from_id(&cfg.append_mode);
    let level = cfg.compress_options().level;
    let s = tauri::async_runtime::spawn_blocking(move || tote_core::append::add_to_zip(&zip, &dest, &inputs, mode, level))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(Some(Added {
        files: s.files,
        dirs: s.dirs,
        replaced: s.replaced,
        skipped: s.skipped.len(),
        fell_back: mode == tote_core::append::AppendMode::Fast && s.mode_used == tote_core::append::AppendMode::Safe,
    }))
}

/// ダブルクリック: 1ファイルだけ一時フォルダに展開して、関連付けられたアプリで開く。
#[tauri::command]
pub async fn open_entry(state: State<'_, Viewer>, path: String) -> Result<(), String> {
    let zip = state.zip.clone();
    let opened = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        let dir = view::scratch_dir(SCRATCH_OPEN).map_err(|e| e.to_string())?;
        let r = view::extract(&zip, &[path], &dir).map_err(|e| e.to_string())?;
        r.items.into_iter().next().ok_or_else(|| no_result_reason(&r.skipped))
    })
    .await
    .map_err(|e| e.to_string())??;
    open_with_default(&opened)
}

#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    open_with_default(Path::new(&path))
}

/// ドラッグの準備: 選択項目を一時フォルダへ展開しておく（ドラッグ開始の瞬間に間に合わせるため）。
#[tauri::command]
pub async fn prepare_drag(state: State<'_, Viewer>, paths: Vec<String>) -> Result<(), String> {
    let key = drag_key(&paths);
    if state.drag_cache.lock().map_err(|e| e.to_string())?.contains_key(&key) {
        return Ok(());
    }
    let zip = state.zip.clone();
    let items = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<PathBuf>, String> {
        let dir = view::scratch_dir(SCRATCH_DRAG).map_err(|e| e.to_string())?;
        let r = view::extract(&zip, &paths, &dir).map_err(|e| e.to_string())?;
        if r.items.is_empty() { Err(no_result_reason(&r.skipped)) } else { Ok(r.items) }
    })
    .await
    .map_err(|e| e.to_string())??;
    state.drag_cache.lock().map_err(|e| e.to_string())?.insert(key, items);
    Ok(())
}

/// 展開済みの項目を、OSのドラッグ＆ドロップ（エクスプローラー等へ）として開始する。
/// マウスボタンが押されている間に呼ぶこと。
#[tauri::command]
pub async fn start_drag(window: WebviewWindow, state: State<'_, Viewer>, paths: Vec<String>) -> Result<(), String> {
    let key = drag_key(&paths);
    let items = state
        .drag_cache
        .lock()
        .map_err(|e| e.to_string())?
        .get(&key)
        .cloned()
        .ok_or_else(|| "ドラッグの準備ができていません".to_string())?;
    let w = window.clone();
    window
        .run_on_main_thread(move || {
            if let Err(e) = begin_drag(&w, items) {
                eprintln!("drag failed: {e}");
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "linux"))]
fn begin_drag(w: &WebviewWindow, items: Vec<PathBuf>) -> Result<(), String> {
    drag::start_drag(w, drag::DragItem::Files(items), drag::Image::Raw(DRAG_IMAGE.to_vec()), |_r, _c| {}, drag::Options::default())
        .map_err(|e| e.to_string())
}

/// Linux (GTK) は WebviewWindow ではなく GTK のウィンドウを渡す必要がある
#[cfg(target_os = "linux")]
fn begin_drag(w: &WebviewWindow, items: Vec<PathBuf>) -> Result<(), String> {
    let gtk = w.gtk_window().map_err(|e| e.to_string())?;
    drag::start_drag(&gtk, drag::DragItem::Files(items), drag::Image::Raw(DRAG_IMAGE.to_vec()), |_r, _c| {}, drag::Options::default())
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- 起動

/// 古い一時フォルダを消す（ビューア起動時）
pub fn cleanup_old_scratch() {
    let day = Duration::from_secs(12 * 3600);
    view::cleanup_scratch(SCRATCH_DRAG, day);
    view::cleanup_scratch(SCRATCH_OPEN, day);
}

#[cfg(test)]
mod tests {
    use super::archive_stem;
    use std::path::Path;

    #[test]
    fn stem_drops_tar_before_gz() {
        assert_eq!(archive_stem(Path::new("/x/a.tar.gz")), "a");
        assert_eq!(archive_stem(Path::new("/x/a.TAR.GZ")), "a");
        assert_eq!(archive_stem(Path::new("/x/a.tgz")), "a");
        assert_eq!(archive_stem(Path::new("/x/photo.v2.zip")), "photo.v2");
        assert_eq!(archive_stem(Path::new("/x/data.bin.gz")), "data.bin");
        assert_eq!(archive_stem(Path::new("/x/.gz")), ".gz");
    }
}
