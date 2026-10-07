//! 展開時のパスワード（ZIP / 7z）。作成側で付けたものを読み戻す。

use std::fs;
use std::path::{Path, PathBuf};

use tote_core::{CompressFormat, Error, Options, create, view};

fn sample(t: &Path) -> PathBuf {
    let src = t.join("データ");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a.txt"), "あいう\n".repeat(1000)).unwrap();
    fs::write(src.join("sub/b.bin"), (0..50_000u32).flat_map(|i| (i % 251).to_le_bytes()).collect::<Vec<u8>>()).unwrap();
    fs::write(src.join("空.txt"), b"").unwrap();
    src
}

fn make(t: &Path, format: CompressFormat, level: Option<i64>, names: bool) -> PathBuf {
    let src = sample(t);
    let o = Options { format, level, password: Some("ひみつ pass".into()), encrypt_names: names, ..Options::default() };
    create(&[src], &o).unwrap().output
}

fn check_ok(ar: &Path, t: &Path) {
    let out = t.join("out");
    let r = view::extract_with(ar, &[], &out, Some("ひみつ pass")).unwrap();
    assert!(r.skipped.is_empty(), "{:?}", r.skipped);
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), "あいう\n".repeat(1000).as_bytes());
    assert_eq!(fs::read(out.join("sub/b.bin")).unwrap().len(), 200_000);
    assert!(out.join("空.txt").exists());
}

fn check_wrong(ar: &Path, t: &Path) {
    let out = t.join("out2");
    let e = view::extract_with(ar, &[], &out, Some("ちがう")).unwrap_err();
    assert!(matches!(e, Error::WrongPassword), "{e:?}");
    assert!(!out.exists(), "間違ったパスワードでは何も作らない");
}

#[test]
fn zip_aes_extract_with_password() {
    let t = tempfile::tempdir().unwrap();
    let ar = make(t.path(), CompressFormat::Zip, None, false);
    // パスワード無しは従来どおりスキップ
    let r = view::extract(&ar, &[], &t.path().join("none")).unwrap();
    assert_eq!(r.files, 0);
    assert!(!r.skipped.is_empty());
    check_wrong(&ar, t.path());
    check_ok(&ar, t.path());
}

#[test]
fn sevenz_extract_with_password() {
    for level in [None, Some(0)] {
        let t = tempfile::tempdir().unwrap();
        let ar = make(t.path(), CompressFormat::SevenZ, level, false);
        check_wrong(&ar, t.path());
        check_ok(&ar, t.path());
    }
}

#[test]
fn sevenz_encrypted_names_need_password_to_list() {
    let t = tempfile::tempdir().unwrap();
    let ar = make(t.path(), CompressFormat::SevenZ, None, true);
    assert!(matches!(view::list(&ar).unwrap_err(), Error::PasswordRequired));
    assert!(matches!(view::list_with(&ar, Some("ちがう")).unwrap_err(), Error::WrongPassword));
    let info = view::list_with(&ar, Some("ひみつ pass")).unwrap();
    assert!(info.entries.iter().any(|e| e.path == "a.txt"));
    check_wrong(&ar, t.path());
    check_ok(&ar, t.path());
}

fn fixture(n: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(n)
}

/// 7-Zip で作った書庫（パスワードは pass123）を読む
#[test]
fn reads_archives_made_by_7zip() {
    for n in ["pw-aes.zip", "pw-zipcrypto.zip", "pw.7z", "pw-names.7z"] {
        let ar = fixture(n);
        let t = tempfile::tempdir().unwrap();
        let out = t.path().join("o");
        let wrong = view::extract_with(&ar, &[], &out, Some("nope"));
        assert!(matches!(wrong, Err(Error::WrongPassword) | Err(Error::PasswordRequired)), "{n}: {wrong:?}");
        let r = view::extract_with(&ar, &[], &out, Some("pass123")).unwrap();
        assert!(r.skipped.is_empty(), "{n}: {:?}", r.skipped);
        assert_eq!(fs::read_to_string(out.join("a.txt")).unwrap(), "hello secret\n", "{n}");
        assert_eq!(fs::read_to_string(out.join("日本.txt")).unwrap(), "こんにちは\n", "{n}");
    }
}
