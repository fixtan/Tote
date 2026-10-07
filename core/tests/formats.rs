//! 各書庫形式の閲覧と展開（ZIP以外）。テスト用の書庫はその場で組み立てる。

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use tote_core::view::{self, ExtractReport};

fn names(info: &view::ArchiveInfo) -> Vec<String> {
    info.entries.iter().map(|e| e.path.clone()).collect()
}

fn read(p: impl AsRef<Path>) -> Vec<u8> {
    fs::read(p.as_ref()).unwrap_or_else(|e| panic!("{}: {e}", p.as_ref().display()))
}

fn extract(archive: &Path, sel: &[&str], dest: &Path) -> ExtractReport {
    let sel: Vec<String> = sel.iter().map(|s| s.to_string()).collect();
    view::extract(archive, &sel, dest).unwrap()
}

// ---------------------------------------------------------------- tar / tar.gz / gz

/// 名前を生のバイト列で書き込めるtar（`..` やShift_JISも入れられる）
fn tar_bytes(items: &[(&[u8], Option<&[u8]>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data) in items {
        let mut h = tar::Header::new_old();
        {
            let old = h.as_old_mut();
            old.name[..name.len()].copy_from_slice(name);
        }
        h.set_mode(0o644);
        h.set_mtime(1_589_718_896); // 2020-05-17 12:34:56 UTC
        match data {
            Some(d) => {
                h.set_size(d.len() as u64);
                h.set_entry_type(tar::EntryType::Regular);
            }
            None => {
                h.set_size(0);
                h.set_entry_type(tar::EntryType::Directory);
            }
        }
        h.set_cksum();
        out.extend_from_slice(h.as_bytes());
        if let Some(d) = data {
            out.extend_from_slice(d);
            out.resize(out.len() + (512 - d.len() % 512) % 512, 0);
        }
    }
    out.resize(out.len() + 1024, 0);
    out
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn sample_tar() -> Vec<u8> {
    let sjis = encoding_rs::SHIFT_JIS.encode("docs/日本語.txt").0.into_owned();
    tar_bytes(&[
        (b"./docs/", None),
        (b"./docs/guide.md", Some(b"guide")),
        (b"./readme.txt", Some(b"hello readme")),
        (b"bin/run.exe", Some(b"MZ")),
        (b"../evil.txt", Some(b"evil")),
        (&sjis, Some(b"sjis")),
    ])
}

#[test]
fn targz_lists_normalizes_and_flags() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("a.tar.gz");
    fs::write(&p, gzip(&sample_tar())).unwrap();

    let info = view::list(&p).unwrap();
    assert_eq!(info.format, "tar.gz");
    assert_eq!(names(&info), ["docs", "docs/guide.md", "readme.txt", "bin/run.exe", "../evil.txt", "docs/日本語.txt"]);
    assert!(info.entries[0].is_dir);
    assert!(info.entries[3].risky);
    assert!(!info.entries[4].safe);
    assert_eq!(info.entries[2].size, 12);
    assert!(info.total_packed > 0, "固体形式は書庫ファイルの大きさを出す");
    assert!(info.entries[2].modified.as_deref().is_some_and(|m| m.starts_with("2020-05-1")));
}

#[test]
fn targz_extracts_selection_all_and_skips_unsafe() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("a.tgz");
    fs::write(&p, gzip(&sample_tar())).unwrap();

    let d1 = t.path().join("one");
    let r = extract(&p, &["readme.txt"], &d1);
    assert_eq!(r.files, 1);
    assert_eq!(read(d1.join("readme.txt")), b"hello readme");

    let d2 = t.path().join("dir");
    let r = extract(&p, &["docs"], &d2);
    assert_eq!(r.files, 2);
    assert_eq!(read(d2.join("docs/guide.md")), b"guide");
    assert_eq!(read(d2.join("docs/日本語.txt")), b"sjis");

    let d3 = t.path().join("all");
    let r = extract(&p, &[], &d3);
    assert_eq!(r.files, 4);
    assert!(r.skipped.iter().any(|s| s.path == "../evil.txt"));
    assert!(!t.path().join("evil.txt").exists() && !d3.join("evil.txt").exists());
}

