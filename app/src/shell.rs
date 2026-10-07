//! Windows のシェル連携の登録・解除・状態確認（すべて HKCU / ユーザーフォルダ内。管理者権限は不要）。
//!
//! 登録する項目（設定画面でひとつずつ ON/OFF できる）:
//!   compress   … ファイル/フォルダの右クリック「ToteでZIPに圧縮」
//!   sendto     … 「送る」メニュー
//!   open:<形式> … その形式の右クリック「Toteで開く」と、「プログラムから開く」の候補
//!
//! 注意:
//!   - Win11 では右クリック項目は「その他のオプションを確認」(旧メニュー) 側に出る。
//!   - Windows は既定のアプリをプログラムから切り替えさせない。ダブルクリックで開きたい形式は、
//!     設定画面の「既定のアプリを開く」から Tote を選ぶ（または 右クリック → プログラムから開く → 常に使う）。
//!   - 作業名 "zipr" 時代の登録が残っていれば、登録・解除のどちらでも一緒に掃除する。
//!
//! レジストリ/ショートカットの読み書きは `Platform` に分けてあり、ロジックは Linux 上のメモリ実装でテストできる。

use std::path::Path;

use serde::Serialize;

/// 設定画面に出す「開く」対象の形式。(id, 表示名, 拡張子)
pub const OPEN_GROUPS: &[(&str, &str, &[&str])] = &[
    ("zip", "ZIP", &["zip"]),
    ("7z", "7z", &["7z"]),
    ("rar", "RAR", &["rar"]),
    ("targz", "tar.gz / gz", &["gz", "tgz", "tar"]),
    ("cab", "CAB", &["cab"]),
    ("lzh", "LZH / LHA", &["lzh", "lha"]),
    ("iso", "ISO", &["iso"]),
];

const MENU_LABEL: &str = "ToteでZIPに圧縮";
const VERB_KEY: &str = "Tote";
const SENDTO_NAME: &str = "ToteでZIPに圧縮.lnk";
const LEGACY_VERB_KEY: &str = "zipr";
const LEGACY_SENDTO_NAME: &str = "ZIPに圧縮 (zipr).lnk";
const VERB_ROOTS: [&str; 2] = [r"Software\Classes\Directory\shell", r"Software\Classes\*\shell"];
const OPEN_LABEL: &str = "Toteで開く";

