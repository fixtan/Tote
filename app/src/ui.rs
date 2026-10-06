//! GUIを持たない v0.1 の最小UI。メッセージボックスとエクスプローラー選択のみ。

use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    Yes,
    No,
    Cancel,
}

#[cfg(windows)]
mod imp {
    use super::Choice;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    fn msgbox(title: &str, text: &str, flags: u32) -> i32 {
        let (t, c) = (wide(title), wide(text));
        unsafe { MessageBoxW(std::ptr::null_mut(), c.as_ptr(), t.as_ptr(), flags) }
    }

    pub fn error(text: &str) {
        msgbox("Tote", text, MB_OK | MB_ICONERROR);
    }

    pub fn info(text: &str) {
        msgbox("Tote", text, MB_OK | MB_ICONINFORMATION);
    }

    pub fn ask(text: &str) -> Choice {
        match msgbox("Tote", text, MB_YESNOCANCEL | MB_ICONQUESTION) {
            x if x == IDYES => Choice::Yes,
            x if x == IDNO => Choice::No,
            _ => Choice::Cancel,
        }
    }

    pub fn reveal(path: &Path) {
        // explorer の /select, はクォートの扱いが特殊なので raw_arg で渡す
        let _ = std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn();
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Choice;
    use std::path::Path;

    pub fn error(text: &str) {
        eprintln!("error: {text}");
    }
    pub fn info(text: &str) {
        println!("{text}");
    }
    pub fn ask(text: &str) -> Choice {
        println!("{text}");
        Choice::Cancel
    }
    pub fn reveal(path: &Path) {
        println!("created: {}", path.display());
    }
}

pub fn error(text: &str) {
    imp::error(text)
}
pub fn info(text: &str) {
    imp::info(text)
}
pub fn ask(text: &str) -> Choice {
    imp::ask(text)
}
pub fn reveal(path: &Path) {
    imp::reveal(path)
}