#[test]
fn plain_tar_and_misnamed_file_are_detected_by_content() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("noext.bin");
    fs::write(&p, sample_tar()).unwrap();
    assert_eq!(view::list(&p).unwrap().format, "tar");

    let q = t.path().join("weird.dat");
    fs::write(&q, gzip(&sample_tar())).unwrap();
    assert_eq!(view::list(&q).unwrap().format, "tar.gz");
}

#[test]
fn single_gz_uses_header_name_or_file_stem() {
    let t = tempfile::tempdir().unwrap();

    let mut b = flate2::GzBuilder::new().filename("note.txt").mtime(1_589_718_896).write(Vec::new(), flate2::Compression::default());
    b.write_all(b"just a note").unwrap();
    let p = t.path().join("whatever.gz");
    fs::write(&p, b.finish().unwrap()).unwrap();
    let info = view::list(&p).unwrap();
    assert_eq!((info.format.as_str(), names(&info)), ("gz", vec!["note.txt".to_string()]));
    assert_eq!(info.entries[0].size, 11);
    let d = t.path().join("o");
    extract(&p, &[], &d);
    assert_eq!(read(d.join("note.txt")), b"just a note");

    let q = t.path().join("data.bin.gz");
    fs::write(&q, gzip(b"xyz")).unwrap();
    assert_eq!(names(&view::list(&q).unwrap()), ["data.bin"]);
}

// ---------------------------------------------------------------- 7z

fn make_7z(dir: &Path) -> PathBuf {
    let src = dir.join("src");
    fs::create_dir_all(src.join("docs")).unwrap();
    fs::write(src.join("readme.txt"), "seven readme").unwrap();
    fs::write(src.join("docs/日本語.txt"), "nihongo").unwrap();
    fs::write(src.join("empty.txt"), "").unwrap();
    let out = dir.join("a.7z");
    sevenz_rust2::compress_to_path(&src, &out).unwrap();
    out
}

#[test]
fn sevenz_lists_and_extracts() {
    let t = tempfile::tempdir().unwrap();
    let p = make_7z(t.path());

    let info = view::list(&p).unwrap();
    assert_eq!(info.format, "7z");
    let mut n = names(&info);
    n.sort();
    assert!(n.contains(&"readme.txt".to_string()) && n.contains(&"docs/日本語.txt".to_string()), "{n:?}");
    assert!(info.entries.iter().all(|e| !e.encrypted));

    let d = t.path().join("sel");
    let r = extract(&p, &["docs"], &d);
    assert_eq!(r.files, 1);
    assert_eq!(read(d.join("docs/日本語.txt")), b"nihongo");

    let d = t.path().join("one");
    extract(&p, &["readme.txt"], &d);
    assert_eq!(read(d.join("readme.txt")), b"seven readme");

    let d = t.path().join("empty");
    let r = extract(&p, &["empty.txt"], &d);
    assert_eq!(r.files, 1);
    assert_eq!(read(d.join("empty.txt")), b"");

    let d = t.path().join("all");
    let r = extract(&p, &[], &d);
    assert_eq!(r.files, 3);
    assert!(r.skipped.is_empty());
}

// ---------------------------------------------------------------- CAB

#[test]
fn cab_lists_and_extracts() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("a.cab");
    {
        let mut b = cab::CabinetBuilder::new();
        let f = b.add_folder(cab::CompressionType::MsZip);
        f.add_file("readme.txt");
        f.add_file("sub\\deep.txt");
        let mut w = b.build(File::create(&p).unwrap()).unwrap();
        let mut i = 0;
        while let Some(mut fw) = w.next_file().unwrap() {
            fw.write_all([b"cab readme".as_slice(), b"deep file"][i]).unwrap();
            i += 1;
        }
        w.finish().unwrap();
    }
    let info = view::list(&p).unwrap();
    assert_eq!(info.format, "CAB");
    assert_eq!(names(&info), ["readme.txt", "sub/deep.txt"]);

    let d = t.path().join("o");
    let r = extract(&p, &["sub"], &d);
    assert_eq!(r.files, 1);
    assert_eq!(read(d.join("sub/deep.txt")), b"deep file");

    let d = t.path().join("all");
    assert_eq!(extract(&p, &[], &d).files, 2);
    assert_eq!(read(d.join("readme.txt")), b"cab readme");
}

