//! 設定画面（Tauri）のコマンド。ビューアからも設定の読み出しと「設定を開く」を使う。

use std::path::PathBuf;

use serde::Serialize;

use crate::config::{self, Config};
use crate::shell::{self, ItemStatus, System};

fn exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tote.exe"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetInfo {
    id: &'static str,
    label: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatInfo {
    id: &'static str,
    label: &'static str,
    presets: Vec<PresetInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    config: Config,
    items: Vec<ItemStatus>,
    formats: Vec<FormatInfo>,
    exe: String,
    config_path: String,
    version: &'static str,
    /// 右クリック登録が使える環境か（Windows のみ）
    supported: bool,
}

fn formats() -> Vec<FormatInfo> {
    tote_core::CompressFormat::ALL
        .iter()
        .map(|f| FormatInfo {
            id: f.id(),
            label: f.label(),
            presets: f.presets().iter().map(|p| PresetInfo { id: p.id, label: p.label }).collect(),
        })
        .collect()
}

#[tauri::command]
pub fn get_state() -> State {
    let sys = System;
    let exe = exe();
    State {
        config: config::load(),
        items: shell::status(&sys, &exe),
        formats: formats(),
        exe: exe.display().to_string(),
        config_path: config::config_path().map(|p| p.display().to_string()).unwrap_or_default(),
        version: env!("CARGO_PKG_VERSION"),
        supported: shell::Platform::supported(&sys),
    }
}

#[tauri::command]
pub fn get_config() -> Config {
    config::load()
}

/// 保存して、補正後の設定を返す
#[tauri::command]
pub fn save_config(config: Config) -> Result<Config, String> {
    let c = config.sanitized();
    config::save(&c)?;
    Ok(c)
}

#[tauri::command]
pub fn reset_config() -> Result<Config, String> {
    let c = Config::default();
    config::save(&c)?;
    Ok(c)
}

#[tauri::command]
pub fn apply_items(wants: Vec<(String, bool)>) -> Result<Vec<ItemStatus>, String> {
    let mut sys = System;
    let exe = exe();
    let r = shell::apply(&mut sys, &exe, &wants);
    let items = shell::status(&sys, &exe);
    r.map(|()| items)
}

/// 登録済み・一部だけ残っている項目を、今のexeで登録し直す。直した項目数を返す
#[tauri::command]
pub fn repair_items() -> Result<usize, String> {
    shell::repair(&mut System, &exe())
}

#[tauri::command]
pub fn uninstall_everything() -> Result<Vec<ItemStatus>, String> {
    let mut sys = System;
    shell::uninstall_all(&mut sys)?;
    Ok(shell::status(&sys, &exe()))
}

/// Windows の「既定のアプリ」設定を開く（既定の切り替えは Windows の仕様で本人が行う）
#[tauri::command]
pub fn open_default_apps() -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg("ms-settings:defaultapps")
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("設定を開けませんでした: {e}"))
    }
    #[cfg(not(windows))]
    {
        Err("Windows 専用です".into())
    }
}

/// ビューアの⚙から。設定画面は別プロセス（引数なしの tote）として開く。
#[tauri::command]
pub fn open_settings() -> Result<(), String> {
    std::process::Command::new(exe())
        .arg("--settings")
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("設定画面を開けませんでした: {e}"))
}
