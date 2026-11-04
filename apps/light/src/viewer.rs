//! File-only startup: no App, module store, project index, Git, WSL or session writes.
use std::{path::PathBuf, sync::mpsc};
use eframe::egui;
use crate::{doc::Doc, editor_view::{self, ViewCfg}, theme::Palette};
use crate::i18n::tr;

pub fn should_view(paths: &[PathBuf], full: bool, script: bool) -> bool {
    !full && !script && !paths.is_empty() && paths.iter().all(|p| !p.is_dir())
}

struct Viewer {
    paths: Vec<PathBuf>,
    active: usize,
    generation: u64,
    rx: mpsc::Receiver<(u64, Result<String, String>)>,
    tx: mpsc::Sender<(u64, Result<String, String>)>,
    doc: Option<Doc>,
    error: Option<String>,
    settings: ide_core::settings::Settings,
    pal: Palette,
    query: String,
    ready: bool,
    started: std::time::Instant,
}

fn read(path: &std::path::Path) -> Result<String, String> {
    // Bound allocation even for a file that grows while it is read.
    use std::io::Read;
    const LIMIT: u64 = 32 * 1024 * 1024;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT { return Err(tr("File is too large for quick view (32 MB)").into()); }
    let text = String::from_utf8(bytes).map_err(|_| tr("This file is not UTF-8 text").to_string())?;
    if text.contains('\0') { return Err(tr("This file is not UTF-8 text").into()); }
    Ok(text)
}

fn document(path: &std::path::Path, text: &str) -> Doc {
    let mut doc = Doc::untitled(0);
    doc.path = Some(path.to_path_buf());
    doc.ed = ide_core::editor::Editor::new(text, lisp_syntax::Dialect::from_path(path).unwrap_or_default());
    if !lisp_syntax::dialect::is_lisp_file(path) {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        let module = match ext.as_str() {
            "rs" | "c" | "h" | "cpp" | "hpp" => include_str!("../../../registry/modules/c-cpp/module.json"),
            "js" | "mjs" | "ts" | "tsx" | "jsx" => include_str!("../../../registry/modules/javascript/module.json"),
            "py" => include_str!("../../../registry/modules/python/module.json"),
            "html" | "htm" => include_str!("../../../registry/modules/html/module.json"),
            "css" => include_str!("../../../registry/modules/css/module.json"),
            "json" => include_str!("../../../registry/modules/json/module.json"),
            "md" => include_str!("../../../registry/modules/markdown/module.json"),
            "toml" => include_str!("../../../registry/modules/toml/module.json"),
            "yaml" | "yml" => include_str!("../../../registry/modules/yaml/module.json"),
            "sh" | "bash" | "mk" => include_str!("../../../registry/modules/shell-make/module.json"),
            "ini" => include_str!("../../../registry/modules/ini/module.json"),
            "rst" => include_str!("../../../registry/modules/restructuredtext/module.json"),
            "asm" | "s" => include_str!("../../../registry/modules/assembler/module.json"),
            _ => "{}",
        };
        let definition = serde_json::from_str::<ide_core::modules::Manifest>(module).ok().and_then(|m| m.languages.into_iter().find(|l| l.extensions.iter().any(|e| e == &ext)));
        let lang = definition.and_then(|d| light_lang::Language::new(d).ok()).unwrap_or_else(light_lang::Language::plain);
        doc.ed.set_language(Some(std::sync::Arc::new(lang)));
    }
    doc
}

impl Viewer {
    fn load(&mut self) {
        self.generation += 1;
        self.doc = None; self.error = None;
        let (path, tx, generation) = (self.paths[self.active].clone(), self.tx.clone(), self.generation);
        std::thread::spawn(move || { let _ = tx.send((generation, read(&path))); });
    }
}

pub fn run(paths: Vec<PathBuf>, settings: ide_core::settings::Settings, smoke: bool, started: std::time::Instant) -> eframe::Result {
    let (tx, rx) = mpsc::channel();
    let pal = Palette::for_settings(&settings);
    let dark = crate::brand::use_dark_logo(settings.external_logo, crate::brand::taskbar_is_light().unwrap_or(false));
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([960.0, 700.0]).with_title("Light — Quick View");
    if let Some(icon) = crate::brand::window_icon(dark) { viewport = viewport.with_icon(std::sync::Arc::new(icon)); }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Light Quick View", options, Box::new(move |cc| {
        crate::fonts::install(&cc.egui_ctx); pal.apply(&cc.egui_ctx);
        let mut app = Viewer { paths, active: 0, generation: 0, rx, tx, doc: None, error: None, settings, pal, query: String::new(), ready: false, started };
        app.load();
        Ok(Box::new(ViewerApp { viewer: app, smoke }))
    }))
}

