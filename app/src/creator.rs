//! 「書庫を作成…」ダイアログのコマンド（Tauri）。`tote.exe --create <パス>...` で窓が開く（起動は gui.rs）。
//! 右クリックの直圧縮（ダイアログなし）はこのモジュールを通らない。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;
use tauri_plugin_dialog::DialogExt;
use tote_core::{CompressFormat, Options};

use crate::config;

pub struct Creator {
    inputs: Vec<PathBuf>,
}

impl Creator {
    pub fn new(inputs: Vec<PathBuf>) -> Self {
        Creator { inputs }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetInfo {
    id: &'static str,
    label: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatDesc {
    id: &'static str,
    label: &'static str,
    extension: &'static str,
    presets: Vec<PresetInfo>,
    supports_password: bool,
    supports_solid: bool,
    supports_name_encryption: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Init {
    formats: Vec<FormatDesc>,
    format: &'static str,
    level: String,
    solid: bool,
    dir: String,
    stem: String,
    count: usize,
    names: Vec<String>,
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

/// 出力先フォルダと名前（拡張子なし）の初期値。入力がバラバラの場所にあるときなどは、最初の入力の隣・"archive"。
fn default_target(inputs: &[PathBuf], format: CompressFormat) -> (String, String) {
    if let Ok(out) = tote_core::suggest_output(inputs, format) {
        let dir = out.parent().map(|p| p.display().to_string()).unwrap_or_default();
        let ext = format!(".{}", format.extension());
        let file = name_of(&out);
        let stem = file.strip_suffix(&ext).unwrap_or(&file).to_string();
        return (dir, stem);
    }
    let dir = inputs.first().and_then(|p| p.parent()).filter(|p| !p.as_os_str().is_empty()).map(|p| p.display().to_string()).unwrap_or_default();
    (dir, "archive".to_string())
}

#[tauri::command]
pub fn create_init(state: State<'_, Creator>) -> Init {
    let cfg = config::load();
    let format = CompressFormat::from_id(&cfg.compress_format).unwrap_or_default();
    let (dir, stem) = default_target(&state.inputs, format);
    Init {
        formats: CompressFormat::ALL
            .iter()
            .map(|f| FormatDesc {
                id: f.id(),
                label: f.label(),
                extension: f.extension(),
                presets: f.presets().iter().map(|p| PresetInfo { id: p.id, label: p.label }).collect(),
                supports_password: f.supports_password(),
                supports_solid: f.supports_solid(),
                supports_name_encryption: f.supports_name_encryption(),
            })
            .collect(),
        format: format.id(),
        level: cfg.compress_level,
        solid: cfg.compress_solid,
        dir,
        stem,
        count: state.inputs.len(),
        names: state.inputs.iter().take(3).map(|p| name_of(p)).collect(),
    }
}

/// 保存先のフォルダを選ぶ。キャンセルなら None。
#[tauri::command]
pub async fn create_pick_dir(app: tauri::AppHandle, current: String) -> Result<Option<String>, String> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut d = app.dialog().file().set_title("保存先のフォルダを選択");
        if !current.is_empty() && Path::new(&current).is_dir() {
            d = d.set_directory(&current);
        }
        d.blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    match picked {
        Some(p) => Ok(Some(p.into_path().map_err(|e| e.to_string())?.display().to_string())),
        None => Ok(None),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    format: String,
    /// 圧縮レベルのプリセット ID（レベルの無い形式では空）
    level: String,
    password: Option<String>,
    solid: bool,
    encrypt_names: bool,
    dir: String,
    stem: String,
    overwrite: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum Created {
    Ok { output: String, files: usize, dirs: usize, skipped: usize, size: u64 },
    Exists { output: String },
}

fn check_stem(stem: &str) -> Result<(), String> {
    if stem.is_empty() {
        return Err("名前を入力してください".into());
    }
    if stem.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
        return Err("名前に使えない文字があります".into());
    }
    if stem.ends_with(['.', ' ']) {
        return Err("名前の末尾に「.」や空白は付けられません".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn create_run(state: State<'_, Creator>, req: CreateRequest) -> Result<Created, String> {
    let format = CompressFormat::from_id(&req.format).ok_or("不明な形式です")?;
    let stem = req.stem.trim().to_string();
    check_stem(&stem)?;
    if req.dir.is_empty() || !Path::new(&req.dir).is_dir() {
        return Err("保存先のフォルダが見つかりません".into());
    }
    let output = Path::new(&req.dir).join(format!("{stem}.{}", format.extension()));
    let inputs = state.inputs.clone();
    if inputs.iter().any(|p| p == &output || std::fs::canonicalize(p).ok().zip(std::fs::canonicalize(&output).ok()).is_some_and(|(a, b)| a == b)) {
        return Err("入力と同じファイルは上書きできません。別の名前にしてください".into());
    }
    if output.exists() && !req.overwrite {
        return Ok(Created::Exists { output: output.display().to_string() });
    }
    let password = req.password.filter(|p| !p.is_empty());
    let opts = Options {
        output: Some(output.clone()),
        level: format.level_for(&req.level),
        format,
        password,
        solid: req.solid,
        encrypt_names: req.encrypt_names,
    };
    let summary = tauri::async_runtime::spawn_blocking(move || tote_core::create(&inputs, &opts))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    // 最後に選んだ内容を次回の初期値にする（パスワードは保存しない）
    let mut cfg = config::load();
    cfg.compress_format = format.id().to_string();
    if format.presets().iter().any(|p| p.id == req.level) {
        cfg.compress_level = req.level.clone();
    }
    if format.supports_solid() {
        cfg.compress_solid = req.solid;
    }
    let _ = config::save(&cfg);

    // 結果画面に「フォルダで表示」があるので、自動で開くのは「毎回」にしているときだけ
    if cfg.reveal == "always" {
        crate::ui::reveal(&summary.output);
    }
    let size = std::fs::metadata(&summary.output).map(|m| m.len()).unwrap_or(0);
    Ok(Created::Ok { output: summary.output.display().to_string(), files: summary.files, dirs: summary.dirs, skipped: summary.skipped.len(), size })
}

#[tauri::command]
pub fn create_reveal(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::ui::reveal(Path::new(&path));
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let dir = Path::new(&path).parent().map(Path::to_path_buf).unwrap_or_default();
        std::process::Command::new("xdg-open").arg(dir).spawn().map(|_| ()).map_err(|e| format!("開けませんでした: {e}"))
    }
}

#[tauri::command]
pub fn create_close(app: tauri::AppHandle) {
    app.exit(0);
}