/// レジストリとショートカットの操作（Windows 実装と、テスト用のメモリ実装がある）
pub trait Platform {
    fn supported(&self) -> bool;
    fn reg_get(&self, key: &str, name: &str) -> Option<String>;
    fn reg_set(&mut self, key: &str, name: &str, value: &str) -> Result<(), String>;
    fn reg_delete_tree(&mut self, key: &str) -> Result<(), String>;
    fn reg_delete_value(&mut self, key: &str, name: &str) -> Result<(), String>;
    fn sendto_exists(&self, name: &str) -> bool;
    fn sendto_create(&mut self, exe: &Path, name: &str) -> Result<(), String>;
    fn sendto_remove(&mut self, name: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ItemStatus {
    pub id: String,
    pub label: String,
    pub hint: String,
    /// "on"（登録済み） | "off"（未登録） | "stale"（古い場所のexeを指している／一部だけ登録） | "na"（この環境では使えない）
    pub state: &'static str,
}

fn compress_command(exe: &str) -> String {
    format!("\"{exe}\" \"%1\"")
}

fn open_command(exe: &str) -> String {
    format!("\"{exe}\" --open \"%1\"")
}

fn open_verb_key(ext: &str) -> String {
    format!(r"Software\Classes\SystemFileAssociations\.{ext}\shell\Tote.open")
}

fn progid(group: &str) -> String {
    format!("Tote.{group}")
}

fn progid_key(group: &str) -> String {
    format!(r"Software\Classes\{}", progid(group))
}

fn open_with_key(ext: &str) -> String {
    format!(r"Software\Classes\.{ext}\OpenWithProgids")
}

fn group_of(id: &str) -> Option<&'static (&'static str, &'static str, &'static [&'static str])> {
    let g = id.strip_prefix("open:")?;
    OPEN_GROUPS.iter().find(|(gid, _, _)| *gid == g)
}

/// 画面に出す全項目の (id, 表示名, 説明)
pub fn item_defs() -> Vec<(String, String, String)> {
    let mut v = vec![
        (
            "compress".to_string(),
            "右クリックメニュー「ToteでZIPに圧縮」".to_string(),
            "ファイル・フォルダの右クリックに追加（Win11は「その他のオプションを確認」の中）".to_string(),
        ),
        (
            "sendto".to_string(),
            "「送る」メニュー".to_string(),
            "右クリック →「送る」。15個を超える選択はこちらが確実".to_string(),
        ),
    ];
    for (id, label, exts) in OPEN_GROUPS {
        let list = exts.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ");
        let extra = match *id {
            "rar" => "。RARは展開のみ（作成はできません）",
            "iso" => "。「既定のアプリ」にすると、ダブルクリックでのマウントができなくなります（右クリックの候補に足すだけなら影響なし）",
            _ => "",
        };
        v.push((
            format!("open:{id}"),
            format!("{label} を開く"),
            format!("右クリック「{OPEN_LABEL}」と「プログラムから開く」の候補に追加（{list}）{extra}"),
        ));
    }
    v
}

/// (存在するか, 今のexeを指しているか)
fn probe_cmd<P: Platform>(p: &P, key: &str, want: &str) -> (bool, bool) {
    match p.reg_get(&format!(r"{key}\command"), "") {
        Some(cur) => (true, cur.eq_ignore_ascii_case(want)),
        None => (false, false),
    }
}

fn state_of<P: Platform>(p: &P, exe: &str, id: &str) -> &'static str {
    if !p.supported() {
        return "na";
    }
    let mut checks: Vec<(bool, bool)> = Vec::new();
    if id == "compress" {
        let want = compress_command(exe);
        for root in VERB_ROOTS {
            checks.push(probe_cmd(p, &format!(r"{root}\{VERB_KEY}"), &want));
        }
    } else if id == "sendto" {
        let e = p.sendto_exists(SENDTO_NAME);
        checks.push((e, e)); // ショートカットの中身は読めないので、存在だけを見る
    } else if let Some((gid, _, exts)) = group_of(id) {
        let want = open_command(exe);
        checks.push(probe_cmd(p, &format!(r"{}\shell\open", progid_key(gid)), &want));
        for ext in *exts {
            checks.push(probe_cmd(p, &open_verb_key(ext), &want));
            let linked = p.reg_get(&open_with_key(ext), &progid(gid)).is_some();
            checks.push((linked, linked));
        }
    } else {
        return "na";
    }
    if checks.iter().all(|(_, ok)| *ok) {
        "on"
    } else if checks.iter().all(|(exists, _)| !*exists) {
        "off"
    } else {
        "stale"
    }
}

pub fn status<P: Platform>(p: &P, exe: &Path) -> Vec<ItemStatus> {
    let exe_s = exe.to_string_lossy();
    item_defs()
        .into_iter()
        .map(|(id, label, hint)| {
            let state = state_of(p, &exe_s, &id);
            ItemStatus { id, label, hint, state }
        })
        .collect()
}

fn write_verb<P: Platform>(p: &mut P, key: &str, label: &str, exe: &str, command: &str) -> Result<(), String> {
    p.reg_set(key, "", label)?;
    p.reg_set(key, "Icon", &format!("\"{exe}\",0"))?;
    p.reg_set(&format!(r"{key}\command"), "", command)
}

fn register<P: Platform>(p: &mut P, exe: &Path, id: &str) -> Result<(), String> {
    let exe_s = exe.to_string_lossy().into_owned();
    if id == "compress" {
        for root in VERB_ROOTS {
            write_verb(p, &format!(r"{root}\{VERB_KEY}"), MENU_LABEL, &exe_s, &compress_command(&exe_s))?;
        }
        Ok(())
    } else if id == "sendto" {
        p.sendto_create(exe, SENDTO_NAME)
    } else if let Some((gid, label, exts)) = group_of(id) {
        let cmd = open_command(&exe_s);
        let pk = progid_key(gid);
        p.reg_set(&pk, "", &format!("{label} (Tote)"))?;
        p.reg_set(&format!(r"{pk}\DefaultIcon"), "", &format!("\"{exe_s}\",0"))?;
        p.reg_set(&format!(r"{pk}\shell\open\command"), "", &cmd)?;
        for ext in *exts {
            write_verb(p, &open_verb_key(ext), OPEN_LABEL, &exe_s, &cmd)?;
            p.reg_set(&open_with_key(ext), &progid(gid), "")?;
        }
        Ok(())
    } else {
        Err(format!("不明な項目: {id}"))
    }
}

fn unregister<P: Platform>(p: &mut P, id: &str) -> Result<(), String> {
    if id == "compress" {
        for root in VERB_ROOTS {
            p.reg_delete_tree(&format!(r"{root}\{VERB_KEY}"))?;
        }
        Ok(())
    } else if id == "sendto" {
        p.sendto_remove(SENDTO_NAME)
    } else if let Some((gid, _, exts)) = group_of(id) {
        for ext in *exts {
            p.reg_delete_tree(&open_verb_key(ext))?;
            p.reg_delete_value(&open_with_key(ext), &progid(gid))?;
        }
        p.reg_delete_tree(&progid_key(gid))
    } else {
        Err(format!("不明な項目: {id}"))
    }
}

/// 作業名 "zipr" 時代の登録を消す。
fn cleanup_legacy<P: Platform>(p: &mut P) -> Result<(), String> {
    for root in VERB_ROOTS {
        p.reg_delete_tree(&format!(r"{root}\{LEGACY_VERB_KEY}"))?;
    }
    p.sendto_remove(LEGACY_SENDTO_NAME)
}

/// 指定した項目を ON/OFF にする。他の項目には触れない。失敗した項目があっても残りは続け、最後にまとめて返す。
pub fn apply<P: Platform>(p: &mut P, exe: &Path, wants: &[(String, bool)]) -> Result<(), String> {
    if !p.supported() {
        return Err("右クリック登録は Windows 専用です".into());
    }
    let mut errors = Vec::new();
    if let Err(e) = cleanup_legacy(p) {
        errors.push(e);
    }
    for (id, on) in wants {
        let r = if *on { register(p, exe, id) } else { unregister(p, id) };
        if let Err(e) = r {
            errors.push(format!("{id}: {e}"));
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
}

/// 登録済み（または一部だけ残っている）項目を、今のexeで登録し直す。未登録の項目は増やさない。
pub fn repair<P: Platform>(p: &mut P, exe: &Path) -> Result<usize, String> {
    let wants: Vec<(String, bool)> =
        status(p, exe).into_iter().filter(|s| s.state == "stale" || s.state == "on").map(|s| (s.id, true)).collect();
    let n = wants.len();
    apply(p, exe, &wants)?;
    Ok(n)
}

/// 初期セット（`--install` 用）: 圧縮・送る・ZIPを開く
pub fn install_defaults<P: Platform>(p: &mut P, exe: &Path) -> Result<(), String> {
    let wants: Vec<(String, bool)> =
        ["compress", "sendto", "open:zip"].iter().map(|s| (s.to_string(), true)).collect();
    apply(p, exe, &wants)
}

/// すべて解除する。
pub fn uninstall_all<P: Platform>(p: &mut P) -> Result<(), String> {
    if !p.supported() {
        return Err("右クリック登録は Windows 専用です".into());
    }
    let wants: Vec<(String, bool)> = item_defs().into_iter().map(|(id, _, _)| (id, false)).collect();
    apply(p, Path::new(""), &wants)
}

// ---------------------------------------------------------------- Windows 実装

#[cfg(windows)]
pub struct System;

#[cfg(windows)]
mod win {
    use super::{Platform, System};
    use std::path::{Path, PathBuf};

    fn hkcu() -> winreg::RegKey {
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
    }

    fn reg_err(e: std::io::Error) -> String {
        format!("レジストリ書き込みに失敗: {e}")
    }

    fn sendto_dir() -> Result<PathBuf, String> {
        std::env::var_os("APPDATA")
            .map(|a| PathBuf::from(a).join(r"Microsoft\Windows\SendTo"))
            .ok_or_else(|| "APPDATA が取得できません".to_string())
    }

    impl Platform for System {
        fn supported(&self) -> bool {
            true
        }

        fn reg_get(&self, key: &str, name: &str) -> Option<String> {
            hkcu().open_subkey(key).ok()?.get_value::<String, _>(name).ok()
        }

        fn reg_set(&mut self, key: &str, name: &str, value: &str) -> Result<(), String> {
            let (k, _) = hkcu().create_subkey(key).map_err(reg_err)?;
            k.set_value(name, &value).map_err(reg_err)
        }

        fn reg_delete_tree(&mut self, key: &str) -> Result<(), String> {
            match hkcu().delete_subkey_all(key) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("レジストリ削除に失敗: {e}")),
            }
        }

        fn reg_delete_value(&mut self, key: &str, name: &str) -> Result<(), String> {
            let k = match hkcu().open_subkey_with_flags(key, winreg::enums::KEY_SET_VALUE) {
                Ok(k) => k,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(format!("レジストリ削除に失敗: {e}")),
            };
            match k.delete_value(name) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("レジストリ削除に失敗: {e}")),
            }
        }

