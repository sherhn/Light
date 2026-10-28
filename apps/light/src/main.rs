//! Light - a modular IDE for any language.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod app_check;
mod app_scenario;
mod ui_scenarios;
mod app_env;
mod hover;
mod lang_completion;
mod modules;
mod preview;
mod ui_preview;
mod ui_modules;
mod brand;
mod app_git;
mod app_github;
mod app_local;
mod app_lsp;
mod app_run;
mod app_split;
mod app_terminal;
mod app_wsl;
mod diff_tab;
mod doc;
mod editor_view;
mod folding;
mod fonts;
mod i18n;
mod i18n_ru;
mod icons;
mod log_select;
mod lucide;
mod input;
mod script;
mod scrollbars;
mod state;
mod theme;
mod ui;
mod ui_chrome;
mod ui_titlebar;
mod ui_wsl;
mod ui_panels;
mod widgets;
mod ui_popups;
mod ui_split;
mod ui_settings;
mod ui_theme;
mod ui_tools;
mod ui_clone;
mod ui_project_browser;
mod project_location;
mod ui_github;
mod ui_commit;
mod ui_diff;
mod ui_local;

use std::path::PathBuf;

use eframe::egui;
use ide_core::settings::Settings;

struct Args {
    paths: Vec<PathBuf>,
    script: Option<PathBuf>,
    size: Option<(f32, f32)>,
    no_restore: bool,
}

fn parse_args() -> Args {
    let mut a = Args { paths: vec![], script: None, size: None, no_restore: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--script" => a.script = it.next().map(PathBuf::from),
            "--size" => {
                if let Some(s) = it.next()
                    && let Some((w, h)) = s.split_once('x')
                    && let (Ok(w), Ok(h)) = (w.parse(), h.parse())
                {
                    a.size = Some((w, h));
                }
            }
            "--no-restore" => a.no_restore = true,
            "--help" | "-h" => {
                println!("light [PATH...] [--script FILE.json] [--size WxH] [--no-restore]");
                std::process::exit(0);
            }
            p => a.paths.push(PathBuf::from(p)),
        }
    }
    a
}

fn main() -> eframe::Result {
    if let Ok(executable) = std::env::current_exe()
        && let Some(path) = ide_core::bundle::tool_path(&executable, std::env::var_os("PATH").as_deref())
    {
        // Startup is still single-threaded; no threads or child processes can read PATH yet.
        unsafe { std::env::set_var("PATH", path) };
    }
    let args = parse_args();
    let settings = Settings::load();
    let (w, h) = args.size.unwrap_or((1280.0, 800.0));
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([w, h]).with_min_inner_size([640.0, 400.0]).with_title("Light").with_decorations(false).with_app_id("light");
    if let Some(icon) = brand::window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Light",
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            let logo = brand::load_logo(&cc.egui_ctx);
            let mut app = app::App::new(settings);
            app.logo = logo;
            app.use_saved_history();
            app.pal.apply(&cc.egui_ctx);
            // what to open: explicit paths win, otherwise restore the last session
            if !args.paths.is_empty() {
                for p in &args.paths {
                    if p.is_dir() {
                        app.open_project(p);
                    } else {
                        if let Some(parent) = p.parent().filter(|d| app.project.is_none() && d.is_dir() && !d.as_os_str().is_empty()) {
                            app.open_project(parent);
                        }
                        app.open_file(p);
                    }
                }
            } else if !args.no_restore {
                    // Opening the project persists a fresh session. Snapshot the
                    // previous tabs before that save replaces open_files.
                    let files = app.settings.open_files.clone();
                    let active = app.settings.active_file.clone();
                    if let Some(last) = app.settings.last_project.clone().filter(|p| std::path::Path::new(p).is_dir()) {
                        app.open_project(std::path::Path::new(&last));
                    }
                    for f in files {
                        let p = PathBuf::from(&f);
                        if p.is_file() {
                            app.open_file(&p);
                        }
                    }
                    if let Some(active) = active {
                        let p = PathBuf::from(active);
                        if p.is_file() {
                            app.open_file(&p);
                        }
                    }
                }
            // a first start with nothing installed: show what can be downloaded
            if args.script.is_none() && app.mods.store.is_some() && app.mods.installed.is_empty() && !app.settings.modules_welcome_done {
                app.open_modules_tab();
            }
            if let Some(path) = &args.script {
                match script::load(path) {
                    Ok(s) => app.script = Some(s),
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(2);
                    }
                }
            }
            Ok(Box::new(app))
        }),
    )
}
