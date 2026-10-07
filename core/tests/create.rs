//! 書庫の作成（ZIP以外）。作ったものを Tote 自身の閲覧・展開で読み戻して確かめる。

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use tote_core::{CompressFormat, Options, create, view};

fn touch(p: &Path, body: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// 入力フォルダを作る。日本語名・空ファイル・深い階層・複数セクタ超のファイルを入れる
fn sample(t: &Path) -> PathBuf {
    let src = t.join("データ");
    touch(&src.join("readme.txt"), b"hello\n");
    touch(&src.join("空.txt"), b"");
    touch(&src.join("日本語フォルダ/あいうえお.txt"), "にほんご\n".as_bytes());
    touch(&src.join("sub/deep/x.bin"), &(0..70_000u32).flat_map(|i| i.to_le_bytes()).collect::<Vec<u8>>());
    fs::create_dir_all(src.join("empty_dir")).unwrap();
    src
}

fn names(p: &Path) -> Vec<String> {
    let mut v: Vec<_> = view::list(p).unwrap().entries.into_iter().map(|e| e.path).collect();
    v.sort();
    v
}

fn assert_same_tree(out: &Path, src: &Path) {
    for rel in ["readme.txt", "空.txt", "日本語フォルダ/あいうえお.txt", "sub/deep/x.bin"] {
        assert_eq!(fs::read(out.join(rel)).unwrap(), fs::read(src.join(rel)).unwrap(), "{rel}");
    }
    assert!(out.join("empty_dir").is_dir());
}

fn roundtrip(format: CompressFormat, tweak: impl FnOnce(&mut Options)) {
    let t = tempfile::tempdir().unwrap();
    let src = sample(t.path());
    let mut o = Options { format, ..Options::default() };
    tweak(&mut o);
    let s = create(&[src.clone()], &o).unwrap();
    assert_eq!(s.output, t.path().join(format!("データ.{}", format.extension())));
    assert_eq!((s.files, s.dirs), (4, 4));
    assert_eq!(names(&s.output), ["empty_dir", "readme.txt", "sub", "sub/deep", "sub/deep/x.bin", "日本語フォルダ", "日本語フォルダ/あいうえお.txt", "空.txt"]);
    let out = t.path().join("out");
    view::extract(&s.output, &[], &out).unwrap();
    assert_same_tree(&out, &src);
}

#[test]
fn sevenz_solid_roundtrip() {
    roundtrip(CompressFormat::SevenZ, |_| {});
}

#[test]
fn sevenz_non_solid_and_levels_roundtrip() {
    roundtrip(CompressFormat::SevenZ, |o| o.solid = false);
    roundtrip(CompressFormat::SevenZ, |o| o.level = Some(0)); // 無圧縮
    roundtrip(CompressFormat::SevenZ, |o| o.level = Some(1));
    roundtrip(CompressFormat::SevenZ, |o| o.level = Some(7));
}

#[test]
fn targz_and_tar_roundtrip() {
    roundtrip(CompressFormat::TarGz, |_| {});
    roundtrip(CompressFormat::TarGz, |o| o.level = Some(9));
    roundtrip(CompressFormat::Tar, |_| {});
}

#[test]
fn compressed_formats_are_smaller_than_input_for_text() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("t");
    touch(&src.join("a.txt"), "abcdefgh".repeat(50_000).as_bytes());
    let size = |f: CompressFormat, level: Option<i64>| {
        let s = create(&[src.clone()], &Options { format: f, level, output: Some(t.path().join(format!("o.{}", f.extension()))), ..Options::default() }).unwrap();
        fs::metadata(s.output).unwrap().len()
    };
    assert!(size(CompressFormat::SevenZ, None) < 4_000);
    assert!(size(CompressFormat::TarGz, None) < 4_000);
    assert!(size(CompressFormat::Tar, None) > 400_000);
    assert!(size(CompressFormat::SevenZ, Some(0)) > 400_000);
}

