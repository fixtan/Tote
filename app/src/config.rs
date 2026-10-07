//! 設定の保存と読み込み（`%APPDATA%\Tote\config.json`）。
//!
//! 壊れていたり、項目が足りなくても、足りない分は既定値で動く（設定のせいで起動できない事故を避ける）。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// 圧縮する形式の ID（`CompressFormat::id`）
    pub compress_format: String,
    /// 圧縮レベルのプリセット ID（`CompressFormat::presets`）
    pub compress_level: String,
    /// 固体圧縮（7z）。作成ダイアログで最後に選んだ値を覚える
    pub compress_solid: bool,
    /// 実行形式・スクリプトを開く前に確認する
    pub confirm_risky: bool,
    /// 圧縮完了後にエクスプローラーで選択表示する: "never" | "slow"（時間がかかったときだけ） | "always"
    pub reveal: String,
    /// 「すべて展開」の展開先: "besideArchive"（書庫と同じ場所の「書庫名」フォルダ） | "ask"（毎回選ぶ）
    pub extract_dest: String,
    /// ZIPへ追加するときの書き込み方: "safe"（一時ファイルへ書いて差し替え） | "fast"（その場で追記。途中で止まると壊れる恐れ）
    /// ※追加機能は今後のバージョンで有効になる。設定だけ先に保存できる。
    pub append_mode: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            compress_format: "zip".into(),
            compress_level: "normal".into(),
            compress_solid: true,
            confirm_risky: true,
            reveal: "slow".into(),
            extract_dest: "besideArchive".into(),
            append_mode: "safe".into(),
        }
    }
}

/// 3秒以上かかったら「時間がかかった」とみなす
pub const SLOW_SECS: f32 = 3.0;

impl Config {
    /// 値が想定外なら既定値に直す（手編集や将来の版との食い違い対策）。
    pub fn sanitized(mut self) -> Config {
        let d = Config::default();
        if tote_core::CompressFormat::from_id(&self.compress_format).is_none() {
            self.compress_format = d.compress_format.clone();
        }
        let fmt = tote_core::CompressFormat::from_id(&self.compress_format).unwrap_or_default();
        if !fmt.presets().iter().any(|p| p.id == self.compress_level) {
            self.compress_level = d.compress_level.clone();
        }
        if !["never", "slow", "always"].contains(&self.reveal.as_str()) {
            self.reveal = d.reveal;
        }
        if !["besideArchive", "ask"].contains(&self.extract_dest.as_str()) {
            self.extract_dest = d.extract_dest;
        }
        if !["safe", "fast"].contains(&self.append_mode.as_str()) {
            self.append_mode = d.append_mode;
        }
        self
    }

    /// 圧縮に使うオプション
    pub fn compress_options(&self) -> tote_core::Options {
        let format = tote_core::CompressFormat::from_id(&self.compress_format).unwrap_or_default();
        tote_core::Options { level: format.level_for(&self.compress_level), format, solid: self.compress_solid, ..Default::default() }
    }

    /// 圧縮が終わったあとにエクスプローラーで見せるか
    pub fn should_reveal(&self, elapsed_secs: f32) -> bool {
        match self.reveal.as_str() {
            "never" => false,
            "always" => true,
            _ => elapsed_secs >= SLOW_SECS,
        }
    }
}

pub fn config_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join(if cfg!(windows) { "Tote" } else { "tote" }).join("config.json"))
}

pub fn load() -> Config {
    load_from(config_path())
}

pub fn load_from(path: Option<PathBuf>) -> Config {
    path.and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Config>(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
        .sanitized()
}

#[cfg_attr(not(feature = "viewer"), allow(dead_code))]
pub fn save(cfg: &Config) -> Result<(), String> {
    save_to(config_path(), cfg)
}

pub fn save_to(path: Option<PathBuf>, cfg: &Config) -> Result<(), String> {
    let path = path.ok_or("設定の保存先が決められません")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("設定フォルダを作れません: {e}"))?;
    }
    let json = serde_json::to_string_pretty(&cfg.clone().sanitized()).map_err(|e| e.to_string())?;
    // 書き込み途中で止まっても設定が壊れないよう、一時ファイルに書いてから差し替える
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("設定を保存できません: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("設定を保存できません: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_broken_file_gives_defaults() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(load_from(Some(t.path().join("none.json"))), Config::default());
        let p = t.path().join("c.json");
        fs::write(&p, "{ this is not json").unwrap();
        assert_eq!(load_from(Some(p)), Config::default());
    }

    #[test]
    fn partial_file_fills_in_defaults_and_roundtrips() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("c.json");
        fs::write(&p, "\u{feff}{\"confirmRisky\": false, \"reveal\": \"always\"}").unwrap();
        let c = load_from(Some(p.clone()));
        assert!(!c.confirm_risky);
        assert_eq!(c.reveal, "always");
        assert_eq!(c.compress_level, "normal");

        let mut c2 = c.clone();
        c2.compress_level = "best".into();
        save_to(Some(p.clone()), &c2).unwrap();
        assert_eq!(load_from(Some(p)), c2);
    }

    #[test]
    fn unknown_values_are_reset() {
        let c = Config { compress_format: "rar".into(), compress_level: "ultra".into(), reveal: "x".into(), extract_dest: "y".into(), append_mode: "z".into(), confirm_risky: true, compress_solid: true }
            .sanitized();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn options_and_reveal_follow_settings() {
        let mut c = Config::default();
        assert_eq!(c.compress_options().level, None);
        c.compress_level = "store".into();
        assert_eq!(c.compress_options().level, Some(0));
        c.reveal = "never".into();
        assert!(!c.should_reveal(100.0));
        c.reveal = "always".into();
        assert!(c.should_reveal(0.0));
        c.reveal = "slow".into();
        assert!(!c.should_reveal(1.0) && c.should_reveal(3.0));
    }
}
