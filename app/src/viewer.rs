//! ZIPビューア（Tauri）。`tote.exe --open <zip>` で窓が開く。
//! 圧縮（右クリック/送る）はこのモジュールを通らない＝窓もWebViewも作らず無窓のまま動く。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;
use tote_core::view::{self, ArchiveInfo, Skipped};

/// ドラッグ中に見せる小さなアイコン（OSのドラッグ画像）
const DRAG_IMAGE: &[u8] = include_bytes!("../icons/drag.png");
const SCRATCH_DRAG: &str = "tote-drag";
const SCRATCH_OPEN: &str = "tote-open";

struct Viewer {
    zip: PathBuf,
    /// ドラッグ用に展開済みの項目（選択パスのキー → 展開された最上位項目）
    drag_cache: Mutex<HashMap<String, Vec<PathBuf>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Loaded {
    path: String,
    name: String,
    info: ArchiveInfo,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Done {
    dest: String,
    files: usize,
    skipped: Vec<Skipped>,
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// 展開先の既定: ZIPと同じ場所の「ZIP名」フォルダ（既にあれば `(1)` ...）
fn default_dest(zip: &Path) -> PathBuf {
    let dir = zip.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let stem = zip.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "extracted".into());
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
async fn load_archive(state: State<'_, Viewer>) -> Result<Loaded, String> {
    let zip = state.zip.clone();
    let z = zip.clone();
    let info = tauri::async_runtime::spawn_blocking(move || view::list_zip(&z))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(Loaded { name: file_name(&zip), path: zip.display().to_string(), info })
}

#[tauri::command]
async fn extract_all(state: State<'_, Viewer>) -> Result<Done, String> {
    let zip = state.zip.clone();
    let dest = default_dest(&zip);
    run_extract(zip, Vec::new(), dest).await
}

/// 展開先をフォルダ選択ダイアログで決めて、選択した項目を展開する。キャンセルなら None。
#[tauri::command]
async fn extract_selected(
    app: tauri::AppHandle,
    state: State<'_, Viewer>,
    paths: Vec<String>,
) -> Result<Option<Done>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("展開先のフォルダを選択").blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(dest) = picked else { return Ok(None) };
    let dest = dest.into_path().map_err(|e| e.to_string())?;
    run_extract(state.zip.clone(), paths, dest).await.map(Some)
}

/// ダブルクリック: 1ファイルだけ一時フォルダに展開して、関連付けられたアプリで開く。
#[tauri::command]
async fn open_entry(state: State<'_, Viewer>, path: String) -> Result<(), String> {
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
fn open_folder(path: String) -> Result<(), String> {
    open_with_default(Path::new(&path))
}

/// ドラッグの準備: 選択項目を一時フォルダへ展開しておく（ドラッグ開始の瞬間に間に合わせるため）。
#[tauri::command]
async fn prepare_drag(state: State<'_, Viewer>, paths: Vec<String>) -> Result<(), String> {
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
async fn start_drag(window: WebviewWindow, state: State<'_, Viewer>, paths: Vec<String>) -> Result<(), String> {
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
            let r = drag::start_drag(
                &w,
                drag::DragItem::Files(items),
                drag::Image::Raw(DRAG_IMAGE.to_vec()),
                |_result, _cursor| {},
                drag::Options::default(),
            );
            if let Err(e) = r {
                eprintln!("drag failed: {e}");
            }
        })
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- 起動

pub fn run(zip: PathBuf) {
    let day = Duration::from_secs(12 * 3600);
    view::cleanup_scratch(SCRATCH_DRAG, day);
    view::cleanup_scratch(SCRATCH_OPEN, day);

    let title = format!("{} - Tote", file_name(&zip));
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Viewer { zip, drag_cache: Mutex::new(HashMap::new()) })
        .invoke_handler(tauri::generate_handler![
            load_archive,
            extract_all,
            extract_selected,
            open_entry,
            open_folder,
            prepare_drag,
            start_drag
        ])
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title(title.clone())
                .inner_size(980.0, 640.0)
                .min_inner_size(560.0, 360.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!());

    if let Err(e) = result {
        crate::ui::error(&format!("ビューアを起動できませんでした: {e}"));
    }
}
