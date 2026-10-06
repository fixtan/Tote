use std::fs;
use std::path::{Path, PathBuf};

use tote_core::view::{extract, is_risky, list_zip};
use tote_core::{Options, create_zip};

// ---- 手組みのZIP（ファイル名をバイト列で自由に指定するため）

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// 2020-05-17 12:34:56 固定の stored ZIP を作る。
fn raw_zip(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    let date: u16 = ((2020 - 1980) << 9) | (5 << 5) | 17;
    let time: u16 = (12 << 11) | (34 << 5) | (56 / 2);
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend([20u8, 0, 0, 0, 0, 0]); // version, flags, method
        out.extend(time.to_le_bytes());
        out.extend(date.to_le_bytes());
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(*name);
        out.extend(*data);

        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend([20u8, 0, 20, 0, 0, 0, 0, 0]); // made by, needed, flags, method
        central.extend(time.to_le_bytes());
        central.extend(date.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend([0u8; 12]); // extra, comment, disk, int attr, ext attr(4) → 2+2+2+2+4
        central.extend(offset.to_le_bytes());
        central.extend(*name);
    }
    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend(cd_size.to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

fn write_raw(dir: &Path, name: &str, entries: &[(&[u8], &[u8])]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, raw_zip(entries)).unwrap();
    p
}

fn touch(p: &Path, body: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

// ----------------------------------------------------------------

#[test]
fn lists_without_extracting_and_flags_risky() {
    let t = tempfile::tempdir().unwrap();
    let proj = t.path().join("proj");
    touch(&proj.join("readme.txt"), "hello");
    touch(&proj.join("bin/setup.EXE"), "MZ");
    fs::create_dir_all(proj.join("empty")).unwrap();
    let zip = create_zip(&[proj], &Options::default()).unwrap().output;

    let info = list_zip(&zip).unwrap();
    let get = |p: &str| info.entries.iter().find(|e| e.path == p).unwrap_or_else(|| panic!("no entry {p}"));
    assert!(get("empty").is_dir);
    assert!(get("bin").is_dir);
    assert_eq!(get("readme.txt").size, 5);
    assert!(!get("readme.txt").risky);
    assert!(get("bin/setup.EXE").risky, "大文字拡張子でも実行形式として扱う");
    assert!(info.entries.iter().all(|e| e.safe));
    assert_eq!(info.total_size, 5 + 2);
}

#[test]
fn risky_detection_cases() {
    assert!(is_risky("a/b/run.bat"));
    assert!(is_risky("invoice.pdf.exe"));
    assert!(!is_risky("notes.txt"));
    assert!(!is_risky(".exe"), "拡張子だけの隠しファイル名は対象外");
    assert!(!is_risky("exe"));
}

#[test]
fn shift_jis_names_are_decoded() {
    let t = tempfile::tempdir().unwrap();
    let (dir_name, _, _) = encoding_rs::SHIFT_JIS.encode("資料/");
    let (file_name, _, _) = encoding_rs::SHIFT_JIS.encode("資料/日本語メモ.txt");
    let zip = write_raw(t.path(), "old.zip", &[(&dir_name, b""), (&file_name, b"abc")]);

    let info = list_zip(&zip).unwrap();
    let paths: Vec<_> = info.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["資料", "資料/日本語メモ.txt"]);
    assert!(info.entries[0].is_dir);
    assert_eq!(info.entries[1].modified.as_deref(), Some("2020-05-17 12:34"));

    let dest = t.path().join("out");
    let r = extract(&zip, &[], &dest).unwrap();
    assert_eq!(fs::read_to_string(dest.join("資料/日本語メモ.txt")).unwrap(), "abc");
    assert_eq!(r.files, 1);
}

#[test]
fn utf8_names_stay_as_is() {
    let t = tempfile::tempdir().unwrap();
    let zip = write_raw(t.path(), "u.zip", &[("日本語.txt".as_bytes(), b"x")]);
    assert_eq!(list_zip(&zip).unwrap().entries[0].path, "日本語.txt");
}

#[test]
fn backslash_separators_are_normalized() {
    let t = tempfile::tempdir().unwrap();
    let zip = write_raw(t.path(), "w.zip", &[(b"a\\b\\c.txt", b"1")]);
    assert_eq!(list_zip(&zip).unwrap().entries[0].path, "a/b/c.txt");
    let dest = t.path().join("out");
    extract(&zip, &[], &dest).unwrap();
    assert!(dest.join("a/b/c.txt").is_file());
}

#[test]
fn parent_traversal_is_never_extracted() {
    let t = tempfile::tempdir().unwrap();
    let zip = write_raw(
        t.path(),
        "evil.zip",
        &[(b"../evil.txt", b"x"), (b"ok/../../evil2.txt", b"x"), (b"good.txt", b"g")],
    );
    let info = list_zip(&zip).unwrap();
    assert_eq!(info.entries.iter().filter(|e| !e.safe).count(), 2);

    let dest = t.path().join("sub/out");
    let r = extract(&zip, &[], &dest).unwrap();
    assert_eq!(r.files, 1);
    assert_eq!(r.skipped.len(), 2);
    assert!(dest.join("good.txt").is_file());
    assert!(!t.path().join("sub/evil.txt").exists());
    assert!(!t.path().join("evil.txt").exists());
    assert!(!t.path().join("evil2.txt").exists());
}

#[test]
fn absolute_and_windows_unsafe_names_are_neutralized() {
    let t = tempfile::tempdir().unwrap();
    let zip = write_raw(
        t.path(),
        "odd.zip",
        &[(b"/abs/file.txt", b"1"), (b"C:/win.txt", b"2"), (b"con.txt", b"3"), (b"a:b?.txt", b"4")],
    );
    let dest = t.path().join("out");
    extract(&zip, &[], &dest).unwrap();
    assert!(dest.join("abs/file.txt").is_file());
    assert!(dest.join("C_/win.txt").is_file());
    assert!(dest.join("_con.txt").is_file());
    assert!(dest.join("a_b_.txt").is_file());
}

#[test]
fn selection_extracts_items_by_name_and_folders_with_children() {
    let t = tempfile::tempdir().unwrap();
    let proj = t.path().join("proj");
    touch(&proj.join("a.txt"), "A");
    touch(&proj.join("docs/guide/intro.md"), "I");
    touch(&proj.join("docs/guide/deep/x.txt"), "X");
    touch(&proj.join("docs/other.txt"), "O");
    let zip = create_zip(&[proj], &Options::default()).unwrap().output;

    // 深い階層のファイル1つ → dest 直下に名前だけ
    let d1 = t.path().join("d1");
    let r = extract(&zip, &["docs/guide/intro.md".into()], &d1).unwrap();
    assert_eq!(fs::read_to_string(d1.join("intro.md")).unwrap(), "I");
    assert_eq!(r.items, vec![d1.join("intro.md")]);

    // フォルダ → フォルダ名から下を保持、兄弟は含まない
    let d2 = t.path().join("d2");
    let r = extract(&zip, &["docs/guide".into()], &d2).unwrap();
    assert!(d2.join("guide/intro.md").is_file());
    assert!(d2.join("guide/deep/x.txt").is_file());
    assert!(!d2.join("guide/other.txt").exists());
    assert!(!d2.join("other.txt").exists());
    assert_eq!(r.items, vec![d2.join("guide")]);
    assert_eq!(r.files, 2);

    // 複数選択
    let d3 = t.path().join("d3");
    let r = extract(&zip, &["a.txt".into(), "docs/other.txt".into()], &d3).unwrap();
    assert!(d3.join("a.txt").is_file() && d3.join("other.txt").is_file());
    assert_eq!(r.items.len(), 2);
}

#[test]
fn name_collisions_never_overwrite() {
    let t = tempfile::tempdir().unwrap();
    let proj = t.path().join("proj");
    touch(&proj.join("a.txt"), "new");
    let zip = create_zip(&[proj], &Options::default()).unwrap().output;

    let dest = t.path().join("out");
    touch(&dest.join("a.txt"), "precious");
    let r = extract(&zip, &["a.txt".into()], &dest).unwrap();
    assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "precious");
    assert_eq!(fs::read_to_string(dest.join("a (1).txt")).unwrap(), "new");
    assert_eq!(r.items, vec![dest.join("a (1).txt")]);
}

