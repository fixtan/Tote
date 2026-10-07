//! Tauri の起動（ビューア / 設定画面）。`generate_context!` は1か所だけに置く（二重に埋め込まないため）。

use std::path::PathBuf;

use tauri::{WebviewUrl, WebviewWindowBuilder};

use crate::{settings, viewer};

pub enum Mode {
    /// 書庫の中身を見る窓
    Viewer(PathBuf),
    /// 設定画面
    Settings,
}

pub fn run(mode: Mode) {
    let (label, page, title, size, min, zip) = match &mode {
        Mode::Viewer(p) => {
            viewer::cleanup_old_scratch();
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ("main", "index.html", format!("{name} - Tote"), (980.0, 640.0), (560.0, 360.0), p.clone())
        }
        Mode::Settings => ("settings", "settings.html", "Tote の設定".to_string(), (780.0, 760.0), (560.0, 420.0), PathBuf::new()),
    };

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(viewer::Viewer::new(zip))
        .invoke_handler(tauri::generate_handler![
            viewer::load_archive,
            viewer::extract_all,
            viewer::extract_selected,
            viewer::open_entry,
            viewer::open_folder,
            viewer::prepare_drag,
            viewer::start_drag,
            settings::get_state,
            settings::get_config,
            settings::save_config,
            settings::reset_config,
            settings::apply_items,
            settings::repair_items,
            settings::uninstall_everything,
            settings::open_default_apps,
            settings::open_settings
        ])
        .setup(move |app| {
            WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
                .title(title.clone())
                .inner_size(size.0, size.1)
                .min_inner_size(min.0, min.1)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!());

    if let Err(e) = result {
        crate::ui::error(&format!("画面を起動できませんでした: {e}"));
    }
}
