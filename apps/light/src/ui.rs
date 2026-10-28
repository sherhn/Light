//! The root layout and the `eframe::App` implementation.

use eframe::egui::{self, CornerRadius, Frame, Margin};

use crate::app::App;
use crate::editor_view::{self, ViewCfg};
use crate::preview::PreviewMode;
use crate::state::*;
use crate::widgets::{self, STATUS_H, STRIP_W, TOOLBAR_H};

impl eframe::App for App {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.script_inject(raw_input);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.exit_now {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.tick(&ctx);
        self.handle_input(&ctx);
        self.script_step(&ctx);
        self.handle_dropped_files(&ctx);

        let pal = self.pal;
        // window title
        let title = {
            let file = self.doc().map(|d| d.name());
            match (&self.project, file) {
                (Some(p), Some(f)) => format!("{} [{}] - {f} - Light", p.name, p.root.display()),
                (Some(p), None) => format!("{} [{}] - Light", p.name, p.root.display()),
                (None, Some(f)) => format!("{f} - Light"),
                (None, None) => "Light".to_string(),
            }
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
        if !self.exit_now && ctx.input(|i| i.viewport().close_requested()) {
            self.flush_saves();
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.has_unsaved() && !self.exit_now {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.popup = Popup::Confirm { kind: ConfirmKind::Exit, message: crate::i18n::tr("Save changes before exiting?").into() };
        }

        // soft gradient window background; tool windows float above it as rounded cards
        widgets::paint_base(ui.painter(), ui.max_rect(), &pal);
        let none = Frame::NONE;
        let gap = widgets::GAP;
        egui::Panel::top("titlebar").exact_size(crate::ui_titlebar::TITLEBAR_H).show_separator_line(false).frame(none).show(ui, |ui| self.draw_titlebar(ui, &ctx));
        egui::Panel::top("toolbar").exact_size(TOOLBAR_H).show_separator_line(false).frame(none).show(ui, |ui| self.draw_toolbar(ui, &ctx));
        egui::Panel::bottom("status").exact_size(STATUS_H).show_separator_line(false).frame(none).show(ui, |ui| self.draw_status_bar(ui, &ctx));
        egui::Panel::left("strip").exact_size(STRIP_W).resizable(false).show_separator_line(false).frame(none).show(ui, |ui| self.draw_tool_strip(ui, &ctx));
        if let Some(t) = self.left_tool {
            let margin = Margin { left: 0, right: gap, top: 0, bottom: gap };
            let (panel_id, width) = if t == LeftTool::Commit { ("left_commit", 360.0) } else { ("left_tool", 290.0) };
            egui::Panel::left(panel_id).resizable(true).default_size(width).size_range(220.0..=700.0).show_separator_line(false).frame(none.inner_margin(margin)).show(ui, |ui| {
                // no card behind the side panels: a short file tree on a big slab looks like an empty block
                widgets::card_frame(&pal, egui::Color32::TRANSPARENT).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    match t {
                        LeftTool::Project => self.draw_project_panel(ui, &ctx),
                        LeftTool::Structure => self.draw_structure_panel(ui),
                        LeftTool::Commit => self.draw_commit_panel(ui, &ctx),
                    }
                });
            });
        }
        if self.bottom_tool.is_some() {
            let margin = Margin { left: 0, right: gap, top: gap, bottom: gap };
            egui::Panel::bottom("bottom_tool").resizable(true).default_size(280.0).size_range(200.0..=900.0).show_separator_line(false).frame(none.inner_margin(margin)).show(ui, |ui| {
                widgets::card_frame(&pal, pal.panel_bg).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    self.draw_bottom_panel(ui, &ctx);
                });
            });
        }
        let margin = Margin { left: 0, right: gap, top: 0, bottom: if self.bottom_tool.is_some() { 0 } else { gap } };
        egui::CentralPanel::default().frame(none.inner_margin(margin)).show(ui, |ui| {
            widgets::card_frame(&pal, pal.editor_bg).show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.draw_central(ui, &ctx);
            });
        });

        self.tick_palette();
        self.draw_popups(&ctx);
        self.draw_toasts(&ctx);
        self.process_screenshots(&ctx);
        self.draw_resize_handles(&ctx);

        // keep animating only when needed
        let busy = self.repl.starting || !self.repl.busy.is_empty() || self.run.running || self.other_runs.iter().any(|tab| tab.run.running) || self.compile.is_some() || self.repl.session.is_some();
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        } else if self.doc().is_some() && self.editor_focus {
            ctx.request_repaint_after(std::time::Duration::from_millis(250)); // caret blink
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.exit_now { self.flush_saves(); }
        self.save_settings();
        self.stop_lsp();
        if let Some(s) = self.repl.session.take() {
            s.kill();
        }
        if let Some(h) = self.run.handle.take() {
            h.kill();
        }
        self.stop_other_runs();
    }
}

