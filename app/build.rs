// ビューア(Tauri)を含むビルドのときだけ、Tauriのビルド処理を走らせる。
// Windowsでは icons/icon.ico を exe に埋め込む（右クリックメニューの Icon も "exe,0" でこれを指す）。
fn main() {
    #[cfg(feature = "viewer")]
    tauri_build::build();
}
