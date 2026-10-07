use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use tote_core::append::{AppendMode, add_to_zip};
use tote_core::{Options, create_zip};

fn touch(p: &Path, body: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// 名前→中身（フォルダは None）
fn dump(zip: &Path) -> Vec<(String, Option<String>)> {
    let mut a = zip::ZipArchive::new(File::open(zip).unwrap()).unwrap();
    let mut v = Vec::new();
    for i in 0..a.len() {
        let mut f = a.by_index(i).unwrap();
        let name = f.name().to_string();
        if f.is_dir() {
            v.push((name, None));
        } else {
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            v.push((name, Some(s)));
        }
    }
    v.sort();
    v
}

fn base_zip(t: &Path) -> PathBuf {
    let src = t.join("src");
    touch(&src.join("a.txt"), "A");
    touch(&src.join("sub/b.txt"), "B");
    let out = t.join("base.zip");
    create_zip(&[src], &Options { output: Some(out.clone()), ..Default::default() }).unwrap();
    out
}

fn leftovers(dir: &Path) -> Vec<String> {
    fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".tote-")).collect()
}

#[test]
fn safe_adds_files_and_folders() {
    let t = tempfile::tempdir().unwrap();
    let z = base_zip(t.path());
    touch(&t.path().join("in/new.txt"), "N");
    touch(&t.path().join("in/dir/x.txt"), "X");
    let s = add_to_zip(&z, "", &[t.path().join("in/new.txt"), t.path().join("in/dir")], AppendMode::Safe, None).unwrap();
    assert_eq!((s.files, s.dirs, s.replaced), (2, 1, 0));
    let d = dump(&z);
    let names: Vec<_> = d.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(names, ["a.txt", "dir/", "dir/x.txt", "new.txt", "sub/", "sub/b.txt"]);
    assert!(leftovers(t.path()).is_empty());
}

#[test]
fn into_subfolder_and_replace_same_name() {
    let t = tempfile::tempdir().unwrap();
    let z = base_zip(t.path());
    touch(&t.path().join("in/b.txt"), "B2");
    let s = add_to_zip(&z, "sub", &[t.path().join("in/b.txt")], AppendMode::Safe, None).unwrap();
    assert_eq!((s.files, s.replaced), (1, 1));
    let d = dump(&z);
    assert_eq!(d.iter().filter(|x| x.0 == "sub/b.txt").count(), 1);
    assert_eq!(d.iter().find(|x| x.0 == "sub/b.txt").unwrap().1.as_deref(), Some("B2"));
    assert_eq!(d.iter().find(|x| x.0 == "a.txt").unwrap().1.as_deref(), Some("A"));
}

#[test]
fn fast_appends_in_place_and_falls_back_on_conflict() {
    let t = tempfile::tempdir().unwrap();
    let z = base_zip(t.path());
    touch(&t.path().join("in/n.txt"), "N");
    let s = add_to_zip(&z, "", &[t.path().join("in/n.txt")], AppendMode::Fast, Some(0)).unwrap();
    assert_eq!(s.mode_used, AppendMode::Fast);
    assert_eq!(dump(&z).len(), 4);

    touch(&t.path().join("in2/a.txt"), "A2");
    let s = add_to_zip(&z, "", &[t.path().join("in2/a.txt")], AppendMode::Fast, None).unwrap();
    assert_eq!(s.mode_used, AppendMode::Safe);
    assert_eq!(s.replaced, 1);
    let d = dump(&z);
    assert_eq!(d.iter().find(|x| x.0 == "a.txt").unwrap().1.as_deref(), Some("A2"));
    assert_eq!(d.len(), 4);
}

#[test]
fn duplicate_input_names_are_renamed_and_self_is_skipped() {
    let t = tempfile::tempdir().unwrap();
    let z = base_zip(t.path());
    touch(&t.path().join("p/same.txt"), "1");
    touch(&t.path().join("q/same.txt"), "2");
    let s = add_to_zip(&z, "", &[t.path().join("p/same.txt"), t.path().join("q/same.txt"), z.clone()], AppendMode::Safe, None).unwrap();
    assert_eq!(s.files, 2);
    let names: Vec<_> = dump(&z).into_iter().map(|x| x.0).collect();
    assert!(names.contains(&"same.txt".to_string()) && names.contains(&"same (1).txt".to_string()));
    assert!(!names.iter().any(|n| n.contains("base.zip")));
}

#[test]
fn errors_keep_original_intact() {
    let t = tempfile::tempdir().unwrap();
    let z = base_zip(t.path());
    let before = fs::read(&z).unwrap();
    // 存在しない入力
    assert!(add_to_zip(&z, "", &[t.path().join("nope.txt")], AppendMode::Safe, None).is_err());
    // 不正な追加先
    touch(&t.path().join("in/n.txt"), "N");
    assert!(add_to_zip(&z, "../x", &[t.path().join("in/n.txt")], AppendMode::Safe, None).is_err());
    // ZIPではないファイル
    let junk = t.path().join("junk.zip");
    fs::write(&junk, b"not a zip").unwrap();
    assert!(add_to_zip(&junk, "", &[t.path().join("in/n.txt")], AppendMode::Safe, None).is_err());
    assert_eq!(fs::read(&junk).unwrap(), b"not a zip");
    assert_eq!(fs::read(&z).unwrap(), before);
    assert!(leftovers(t.path()).is_empty());
}

#[test]
fn file_vs_folder_name_clash_is_rejected() {
    let t = tempfile::tempdir().unwrap();
    let z = base_zip(t.path());
    touch(&t.path().join("in/sub"), "i am a file");
    let e = add_to_zip(&z, "", &[t.path().join("in/sub")], AppendMode::Safe, None).unwrap_err();
    assert!(e.to_string().contains("sub"));
}