impl App {
    fn draw_central(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.draw_env_banner(ui);
        self.draw_tools_banner(ui);
        self.draw_modules_banner(ui);
        if self.docs.is_empty() {
            self.split = None;
            self.draw_welcome(ui, ctx);
            return;
        }
        self.fix_split();
        self.draw_tabs(ui, ctx);
        if self.docs.is_empty() {
            // the last tab was just closed
            self.split = None;
            return;
        }
        let shown = self.active.min(self.docs.len() - 1);
        self.active = shown;
        if self.split.is_some() {
            self.draw_split_panes(ui, ctx);
        } else {
            self.pane_rects.clear();
            self.draw_doc_pane(ui, ctx, shown, true, false);
        }
    }

    /// One editor pane: the diff of a diff tab, or the conflict banner, breadcrumbs, find bar and code of a file.
    /// Only the focused pane takes the keyboard, the find bar and the popup anchors. `own_view` means the pane keeps
    /// its scroll state apart from the document (both panes show the same document). Returns true on a click.
    pub fn draw_doc_pane(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, idx: usize, focused: bool, own_view: bool) -> bool {
        if self.docs[idx].diff.is_some() {
            if focused {
                self.editor_rect = None;
                self.gutter_rect = None;
                self.caret_screen = None;
            }
            match self.docs[idx].page {
                Some(crate::doc::Page::Modules) => self.draw_modules_page(ui, ctx),
                None => self.draw_diff_view(ui, idx),
            }
            return false;
        }
        // conflict banner
        if self.docs[idx].conflict {
            let pal = self.pal;
            let mut reload = false;
            let mut keep = false;
            Frame::new().fill(pal.warning.gamma_multiply(0.18)).inner_margin(Margin::symmetric(12, 6)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("This file was changed on disk, and you have unsaved edits.");
                    if ui.button("Reload from disk").clicked() {
                        reload = true;
                    }
                    if ui.button("Keep my version").clicked() {
                        keep = true;
                    }
                });
            });
            if reload {
                let _ = self.docs[idx].reload();
            }
            if keep {
                self.docs[idx].conflict = false;
            }
        }
        self.draw_breadcrumbs(ui, idx);
        if focused {
            self.draw_find_bar(ui, ctx);
            self.refresh_find();
        }
        // Markdown and reStructuredText: the code, the rendered document, or both side by side
        match (self.doc_markup(idx), self.preview_mode(idx)) {
            (Some(markup), PreviewMode::Preview) => {
                if focused {
                    self.editor_rect = None;
                    self.gutter_rect = None;
                    self.caret_screen = None;
                }
                self.draw_preview(ui, ctx, idx, markup);
                false
            }
            (Some(markup), PreviewMode::Split) => self.draw_split_markup(ui, ctx, idx, focused, own_view, markup),
            _ => self.draw_editor(ui, ctx, idx, focused, own_view),
        }
    }

    /// The code of a file: gutter, text, minimap and the problems on it.
    pub fn draw_editor(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, idx: usize, focused: bool, own_view: bool) -> bool {
        let markers = self.docs[idx].markers();
        let find: Vec<(usize, usize)> = if focused && self.find.open { self.find.matches.clone() } else { vec![] };
        let git_marks: Vec<ide_core::diff::Mark> = App::git_marks(&mut self.docs[idx]).to_vec();
        let editor_on = focused && self.editor_active(ctx);
        let cfg = ViewCfg {
            pal: &self.pal,
            font_size: self.settings.font_size,
            line_numbers: self.settings.show_line_numbers,
            whitespace: self.settings.show_whitespace,
            rainbow: self.settings.rainbow_brackets,
            focused: editor_on,
            markers: &markers,
            find: &find,
            find_current: self.find.current,
            git_marks: &git_marks,
            minimap: self.settings.minimap,
            doc_id: self.docs[idx].id,
        };
        let mut view = match (own_view, self.split.as_mut()) {
            (true, Some(sp)) => std::mem::take(&mut sp.view),
            _ => std::mem::take(&mut self.docs[idx].view),
        };
        let doc = &mut self.docs[idx];
        let out = editor_view::show(ui, &mut doc.ed, &mut view, &mut doc.folds, &cfg);
        if let Some(key) = out.fold_toggle.clone()
            && !doc.folds.remove(&key)
        {
            doc.folds.insert(key);
        }
        match (own_view, self.split.as_mut()) {
            (true, Some(sp)) => sp.view = view,
            _ => self.docs[idx].view = view,
        }
        if focused {
            self.editor_rect = out.text_rect;
            self.gutter_rect = out.gutter_rect;
            if let Some((line, pos)) = out.gutter_click {
                self.open_hunk_popup(line, pos);
            }
            self.caret_screen = out.caret_screen;
        }
        if out.clicked {
            self.editor_focus = true;
            self.completion.active = false;
        }
        self.update_hover(ctx, idx, &out);
        if let Some(action) = out.context_action {
            self.show_doc(idx);
            self.editor_focus = true;
            let path = self.docs[idx].path.clone();
            let rel = self.current_vcs_rel_path();
            match action {
                "context.run" => self.run_current_file(),
                "context.rename" => if let Some(path) = path { let input = path.file_name().unwrap_or_default().to_string_lossy().into_owned(); self.popup = Popup::RenameFile { path, input, just_opened: true }; },
                "context.delete" => if let Some(path) = path { self.popup = Popup::Confirm { message: crate::i18n::fmt("Delete '{}'? This cannot be undone.", &[&path.display()]), kind: ConfirmKind::Delete(path) }; },
                "context.diff" => if let Some(rel) = rel { self.open_diff_tab(&rel); },
                "context.rollback" => if let Some(rel) = rel { self.popup = Popup::Confirm { message: crate::i18n::fmt("Discard all local changes to '{}'? This cannot be undone.", &[&rel]), kind: ConfirmKind::Rollback(rel) }; },
                "context.stage" | "context.unstage" => if let Some(rel) = rel { let stage = action == "context.stage"; self.save_doc(idx); self.git_run_sync(if stage { "Stage" } else { "Unstage" }, move |repo| if stage { repo.stage(&[rel]).map(|_| String::new()) } else { repo.unstage(&[rel]).map(|_| String::new()) }); },
                action => self.run_command(ctx, action),
            }
        }
        if let Some(off) = out.ctrl_click {
            self.show_doc(idx);
            let doc = &mut self.docs[idx];
            doc.ed.set_caret(off, false);
            if let Some(url) = crate::editor_view::link_at(doc.ed.text(), off) {
                crate::input::open_url(&url);
            } else {
                self.goto_declaration(None);
            }
        }
        out.clicked
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for p in dropped {
            if p.is_dir() {
                self.request_open_project(&p);
            } else {
                self.open_file(&p);
            }
        }
    }

    fn process_screenshots(&mut self, ctx: &egui::Context) {
        let _ = (ctx, CornerRadius::ZERO);
    }
}