// ---------------------------------------------------------------- LZH

fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= u16::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

/// レベル0ヘッダーの `-lh0-`（無圧縮）エントリ
fn lzh_entry(name: &[u8], data: &[u8]) -> Vec<u8> {
    let dos_time: u32 = ((2020 - 1980) << 25) | (5 << 21) | (17 << 16) | (12 << 11) | (34 << 5) | (56 / 2);
    let mut h = Vec::new();
    h.extend_from_slice(b"-lh0-");
    h.extend_from_slice(&(data.len() as u32).to_le_bytes());
    h.extend_from_slice(&(data.len() as u32).to_le_bytes());
    h.extend_from_slice(&dos_time.to_le_bytes());
    h.push(0x20); // attr
    h.push(0); // level 0
    h.push(name.len() as u8);
    h.extend_from_slice(name);
    h.extend_from_slice(&crc16(data).to_le_bytes());
    let sum = h.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    let mut out = vec![h.len() as u8, sum];
    out.extend_from_slice(&h);
    out.extend_from_slice(data);
    out
}

#[test]
fn lzh_lists_japanese_names_and_extracts() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("a.lzh");
    let mut name = encoding_rs::SHIFT_JIS.encode("資料").0.into_owned();
    name.push(0xFF);
    name.extend_from_slice(&encoding_rs::SHIFT_JIS.encode("表.txt").0); // 「表」の第2バイトは0x5C
    let mut bytes = lzh_entry(b"readme.txt", b"lzh readme");
    bytes.extend(lzh_entry(&name, b"nihongo lzh"));
    bytes.push(0);
    fs::write(&p, bytes).unwrap();

    let info = view::list(&p).unwrap();
    assert_eq!(info.format, "LZH");
    assert_eq!(names(&info), ["readme.txt", "資料/表.txt"]);
    assert_eq!(info.entries[0].modified.as_deref(), Some("2020-05-17 12:34"));

    let d = t.path().join("o");
    let r = extract(&p, &["資料"], &d);
    assert_eq!(r.files, 1);
    assert_eq!(read(d.join("資料/表.txt")), b"nihongo lzh");

    let d = t.path().join("all");
    assert_eq!(extract(&p, &[], &d).files, 2);
    assert_eq!(read(d.join("readme.txt")), b"lzh readme");
}

// ---------------------------------------------------------------- RAR

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

#[test]
fn rar_lists_and_extracts() {
    let t = tempfile::tempdir().unwrap();
    let info = view::list(&fixture("version.rar")).unwrap();
    assert_eq!((info.format.as_str(), names(&info)), ("RAR", vec!["VERSION".to_string()]));

    let d = t.path().join("o");
    let r = extract(&fixture("version.rar"), &["VERSION"], &d);
    assert_eq!(r.files, 1);
    assert_eq!(read(d.join("VERSION")).len(), 11);

    let d = t.path().join("solid");
    assert_eq!(extract(&fixture("solid.rar"), &[], &d).files, 1);

    let d = t.path().join("uni");
    let r = extract(&fixture("unicode.rar"), &[], &d);
    assert_eq!(r.files, 1);
}

#[test]
fn rar_encrypted_entries_are_skipped_not_extracted() {
    let t = tempfile::tempdir().unwrap();
    let info = view::list(&fixture("crypted.rar")).unwrap();
    assert!(info.entries[0].encrypted);
    let r = extract(&fixture("crypted.rar"), &[], &t.path().join("o"));
    assert_eq!(r.files, 0);
    assert!(r.skipped[0].reason.contains("パスワード"));
}

