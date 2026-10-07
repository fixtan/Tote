// RAR 展開用の unrar（C++）は、Windows のトークン/乱数 API（OpenProcessToken, CryptGenRandom など）を使う。
// 画面つきの exe は他のクレートが advapi32 をリンクするが、tote-core 単体のテスト exe にはそれが無く、
// リンクエラーになるので、ここで明示する。
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=advapi32");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