        fn sendto_exists(&self, name: &str) -> bool {
            sendto_dir().is_ok_and(|d| d.join(name).exists())
        }

        fn sendto_create(&mut self, exe: &Path, name: &str) -> Result<(), String> {
            let lnk = sendto_dir()?.join(name);
            let link = mslnk::ShellLink::new(exe).map_err(|e| format!("ショートカット作成に失敗: {e}"))?;
            link.create_lnk(&lnk).map_err(|e| format!("ショートカット作成に失敗: {e}"))
        }

        fn sendto_remove(&mut self, name: &str) -> Result<(), String> {
            match std::fs::remove_file(sendto_dir()?.join(name)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("ショートカット削除に失敗: {e}")),
            }
        }
    }
}

// ---------------------------------------------------------------- Windows以外（登録は使えない）

#[cfg(not(windows))]
pub struct System;

#[cfg(not(windows))]
impl Platform for System {
    fn supported(&self) -> bool {
        false
    }
    fn reg_get(&self, _: &str, _: &str) -> Option<String> {
        None
    }
    fn reg_set(&mut self, _: &str, _: &str, _: &str) -> Result<(), String> {
        Err("右クリック登録は Windows 専用です".into())
    }
    fn reg_delete_tree(&mut self, _: &str) -> Result<(), String> {
        Err("右クリック登録は Windows 専用です".into())
    }
    fn reg_delete_value(&mut self, _: &str, _: &str) -> Result<(), String> {
        Err("右クリック登録は Windows 専用です".into())
    }
    fn sendto_exists(&self, _: &str) -> bool {
        false
    }
    fn sendto_create(&mut self, _: &Path, _: &str) -> Result<(), String> {
        Err("右クリック登録は Windows 専用です".into())
    }
    fn sendto_remove(&mut self, _: &str) -> Result<(), String> {
        Err("右クリック登録は Windows 専用です".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// メモリ上のレジストリ（キーは大文字小文字を区別しない）
    #[derive(Default)]
    struct Mem {
        values: HashMap<(String, String), String>,
        sendto: HashSet<String>,
    }

    fn norm(k: &str) -> String {
        k.to_ascii_lowercase()
    }

    impl Platform for Mem {
        fn supported(&self) -> bool {
            true
        }
        fn reg_get(&self, key: &str, name: &str) -> Option<String> {
            self.values.get(&(norm(key), norm(name))).cloned()
        }
        fn reg_set(&mut self, key: &str, name: &str, value: &str) -> Result<(), String> {
            self.values.insert((norm(key), norm(name)), value.to_string());
            Ok(())
        }
        fn reg_delete_tree(&mut self, key: &str) -> Result<(), String> {
            let k = norm(key);
            let sub = format!("{k}\\");
            self.values.retain(|(kk, _), _| *kk != k && !kk.starts_with(&sub));
            Ok(())
        }
        fn reg_delete_value(&mut self, key: &str, name: &str) -> Result<(), String> {
            self.values.remove(&(norm(key), norm(name)));
            Ok(())
        }
        fn sendto_exists(&self, name: &str) -> bool {
            self.sendto.contains(name)
        }
        fn sendto_create(&mut self, _: &Path, name: &str) -> Result<(), String> {
            self.sendto.insert(name.to_string());
            Ok(())
        }
        fn sendto_remove(&mut self, name: &str) -> Result<(), String> {
            self.sendto.remove(name);
            Ok(())
        }
    }

    fn st(p: &Mem, exe: &str, id: &str) -> &'static str {
        status(p, Path::new(exe)).into_iter().find(|s| s.id == id).unwrap().state
    }

    fn on(id: &str) -> (String, bool) {
        (id.to_string(), true)
    }

    fn off(id: &str) -> (String, bool) {
        (id.to_string(), false)
    }

    const EXE: &str = r"C:\Tools\Tote\tote.exe";

    #[test]
    fn fresh_machine_everything_is_off() {
        let p = Mem::default();
        let s = status(&p, Path::new(EXE));
        assert_eq!(s.len(), 2 + OPEN_GROUPS.len());
        assert!(s.iter().all(|i| i.state == "off"));
    }

    #[test]
    fn register_only_what_was_asked() {
        let mut p = Mem::default();
        apply(&mut p, Path::new(EXE), &[on("compress"), on("open:7z")]).unwrap();
        assert_eq!(st(&p, EXE, "compress"), "on");
        assert_eq!(st(&p, EXE, "open:7z"), "on");
        assert_eq!(st(&p, EXE, "sendto"), "off");
        assert_eq!(st(&p, EXE, "open:zip"), "off");
        assert_eq!(st(&p, EXE, "open:rar"), "off");
        // 圧縮メニューの中身
        let cmd = p.reg_get(r"Software\Classes\*\shell\Tote\command", "").unwrap();
        assert_eq!(cmd, format!("\"{EXE}\" \"%1\""));
        let open = p.reg_get(r"Software\Classes\SystemFileAssociations\.7z\shell\Tote.open\command", "").unwrap();
        assert_eq!(open, format!("\"{EXE}\" --open \"%1\""));
        assert!(p.reg_get(r"Software\Classes\.7z\OpenWithProgids", "Tote.7z").is_some());
    }

    #[test]
    fn group_with_several_extensions_registers_all() {
        let mut p = Mem::default();
        apply(&mut p, Path::new(EXE), &[on("open:targz")]).unwrap();
        for ext in ["gz", "tgz", "tar"] {
            assert!(p.reg_get(&format!(r"Software\Classes\SystemFileAssociations\.{ext}\shell\Tote.open\command"), "").is_some(), "{ext}");
        }
        assert_eq!(st(&p, EXE, "open:targz"), "on");
        // 一部が消えたら「一部だけ」として検出する
        p.reg_delete_tree(r"Software\Classes\SystemFileAssociations\.tgz\shell\Tote.open").unwrap();
        assert_eq!(st(&p, EXE, "open:targz"), "stale");
    }

    #[test]
    fn moved_exe_is_detected_as_stale_and_repair_fixes_only_registered_items() {
        let mut p = Mem::default();
        apply(&mut p, Path::new(EXE), &[on("compress"), on("open:zip"), on("sendto")]).unwrap();
        let moved = r"D:\New\tote.exe";
        assert_eq!(st(&p, moved, "compress"), "stale");
        assert_eq!(st(&p, moved, "open:zip"), "stale");
        assert_eq!(st(&p, moved, "open:7z"), "off");

        let n = repair(&mut p, Path::new(moved)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(st(&p, moved, "compress"), "on");
        assert_eq!(st(&p, moved, "open:zip"), "on");
        assert_eq!(st(&p, moved, "open:7z"), "off", "未登録の項目は修復で増やさない");
    }

    #[test]
    fn turning_off_removes_everything_of_that_item_only() {
        let mut p = Mem::default();
        apply(&mut p, Path::new(EXE), &[on("compress"), on("open:zip"), on("open:7z"), on("sendto")]).unwrap();
        apply(&mut p, Path::new(EXE), &[off("open:zip"), off("sendto")]).unwrap();
        assert_eq!(st(&p, EXE, "open:zip"), "off");
        assert_eq!(st(&p, EXE, "sendto"), "off");
        assert_eq!(st(&p, EXE, "open:7z"), "on");
        assert_eq!(st(&p, EXE, "compress"), "on");
        assert!(p.reg_get(r"Software\Classes\Tote.zip", "").is_none());
        assert!(p.reg_get(r"Software\Classes\.zip\OpenWithProgids", "Tote.zip").is_none());
    }

    #[test]
    fn uninstall_all_leaves_nothing_and_cleans_legacy() {
        let mut p = Mem::default();
        install_defaults(&mut p, Path::new(EXE)).unwrap();
        // 旧名 zipr の残り
        p.reg_set(r"Software\Classes\Directory\shell\zipr", "", "old").unwrap();
        p.reg_set(r"Software\Classes\Directory\shell\zipr\command", "", "old").unwrap();
        p.sendto.insert(LEGACY_SENDTO_NAME.to_string());

        uninstall_all(&mut p).unwrap();
        assert!(p.values.is_empty(), "{:?}", p.values);
        assert!(p.sendto.is_empty());
    }

    #[test]
    fn install_defaults_is_compress_sendto_and_zip() {
        let mut p = Mem::default();
        install_defaults(&mut p, Path::new(EXE)).unwrap();
        let on_ids: Vec<String> = status(&p, Path::new(EXE)).into_iter().filter(|s| s.state == "on").map(|s| s.id).collect();
        assert_eq!(on_ids, ["compress", "sendto", "open:zip"]);
    }

    #[test]
    fn unknown_item_is_an_error_but_others_still_apply() {
        let mut p = Mem::default();
        let r = apply(&mut p, Path::new(EXE), &[on("bogus"), on("compress")]);
        assert!(r.unwrap_err().contains("bogus"));
        assert_eq!(st(&p, EXE, "compress"), "on");
    }

    #[test]
    fn unsupported_platform_reports_na() {
        struct No;
        impl Platform for No {
            fn supported(&self) -> bool { false }
            fn reg_get(&self, _: &str, _: &str) -> Option<String> { None }
            fn reg_set(&mut self, _: &str, _: &str, _: &str) -> Result<(), String> { Err("x".into()) }
            fn reg_delete_tree(&mut self, _: &str) -> Result<(), String> { Err("x".into()) }
            fn reg_delete_value(&mut self, _: &str, _: &str) -> Result<(), String> { Err("x".into()) }
            fn sendto_exists(&self, _: &str) -> bool { false }
            fn sendto_create(&mut self, _: &Path, _: &str) -> Result<(), String> { Err("x".into()) }
            fn sendto_remove(&mut self, _: &str) -> Result<(), String> { Err("x".into()) }
        }
        assert!(status(&No, Path::new(EXE)).iter().all(|s| s.state == "na"));
        assert!(apply(&mut No, Path::new(EXE), &[on("compress")]).is_err());
    }
}