// ---------------------------------------------------------------- 判定

#[test]
fn unknown_files_and_iso_report_clear_errors() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("x.bin");
    fs::write(&p, b"this is not an archive").unwrap();
    assert!(view::list(&p).unwrap_err().to_string().contains("対応していない"));

    // ISO の記述子はあるが中身が空・壊れているイメージは、パニックせず分かるエラーにする
    let iso = t.path().join("disc.iso");
    let mut b = vec![0u8; 0x8100];
    b[0x8001..0x8006].copy_from_slice(b"CD001");
    fs::write(&iso, b).unwrap();
    assert!(view::list(&iso).unwrap_err().to_string().contains("ISO"));
}

// ---------------------------------------------------------------- ISO

const BIG: usize = 4999 + 1; // 2048バイトのセクタを3つまたぐ

#[test]
fn iso_joliet_lists_japanese_names_and_extracts() {
    let iso = fixture("joliet.iso");
    let info = view::list(&iso).unwrap();
    assert_eq!(info.format, "ISO");
    assert!(info.comment.contains("TOTE_TEST"));
    let n = names(&info);
    for want in ["readme.txt", "empty.txt", "docs/big_file_name_longer_than_8dot3.txt", "てすとフォルダー/あいうえお_.txt", "てすとフォルダー/sub/deep.txt"] {
        assert!(n.contains(&want.to_string()), "{want} が無い: {n:?}");
    }
    let e = info.entries.iter().find(|e| e.path == "readme.txt").unwrap();
    assert_eq!(e.size, 10);
    assert!(e.modified.is_some());

    let t = tempfile::tempdir().unwrap();
    let r = extract(&iso, &[], t.path());
    assert!(r.skipped.iter().all(|s| s.path == "link.txt"), "{:?}", r.skipped); // リンクだけは展開しない
    assert_eq!(read(t.path().join("readme.txt")), b"hello iso\n");
    assert_eq!(read(t.path().join("empty.txt")), b"");
    assert_eq!(read(t.path().join("てすとフォルダー/あいうえお_.txt")), b"nihongo\n");
    let big = read(t.path().join("docs/big_file_name_longer_than_8dot3.txt"));
    assert_eq!(big.len(), BIG);
    assert!(big[..BIG - 1].iter().all(|&b| b == b'x') && big[BIG - 1] == b'\n');
}

#[test]
fn iso_rock_ridge_only_uses_long_names_and_flags_symlinks() {
    let iso = fixture("rockridge.iso");
    let info = view::list(&iso).unwrap();
    let n = names(&info);
    assert!(n.contains(&"docs/big_file_name_longer_than_8dot3.txt".to_string()), "{n:?}");
    assert!(n.contains(&"てすとフォルダー/sub/deep.txt".to_string()), "{n:?}");
    assert!(n.contains(&"てすとフォルダー/あいうえお🤯.txt".to_string()), "Rock Ridge(UTF-8)なら絵文字も残る: {n:?}");
    assert!(info.entries.iter().find(|e| e.path == "link.txt").unwrap().symlink);
    let t = tempfile::tempdir().unwrap();
    extract(&iso, &["docs"], t.path());
    assert_eq!(read(t.path().join("docs/big_file_name_longer_than_8dot3.txt")).len(), BIG);
}

#[test]
fn iso_plain_9660_strips_version_suffix() {
    let iso = fixture("plain.iso");
    let info = view::list(&iso).unwrap();
    let mut n = names(&info);
    n.sort();
    assert_eq!(n, ["DIR", "DIR/INNER.TXT", "HELLO.TXT"]);
    let t = tempfile::tempdir().unwrap();
    extract(&iso, &["DIR/INNER.TXT"], t.path());
    assert_eq!(read(t.path().join("INNER.TXT")), b"in dir\n");
}

