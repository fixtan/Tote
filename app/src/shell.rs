//! Windows のシェル連携の登録/解除（すべて HKCU / ユーザーフォルダ内。管理者権限は不要）。
//!
//! 登録するもの:
//!   圧縮
//!   - フォルダの右クリック:   HKCU\Software\Classes\Directory\shell\Tote
//!   - ファイルの右クリック:   HKCU\Software\Classes\*\shell\Tote
//!   - 「送る」メニュー:       %APPDATA%\Microsoft\Windows\SendTo\ToteでZIPに圧縮.lnk
//!   閲覧
//!   - .zip の右クリック:      HKCU\Software\Classes\SystemFileAssociations\.zip\shell\Tote.open
//!   - 「プログラムから開く」の候補: ProgID `Tote.zip` を .zip の OpenWithProgids に追加
//!
//! 注意:
//!   - Win11 では右クリック項目は「その他のオプションを確認」(旧メニュー) 側に出る。
//!   - Windows は既定のアプリをプログラムから切り替えさせない。ダブルクリックで開きたい場合は、
//!     .zip を右クリック →「プログラムから開く」→ Tote →「常に使う」を一度選ぶ。
//!   - 作業名 "zipr" 時代の登録が残っていれば、登録・解除のどちらでも一緒に掃除する。

use std::path::Path;

#[cfg(windows)]
mod consts {
    pub const MENU_LABEL: &str = "ToteでZIPに圧縮";
    pub const VERB_KEY: &str = "Tote";
    pub const SENDTO_NAME: &str = "ToteでZIPに圧縮.lnk";
    pub const LEGACY_VERB_KEY: &str = "zipr";
    pub const LEGACY_SENDTO_NAME: &str = "ZIPに圧縮 (zipr).lnk";
    pub const VERB_ROOTS: [&str; 2] = [r"Software\Classes\Directory\shell", r"Software\Classes\*\shell"];

    pub const OPEN_LABEL: &str = "Toteで開く";
    pub const OPEN_VERB_KEY: &str = r"Software\Classes\SystemFileAssociations\.zip\shell\Tote.open";
    pub const PROGID: &str = "Tote.zip";
    pub const PROGID_KEY: &str = r"Software\Classes\Tote.zip";
    pub const OPEN_WITH_PROGIDS: &str = r"Software\Classes\.zip\OpenWithProgids";
}

#[cfg(windows)]
use consts::*;

#[cfg(windows)]
fn hkcu() -> winreg::RegKey {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
}

#[cfg(windows)]
fn reg_err(e: std::io::Error) -> String {
    format!("レジストリ書き込みに失敗: {e}")
}

/// キーを丸ごと消す。無ければ何もしない。
#[cfg(windows)]
fn delete_tree(path: &str) -> Result<(), String> {
    match hkcu().delete_subkey_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("レジストリ削除に失敗: {e}")),
    }
}

/// `<key>` に「表示名・アイコン・コマンド」を持つ右クリック項目を作る。
#[cfg(windows)]
fn write_verb(key_path: &str, label: &str, exe: &str, command: &str) -> Result<(), String> {
    let (key, _) = hkcu().create_subkey(key_path).map_err(reg_err)?;
    key.set_value("", &label).map_err(reg_err)?;
    key.set_value("Icon", &format!("\"{exe}\",0")).map_err(reg_err)?;
    let (cmd, _) = key.create_subkey("command").map_err(reg_err)?;
    cmd.set_value("", &command).map_err(reg_err)
}

#[cfg(windows)]
fn remove_lnk(name: &str) -> Result<(), String> {
    match std::fs::remove_file(sendto_dir()?.join(name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("ショートカット削除に失敗: {e}")),
    }
}

#[cfg(windows)]
fn sendto_dir() -> Result<std::path::PathBuf, String> {
    std::env::var_os("APPDATA")
        .map(|a| std::path::PathBuf::from(a).join(r"Microsoft\Windows\SendTo"))
        .ok_or_else(|| "APPDATA が取得できません".to_string())
}

#[cfg(windows)]
pub fn install(exe: &Path) -> Result<(), String> {
    // 旧名(zipr)の登録が残っていたら先に消す
    for root in VERB_ROOTS {
        delete_tree(&format!(r"{root}\{LEGACY_VERB_KEY}"))?;
    }
    remove_lnk(LEGACY_SENDTO_NAME)?;

    let exe_s = exe.to_string_lossy();
    let compress_cmd = format!("\"{exe_s}\" \"%1\"");
    let open_cmd = format!("\"{exe_s}\" --open \"%1\"");

    // 圧縮: フォルダ / ファイルの右クリック
    for root in VERB_ROOTS {
        write_verb(&format!(r"{root}\{VERB_KEY}"), MENU_LABEL, &exe_s, &compress_cmd)?;
    }

    // 閲覧: .zip の右クリック「Toteで開く」
    write_verb(OPEN_VERB_KEY, OPEN_LABEL, &exe_s, &open_cmd)?;

    // 閲覧: 「プログラムから開く」の候補（ProgID）
    let (progid, _) = hkcu().create_subkey(PROGID_KEY).map_err(reg_err)?;
    progid.set_value("", &"ZIP アーカイブ (Tote)").map_err(reg_err)?;
    let (icon, _) = progid.create_subkey("DefaultIcon").map_err(reg_err)?;
    icon.set_value("", &format!("\"{exe_s}\",0")).map_err(reg_err)?;
    let (open, _) = progid.create_subkey(r"shell\open\command").map_err(reg_err)?;
    open.set_value("", &open_cmd).map_err(reg_err)?;
    let (with, _) = hkcu().create_subkey(OPEN_WITH_PROGIDS).map_err(reg_err)?;
    with.set_value(PROGID, &"").map_err(reg_err)?;

    // 「送る」
    let lnk = sendto_dir()?.join(SENDTO_NAME);
    let link = mslnk::ShellLink::new(exe).map_err(|e| format!("ショートカット作成に失敗: {e}"))?;
    link.create_lnk(&lnk).map_err(|e| format!("ショートカット作成に失敗: {e}"))?;
    Ok(())
}

#[cfg(windows)]
pub fn uninstall() -> Result<(), String> {
    for root in VERB_ROOTS {
        delete_tree(&format!(r"{root}\{VERB_KEY}"))?;
        delete_tree(&format!(r"{root}\{LEGACY_VERB_KEY}"))?;
    }
    delete_tree(OPEN_VERB_KEY)?;
    delete_tree(PROGID_KEY)?;

    // .zip の「プログラムから開く」候補から外す
    match hkcu().open_subkey_with_flags(OPEN_WITH_PROGIDS, winreg::enums::KEY_SET_VALUE) {
        Ok(with) => match with.delete_value(PROGID) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("レジストリ削除に失敗: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("レジストリ削除に失敗: {e}")),
    }

    remove_lnk(SENDTO_NAME)?;
    remove_lnk(LEGACY_SENDTO_NAME)
}

#[cfg(not(windows))]
pub fn install(_exe: &Path) -> Result<(), String> {
    Err("右クリック登録は Windows 専用です".into())
}

#[cfg(not(windows))]
pub fn uninstall() -> Result<(), String> {
    Err("右クリック登録は Windows 専用です".into())
}
