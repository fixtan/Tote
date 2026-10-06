// リリースビルドのWindowsではコンソールを出さない（右クリックから無窓で動かすため）
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod collect;
mod shell;
mod ui;

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Instant;

use tote_core::{Options, create_zip};

const USAGE: &str = "Tote — ZIP作成ツール\n\n\
使い方:\n  tote <ファイル/フォルダ>...   ZIPを作成\n  tote --install               右クリックと「送る」に登録\n  tote --uninstall             登録を解除";

/// この秒数以上かかったZIPは、完了をエクスプローラーで選択して知らせる
const REVEAL_AFTER_SECS: f32 = 3.0;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    match args.first().and_then(|a| a.to_str()) {
        None => interactive_setup(),
        Some("--install") => report(shell::install(&exe()), "登録しました。右クリックメニューと「送る」に追加されています。"),
        Some("--uninstall") => report(shell::uninstall(), "登録を解除しました。"),
        Some("--help" | "-h" | "/?") => ui::info(USAGE),
        Some(_) => compress_args(args.into_iter().map(PathBuf::from).collect()),
    }
}

fn exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tote.exe"))
}

fn report(r: Result<(), String>, ok_msg: &str) {
    match r {
        Ok(()) => ui::info(ok_msg),
        Err(e) => ui::error(&e),
    }
}

/// 引数なし起動（exeのダブルクリック）: 登録/解除を選べる。
fn interactive_setup() {
    match ui::ask("右クリックメニューと「送る」に登録しますか？\n\nはい: 登録 / いいえ: 登録解除 / キャンセル: 何もしない") {
        ui::Choice::Yes => report(shell::install(&exe()), "登録しました。"),
        ui::Choice::No => report(shell::uninstall(), "登録を解除しました。"),
        ui::Choice::Cancel => {}
    }
}

fn compress_args(paths: Vec<PathBuf>) {
    let dir = collect::default_spool_dir();
    if paths.len() > 1 {
        // 「送る」やD&Dは1プロセスで全部届くので、集約を待たずに直接処理する
        run(paths);
        return;
    }
    // 右クリック動詞は選択数ぶんプロセスが起動されるので、引数を集約する
    if let Err(e) = collect::submit(&dir, &paths) {
        ui::error(&format!("一時フォルダへの書き込みに失敗しました: {e}"));
        return;
    }
    collect::drain_as_leader(&dir, collect::DEBOUNCE, run);
}

fn run(paths: Vec<PathBuf>) {
    let started = Instant::now();
    match create_zip(&paths, &Options::default()) {
        Ok(summary) => {
            if !summary.skipped.is_empty() {
                ui::error(&format!(
                    "{} 個の項目（シンボリックリンク等）をスキップしました。\n作成: {}",
                    summary.skipped.len(),
                    summary.output.display()
                ));
            } else if started.elapsed().as_secs_f32() >= REVEAL_AFTER_SECS {
                ui::reveal(&summary.output);
            }
        }
        Err(e) => ui::error(&e.to_string()),
    }
}
