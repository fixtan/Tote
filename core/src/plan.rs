//! 書庫に入れる項目の洗い出し（形式に依らない共通部分）。
//!
//! 方針は lib.rs の冒頭を参照: フォルダ1つだけなら中身をルート直下へ、それ以外は各項目をルート直下へ。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{Error, file_name, io_err};

pub(crate) struct Item {
    /// 書庫内の名前（`/` 区切り、フォルダも末尾の `/` なし）
    pub name: String,
    pub path: PathBuf,
    pub meta: fs::Metadata,
}

impl Item {
    pub fn is_dir(&self) -> bool {
        self.meta.is_dir()
    }
}

pub(crate) struct Plan {
    pub items: Vec<Item>,
    pub skipped: Vec<PathBuf>,
}

/// ルート直下で名前が被ったら `name (1).ext` にして避ける
fn claim(used: &mut HashSet<String>, name: String) -> String {
    if used.insert(name.clone()) {
        return name;
    }
    let (base, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    for n in 1.. {
        let cand = format!("{base} ({n}){ext}");
        if used.insert(cand.clone()) {
            return cand;
        }
    }
    unreachable!()
}

struct Walker<'a> {
    skip: &'a [PathBuf],
    items: Vec<Item>,
    skipped: Vec<PathBuf>,
}

impl Walker<'_> {
    fn tree(&mut self, path: &Path, name: String) -> Result<(), Error> {
        let meta = fs::symlink_metadata(path).map_err(io_err(path))?;
        if meta.file_type().is_symlink() {
            self.skipped.push(path.to_path_buf());
        } else if meta.is_dir() {
            self.items.push(Item { name: name.clone(), path: path.to_path_buf(), meta });
            self.children(path, &name, None)?;
        } else if meta.is_file() {
            // 出力する書庫自身を取り込まない
            if fs::canonicalize(path).map(|p| self.skip.contains(&p)).unwrap_or(false) {
                return Ok(());
            }
            self.items.push(Item { name, path: path.to_path_buf(), meta });
        } else {
            self.skipped.push(path.to_path_buf());
        }
        Ok(())
    }

    /// フォルダの中身を `prefix` 配下へ。`root_used` があれば（prefix が空のとき）名前の衝突を避ける。
    fn children(&mut self, dir: &Path, prefix: &str, mut root_used: Option<&mut HashSet<String>>) -> Result<(), Error> {
        let mut entries: Vec<_> = fs::read_dir(dir).map_err(io_err(dir))?.collect::<Result<_, _>>().map_err(io_err(dir))?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let zp = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let zp = match root_used.as_deref_mut() {
                Some(u) if prefix.is_empty() => claim(u, zp),
                _ => zp,
            };
            self.tree(&e.path(), zp)?;
        }
        Ok(())
    }
}

pub(crate) fn collect(inputs: &[PathBuf], skip: &[PathBuf]) -> Result<Plan, Error> {
    let mut w = Walker { skip, items: Vec::new(), skipped: Vec::new() };
    let single_dir = inputs.len() == 1 && fs::metadata(&inputs[0]).map(|m| m.is_dir()).unwrap_or(false);
    let mut used = HashSet::new();
    if single_dir {
        w.children(&inputs[0], "", Some(&mut used))?;
    } else {
        for p in inputs {
            let name = claim(&mut used, file_name(p));
            w.tree(p, name)?;
        }
    }
    Ok(Plan { items: w.items, skipped: w.skipped })
}