struct ViewerApp { viewer: Viewer, smoke: bool }
impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let v = &mut self.viewer;
        let ctx = ui.ctx().clone();
        while let Ok((generation, result)) = v.rx.try_recv() {
            if generation == v.generation { match result { Ok(text) => v.doc = Some(document(&v.paths[v.active], &text)), Err(e) => v.error = Some(e) } }
        }
        egui::Panel::top("viewer_tools").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("Quick View — read only"));
                if ui.button(tr("Open in IDE")).clicked() {
                    let result = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--ide").arg(&v.paths[v.active]).spawn());
                    match result { Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close), Err(e) => v.error = Some(e.to_string()) }
                }
                ui.label(tr("Find")); ui.text_edit_singleline(&mut v.query);
                if ui.button(tr("Reload")).clicked() { v.load(); }
            });
            ui.horizontal_wrapped(|ui| {
                let mut next = v.active;
                for (i, path) in v.paths.iter().enumerate() {
                    if ui.selectable_label(i == v.active, path.file_name().unwrap_or_default().to_string_lossy()).on_hover_text(path.display().to_string()).clicked() { next = i; }
                }
                if next != v.active { v.active = next; v.load(); }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(doc) = &mut v.doc {
                if !ctx.text_edit_focused() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A)) { doc.ed.select_all(); }
                if !ctx.text_edit_focused() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)) { ctx.copy_text(doc.ed.selected_text().to_owned()); }
                let found: Vec<_> = if v.query.is_empty() { vec![] } else { doc.ed.text().match_indices(&v.query).map(|(i, s)| (i, i + s.len())).collect() };
                if ui.button(format!("{}: {}", tr("Matches"), found.len())).clicked() && let Some(&(start, end)) = found.iter().find(|(s, _)| *s > doc.ed.caret()).or(found.first()) {
                    doc.ed.select_range(start, end); doc.view.scroll_to_caret = true;
                }
                let cfg = ViewCfg { pal: &v.pal, font_size: v.settings.font_size, line_numbers: true, whitespace: false, rainbow: true, focused: false, markers: &[], find: &found, find_current: None, git_marks: &[], minimap: false, doc_id: doc.id, icons: &[], line_tints: &[], breakpoint_lane: false, breakpoints: &[], coverage: &[], annotations: &[] };
                editor_view::show(ui, &mut doc.ed, &mut doc.view, &mut doc.folds, &cfg);
                if !v.ready {
                    v.ready = true; eprintln!("QUICK VIEW READY {} ms", v.started.elapsed().as_millis());
                    if self.smoke {
                        if let Some(path) = std::env::var_os("LIGHT_VIEWER_SCREENSHOT") {
                            let close = ctx.clone();
                            ctx.request_screenshot(move |image| {
                                let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                                image::save_buffer(path, &bytes, image.size[0] as u32, image.size[1] as u32, image::ColorType::Rgba8).expect("save viewer screenshot");
                                close.send_viewport_cmd(egui::ViewportCommand::Close);
                            });
                        } else { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
                    }
                }
            } else if let Some(error) = &v.error { ui.colored_label(v.pal.error, error); if self.smoke { ctx.send_viewport_cmd(egui::ViewportCommand::Close); } }
            else { ui.spinner(); }
        });
        if v.doc.is_none() && v.error.is_none() { ctx.request_repaint_after(std::time::Duration::from_millis(16)); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_ide_and_scripts_bypass_quick_view() {
        let paths = vec![PathBuf::from("missing-file.js")];
        assert!(should_view(&paths, false, false));
        assert!(!should_view(&paths, true, false));
        assert!(!should_view(&paths, false, true));
        assert!(!should_view(&[], false, false));
        assert!(!should_view(&[std::env::temp_dir()], false, false));
    }

    #[test]
    fn reading_is_bounded_and_never_writes_the_source() {
        let dir = std::env::temp_dir().join(format!("light-viewer-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("code.js");
        let source = "const answer = 42;\n";
        std::fs::write(&path, source).unwrap();
        assert_eq!(read(&path).unwrap(), source);
        let doc = document(&path, source);
        assert_eq!(doc.ed.language().unwrap().id(), "javascript");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        std::fs::write(&path, [0, 1, 2]).unwrap();
        assert!(read(&path).is_err());
        std::fs::write(&path, [255, 254]).unwrap();
        assert!(read(&path).is_err());
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(33 * 1024 * 1024).unwrap(); drop(file);
        assert!(read(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(read(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