#[test]
fn iso_truncated_image_is_an_error_not_a_panic() {
    let t = tempfile::tempdir().unwrap();
    let cut = t.path().join("cut.iso");
    let full = read(fixture("joliet.iso"));
    fs::write(&cut, &full[..0x8000 + 4096]).unwrap(); // 記述子の直後で切れている
    assert!(view::list(&cut).is_err());
}

// ---------------------------------------------------------------- 圧縮オプション

#[test]
fn compress_level_presets_change_zip_method_and_size() {
    use tote_core::{CompressFormat, Options, create};
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("data");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.txt"), "abcdefgh".repeat(20000)).unwrap();

    let mut sizes = Vec::new();
    for id in ["store", "normal", "best"] {
        let out = t.path().join(format!("{id}.zip"));
        let f = CompressFormat::Zip;
        let opts = Options { output: Some(out.clone()), level: f.level_for(id), format: f, ..Options::default() };
        create(&[src.clone()], &opts).unwrap();
        let info = view::list(&out).unwrap();
        sizes.push((info.entries[0].method.clone(), fs::metadata(&out).unwrap().len()));
    }
    assert_eq!(sizes[0].0, "Stored");
    assert_eq!(sizes[1].0, "Deflated");
    assert!(sizes[0].1 > sizes[1].1 * 10, "{sizes:?}");
    assert!(sizes[2].1 <= sizes[1].1, "{sizes:?}");
    assert_eq!(CompressFormat::from_id("zip"), Some(CompressFormat::Zip));
    assert_eq!(CompressFormat::from_id("rar"), None);
}

#[test]
fn tar_root_entry_dot_slash_is_not_flagged_unsafe() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("r.tar.gz");
    fs::write(&p, gzip(&tar_bytes(&[(b"./", None), (b"./a.txt", Some(b"a"))]))).unwrap();
    let info = view::list(&p).unwrap();
    assert!(info.entries.iter().all(|e| e.safe), "{:?}", info.entries);
    let r = extract(&p, &[], &t.path().join("o"));
    assert_eq!((r.files, r.skipped.len()), (1, 0));
    assert_eq!(read(t.path().join("o/a.txt")), b"a");
}

/// 7-Zip系のツールが作る、フォルダ・空ファイルがデータ付きファイルの間に混ざった7z（実物）。
/// 以前は、混ざっていると後ろのファイルを取りこぼしていた。
#[test]
fn sevenz_with_interleaved_directories_and_empty_files_extracts_everything() {
    let t = tempfile::tempdir().unwrap();
    let p = fixture("interleaved.7z");
    let info = view::list(&p).unwrap();
    assert!(info.entries.iter().all(|e| e.safe), "{:?}", names(&info));

    let all = t.path().join("all");
    let r = extract(&p, &[], &all);
    assert!(r.skipped.is_empty(), "{:?}", r.skipped);
    assert_eq!(read(all.join("a/1.txt")), b"one\n");
    assert_eq!(read(all.join("a/b/2.txt")), b"two\n");
    assert_eq!(read(all.join("c.txt")), b"see\n");
    assert_eq!(read(all.join("empty.txt")), b"");
    assert_eq!(read(all.join("z/メモ.txt")), "日本語\n".as_bytes());
    assert_eq!(r.files, 5);

    // どの1ファイルだけを選んでも、正しい中身が出る
    for (sel, content) in [("a/b/2.txt", &b"two\n"[..]), ("c.txt", b"see\n"), ("z/メモ.txt", "日本語\n".as_bytes()), ("empty.txt", b"")] {
        let d = t.path().join(format!("one-{}", sel.replace('/', "_")));
        let r = extract(&p, &[sel], &d);
        assert_eq!((r.files, r.skipped.len()), (1, 0), "{sel}");
        let name = sel.rsplit('/').next().unwrap();
        assert_eq!(read(d.join(name)), content, "{sel}");
    }
    // フォルダ選択
    let d = t.path().join("dir-a");
    assert_eq!(extract(&p, &["a"], &d).files, 2);
    assert_eq!(read(d.join("a/b/2.txt")), b"two\n");
}
