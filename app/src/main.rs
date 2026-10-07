// リリースビルドのWindowsではコンソールを出さない（右クリックから無窓で動かすため）
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod collect;
mod config;
mod shell;
mod ui;
#[cfg(feature = "viewer")]
mod gui;
#[cfg(feature = "viewer")]
mod settings;
#[cfg(feature = "viewer")]
mod viewer;

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Instant;


const USAGE: &str = "Tote — 書庫の作成・閲覧ツール\n\n\
使い方:\n  tote                         設定画面を開く（右クリック登録・圧縮設定など）\n  tote <ファイル/フォルダ>...   ZIPを作成\n  tote --open <書庫>           書庫の中身を開く（zip 7z rar gz tgz tar cab lzh）\n  tote --install               右クリック・「送る」・ZIPの関連付け候補を登録\n  tote --uninstall             登録をすべて解除";

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    match args.first().and_then(|a| a.to_str()) {
        None | Some("--settings") => open_settings(),
        Some("--install") => {
            report(shell::install_defaults(&mut shell::System, &exe()), "登録しました。右クリックメニューと「送る」に追加されています。")
        }
        Some("--uninstall") => report(shell::uninstall_all(&mut shell::System), "登録を解除しました。"),
        Some("--open") => open_viewer(args.get(1)),
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

/// `--open <書庫>`: 書庫の中身を見る窓を開く。
fn open_viewer(path: Option<&OsString>) {
    let Some(p) = path else {
        ui::error("--open には書庫ファイルのパスを指定してください");
        return;
    };
    #[cfg(feature = "viewer")]
    gui::run(gui::Mode::Viewer(PathBuf::from(p)));
    #[cfg(not(feature = "viewer"))]
    {
        let _ = p;
        ui::error("このビルドにはビューアが含まれていません");
    }
}

/// 引数なし起動（exeのダブルクリック）: 設定画面を開く。
fn open_settings() {
    #[cfg(feature = "viewer")]
    gui::run(gui::Mode::Settings);
    #[cfg(not(feature = "viewer"))]
    interactive_setup();
}

/// 画面なしのビルド用の簡易版: 登録/解除だけ選べる。
#[cfg(not(feature = "viewer"))]
fn interactive_setup() {
    match ui::ask("右クリックメニューと「送る」に登録しますか？\n\nはい: 登録 / いいえ: 登録解除 / キャンセル: 何もしない") {
        ui::Choice::Yes => report(shell::install_defaults(&mut shell::System, &exe()), "登録しました。"),
        ui::Choice::No => report(shell::uninstall_all(&mut shell::System), "登録を解除しました。"),
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
    let cfg = config::load();
    match tote_core::create(&paths, &cfg.compress_options()) {
        Ok(summary) => {
            if !summary.skipped.is_empty() {
                ui::error(&format!(
                    "{} 個の項目（シンボリックリンク等）をスキップしました。\n作成: {}",
                    summary.skipped.len(),
                    summary.output.display()
                ));
            } else if cfg.should_reveal(started.elapsed().as_secs_f32()) {
                ui::reveal(&summary.output);
            }
        }
        Err(e) => ui::error(&e.to_string()),
    }
}
