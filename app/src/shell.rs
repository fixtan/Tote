//! Windows のシェル連携の登録/解除（すべて HKCU / ユーザーフォルダ内。管理者権限は不要）。
//!
//! 登録するもの:
//!   - フォルダの右クリック:   HKCU\Software\Classes\Directory\shell\Tote
//!   - ファイルの右クリック:   HKCU\Software\Classes\*\shell\Tote
//!   - 「送る」メニュー:       %APPDATA%\Microsoft\Windows\SendTo\ToteでZIPに圧縮.lnk
//!
//! 注意: Win11 では前の2つは「その他のオプションを確認」(旧メニュー) 側に出る。
//! 作業名 "zipr" 時代の登録が残っていれば、登録・解除のどちらでも一緒に掃除する。

use std::path::Path;

#[cfg(windows)]
const MENU_LABEL: &str = "ToteでZIPに圧縮";
#[cfg(windows)]
const VERB_KEY: &str = "Tote";
#[cfg(windows)]
const SENDTO_NAME: &str = "ToteでZIPに圧縮.lnk";

#[cfg(windows)]
const LEGACY_VERB_KEY: &str = "zipr";
#[cfg(windows)]
const LEGACY_SENDTO_NAME: &str = "ZIPに圧縮 (zipr).lnk";

#[cfg(windows)]
const VERB_ROOTS: [&str; 2] = [r"Software\Classes\Directory\shell", r"Software\Classes\*\shell"];

#[cfg(windows)]
fn remove_verb(verb: &str) -> Result<(), String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for root in VERB_ROOTS {
        match hkcu.delete_subkey_all(format!(r"{root}\{verb}")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("レジストリ削除に失敗: {e}")),
        }
    }
    Ok(())
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
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    // 旧名(zipr)の登録が残っていたら先に消す
    remove_verb(LEGACY_VERB_KEY)?;
    remove_lnk(LEGACY_SENDTO_NAME)?;

    let exe_s = exe.to_string_lossy();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    for root in VERB_ROOTS {
        let (key, _) = hkcu
            .create_subkey(format!(r"{root}\{VERB_KEY}"))
            .map_err(|e| format!("レジストリ書き込みに失敗: {e}"))?;
        key.set_value("", &MENU_LABEL).map_err(|e| e.to_string())?;
        key.set_value("Icon", &format!("\"{exe_s}\",0")).map_err(|e| e.to_string())?;
        let (cmd, _) = key.create_subkey("command").map_err(|e| e.to_string())?;
        cmd.set_value("", &format!("\"{exe_s}\" \"%1\"")).map_err(|e| e.to_string())?;
    }

    let lnk = sendto_dir()?.join(SENDTO_NAME);
    let link = mslnk::ShellLink::new(exe).map_err(|e| format!("ショートカット作成に失敗: {e}"))?;
    link.create_lnk(&lnk).map_err(|e| format!("ショートカット作成に失敗: {e}"))?;
    Ok(())
}

#[cfg(windows)]
pub fn uninstall() -> Result<(), String> {
    remove_verb(VERB_KEY)?;
    remove_verb(LEGACY_VERB_KEY)?;
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