#[test]
fn zip_aes_password_encrypts_and_decrypts() {
    let t = tempfile::tempdir().unwrap();
    let src = sample(t.path());
    let s = create(&[src], &Options { password: Some("ひみつ pass".into()), ..Options::default() }).unwrap();
    let mut a = zip::ZipArchive::new(File::open(&s.output).unwrap()).unwrap();
    for i in 0..a.len() {
        let (name, enc) = {
            let f = a.by_index_raw(i).unwrap();
            (f.name().to_string(), f.encrypted())
        };
        if name.ends_with('/') {
            assert!(!enc, "フォルダ項目は暗号化しない");
        } else {
            assert!(enc, "{name} が暗号化されていない");
        }
    }
    use std::io::Read;
    let mut buf = Vec::new();
    a.by_name_decrypt("readme.txt", "ひみつ pass".as_bytes()).unwrap().read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"hello\n");
    assert!(a.by_name_decrypt("readme.txt", b"wrong").is_err() || {
        let mut b = Vec::new();
        a.by_name_decrypt("readme.txt", b"wrong").unwrap().read_to_end(&mut b).is_err()
    });
    // パスワード付きは Tote の一覧で 🔒 扱いになる
    assert!(view::list(&s.output).unwrap().entries.iter().filter(|e| !e.is_dir).all(|e| e.encrypted));
}

#[test]
fn sevenz_password_with_and_without_name_encryption() {
    use sevenz_rust2::{Archive, Password};
    let t = tempfile::tempdir().unwrap();
    let src = sample(t.path());

    // 既定: 名前は見える（7-Zipと同じ）。中身はパスワードが要る
    let s = create(&[src.clone()], &Options { format: CompressFormat::SevenZ, password: Some("pw".into()), ..Options::default() }).unwrap();
    assert!(names(&s.output).contains(&"readme.txt".to_string()));
    let out = t.path().join("dec");
    sevenz_rust2::decompress_file_with_password(&s.output, &out, Password::new("pw")).unwrap();
    assert_eq!(fs::read(out.join("readme.txt")).unwrap(), b"hello\n");
    assert_eq!(fs::read(out.join("日本語フォルダ/あいうえお.txt")).unwrap(), "にほんご\n".as_bytes());
    assert!(sevenz_rust2::decompress_file_with_password(&s.output, t.path().join("bad"), Password::new("nope")).is_err());

    // 名前も暗号化: パスワード無しでは開けない
    let s2 = create(
        &[src],
        &Options { format: CompressFormat::SevenZ, password: Some("pw".into()), encrypt_names: true, output: Some(t.path().join("n.7z")), ..Options::default() },
    )
    .unwrap();
    assert!(Archive::open(&s2.output).is_err());
    assert!(Archive::open_with_password(&s2.output, &Password::new("pw")).is_ok());
}

#[test]
fn invalid_option_combinations_are_rejected_without_leaving_files() {
    let t = tempfile::tempdir().unwrap();
    let src = sample(t.path());
    let e = create(&[src.clone()], &Options { format: CompressFormat::TarGz, password: Some("x".into()), ..Options::default() }).unwrap_err();
    assert!(e.to_string().contains("パスワード"));
    let e = create(&[src], &Options { password: Some(String::new()), ..Options::default() }).unwrap_err();
    assert!(e.to_string().contains("空"));
    assert_eq!(fs::read_dir(t.path()).unwrap().count(), 1, "出力ファイルが残っていない");
}

#[test]
fn single_files_and_multiple_inputs_go_to_root_for_every_format() {
    for f in CompressFormat::ALL {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("a.txt"), b"A");
        touch(&t.path().join("b/c.txt"), b"C");
        let s = create(&[t.path().join("a.txt"), t.path().join("b")], &Options { format: *f, ..Options::default() }).unwrap();
        assert_eq!(names(&s.output), ["a.txt", "b", "b/c.txt"], "{}", f.label());
    }
}

#[test]
fn presets_exist_per_format() {
    assert!(CompressFormat::Tar.presets().is_empty());
    assert_eq!(CompressFormat::TarGz.level_for("store"), None); // tar.gz に「無圧縮」は無い → 既定
    assert_eq!(CompressFormat::SevenZ.level_for("store"), Some(0));
    assert_eq!(CompressFormat::from_id("7z"), Some(CompressFormat::SevenZ));
    assert!(CompressFormat::SevenZ.supports_password() && !CompressFormat::TarGz.supports_password());
}
