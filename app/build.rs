// Windowsビルドのときだけ、exeにアイコンとバージョン情報を埋め込む。
// （右クリックメニューの Icon は "exe,0" を指すので、ここで埋め込んだアイコンが出る）
fn main() {
    println!("cargo:rerun-if-changed=assets/tote.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/tote.ico");
        res.set("ProductName", "Tote");
        res.set("FileDescription", "Tote - ZIP archiver");
        if let Err(e) = res.compile() {
            // アイコン埋め込みに失敗してもビルド自体は通す（rc.exe が無い環境など）
            println!("cargo:warning=アイコンの埋め込みに失敗しました: {e}");
        }
    }
}