#[test]
fn extract_all_restores_tree_and_empty_dirs() {
    let t = tempfile::tempdir().unwrap();
    let proj = t.path().join("proj");
    touch(&proj.join("a.txt"), "A");
    touch(&proj.join("sub/b.txt"), "B");
    fs::create_dir_all(proj.join("empty")).unwrap();
    let zip = create_zip(&[proj], &Options::default()).unwrap().output;

    let dest = t.path().join("out");
    let r = extract(&zip, &[], &dest).unwrap();
    assert_eq!(fs::read_to_string(dest.join("sub/b.txt")).unwrap(), "B");
    assert!(dest.join("empty").is_dir());
    assert_eq!(r.files, 2);
    assert!(r.skipped.is_empty());
}

#[test]
fn extracted_file_keeps_modified_time() {
    let t = tempfile::tempdir().unwrap();
    let zip = write_raw(t.path(), "m.zip", &[(b"f.txt", b"x")]);
    let dest = t.path().join("out");
    extract(&zip, &[], &dest).unwrap();
    let mtime = fs::metadata(dest.join("f.txt")).unwrap().modified().unwrap();
    let secs = mtime.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    // 2020-05-17 は UTC で 1589673600 前後。タイムゾーン差を見込んで±1日で確認
    assert!((1589587200..=1589760000).contains(&secs), "mtime={secs}");
}
