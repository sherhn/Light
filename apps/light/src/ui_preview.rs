//! Markdown and reStructuredText tabs: the three buttons (code, code and preview, preview) and the panes they switch
//! between, with the scrolling of both sides kept together.

use std::path::{Path, PathBuf};

use light_markup::Markup;
use eframe::egui::{self, CornerRadius, Rect, Sense, Vec2, pos2, vec2};
use lisp_syntax::highlight::Span;

use crate::app::App;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::preview::{self, Env, PreviewMode};
use crate::widgets::CRUMB_H;

/// Width of the line between the code and the preview.
const GAP: f32 = 1.0;

/// Fenced code names that are Lisp (read by the Lisp reader, not by a module's grammar).
fn lisp_dialect(name: &str) -> Option<lisp_syntax::Dialect> {
    Some(match name {
        "lisp" | "common-lisp" | "commonlisp" | "cl" => lisp_syntax::Dialect::CommonLisp,
        "scheme" | "scm" | "racket" => lisp_syntax::Dialect::Scheme,
        "clojure" | "clj" | "cljs" => lisp_syntax::Dialect::Clojure,
        "elisp" | "emacs-lisp" | "el" => lisp_syntax::Dialect::EmacsLisp,
        _ => return None,
    })
}

/// The language a code fence names: by id, by file extension, by a few usual names.
fn fence_language(langs: &light_lang::LanguageSet, name: &str) -> Option<std::sync::Arc<light_lang::Language>> {
    let name = name.to_ascii_lowercase();
    let alias = match name.as_str() {
        "sh" | "bash" | "zsh" | "console" | "shell-session" => "shell",
        "c++" | "cc" | "hpp" => "cpp",
        "asm" | "assembly" => "nasm",
        "py" | "python3" => "python",
        "md" => "markdown",
        "rst" | "restructuredtext" => "rst",
        n => n,
    };
    langs.by_id(alias).or_else(|| langs.find(Path::new(&format!("x.{alias}")), None))
}

impl App {
    /// The markup of the document in tab `idx` when it has a preview.
    pub fn doc_markup(&self, idx: usize) -> Option<Markup> {
        let d = self.docs.get(idx).filter(|d| d.diff.is_none())?;
        d.ed.language().and_then(|l| preview::markup_of(l))
    }

    pub fn default_preview_mode(&self) -> PreviewMode {
        PreviewMode::from_name(&self.settings.markup_preview).unwrap_or(PreviewMode::Split)
    }

    /// What tab `idx` shows: always the code for files without a preview.
    pub fn preview_mode(&self, idx: usize) -> PreviewMode {
        if self.doc_markup(idx).is_none() {
            return PreviewMode::Code;
        }
        self.docs[idx].preview.mode.unwrap_or_else(|| self.default_preview_mode())
    }

    /// Only the rendered document is on screen: nothing receives typing.
    pub fn editor_hidden(&self) -> bool {
        self.docs.get(self.active).is_some() && self.preview_mode(self.active) == PreviewMode::Preview
    }

    /// Switch tab `idx` (and, as the new default, every Markdown and reStructuredText tab) to `mode`.
    pub fn set_preview_mode(&mut self, idx: usize, mode: PreviewMode) {
        if self.doc_markup(idx).is_none() {
            return;
        }
        let old = self.preview_mode(idx);
        let d = &mut self.docs[idx];
        match (old, mode) {
            (PreviewMode::Preview, PreviewMode::Code | PreviewMode::Split) => d.view.scroll_to_line_f = d.preview.y_to_line(d.preview.scroll),
            (PreviewMode::Code, PreviewMode::Split | PreviewMode::Preview) => d.preview.pending_line = Some(d.view.top_line_f),
            _ => {}
        }
        d.preview.mode = Some(mode);
        self.settings.markup_preview = mode.name().to_string();
        self.save_settings();
        self.editor_focus = mode != PreviewMode::Preview;
    }

    /// The three buttons at the right end of the breadcrumbs row `row`.
    pub fn draw_preview_switch(&mut self, ui: &mut egui::Ui, row: Rect, idx: usize) {
        if self.doc_markup(idx).is_none() {
            return;
        }
        let pal = self.pal;
        let cur = self.preview_mode(idx);
        let buttons = [(PreviewMode::Code, Icon::Code, "Editor Only"), (PreviewMode::Split, Icon::Columns, "Editor and Preview"), (PreviewMode::Preview, Icon::Preview, "Preview Only")];
        let size = CRUMB_H - 8.0;
        let total = size * 3.0 + 8.0;
        let mut x = row.max.x - total - 10.0;
        let mut chosen = None;
        for (mode, icon, tip) in buttons {
            let r = Rect::from_min_size(pos2(x, row.center().y - size / 2.0), vec2(size, size));
            let resp = ui.interact(r, ui.scope_id().with(("preview_mode", idx, mode.name())), Sense::click());
            let on = cur == mode;
            if on {
                ui.painter().rect_filled(r, CornerRadius::same(7), pal.accent.gamma_multiply(0.22));
            } else if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(7), pal.hover);
            }
            icons::paint(ui.painter(), r.shrink(6.0), icon, if on { pal.accent } else { pal.text_dim });
            if resp.on_hover_text(tr(tip)).clicked() {
                chosen = Some(mode);
            }
            x += size + 4.0;
        }
        if let Some(m) = chosen {
            self.set_preview_mode(idx, m);
        }
    }

    /// Draw the rendered document of tab `idx` in the space left in `ui`.
    pub fn draw_preview(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context, idx: usize, markup: Markup) {
        let mut st = std::mem::take(&mut self.docs[idx].preview);
        let (rev, base): (u64, Option<PathBuf>) = {
            let d = &self.docs[idx];
            (d.ed.revision(), d.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)))
        };
        let doc = {
            let d = &self.docs[idx];
            st.document(d.ed.text(), rev, markup)
        };
        let langs = &self.mods.langs;
        let lisp = self.mods.lisp;
        let hl = |lang: &str, code: &str| -> Vec<Span> {
            let l = lang.to_ascii_lowercase();
            if lisp && let Some(d) = lisp_dialect(&l) {
                let p = lisp_syntax::parse(code, d);
                return lisp_syntax::highlight::highlight(code, &p);
            }
            fence_language(langs, &l).map(|lang| lang.highlight(code)).unwrap_or_default()
        };
        let env = Env { pal: &self.pal, base_dir: base.as_deref(), font_size: self.settings.font_size, highlight: &hl, rev };
        let out = preview::show(ui, &env, &doc, &mut st);
        self.docs[idx].preview = st;
        if let Some(link) = out.link {
            self.follow_link(idx, &link);
        }
    }

    /// A link of a rendered document was clicked: a heading of it, a file next to it, or a web address.
    pub fn follow_link(&mut self, idx: usize, link: &str) {
        if let Some(anchor) = link.strip_prefix('#') {
            self.docs[idx].preview.pending_anchor = Some(preview::percent_decode(anchor).to_lowercase());
            return;
        }
        if link.contains("://") || link.starts_with("mailto:") {
            crate::input::open_url(link);
            return;
        }
        let (file, anchor) = match link.split_once('#') {
            Some((f, a)) => (f, Some(a)),
            None => (link, None),
        };
        let base = self.docs[idx].path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf));
        let Some(path) = base.map(|b| b.join(preview::percent_decode(file))).filter(|p| p.is_file()) else {
            self.toast(crate::i18n::fmt("Cannot find {}", &[&link]), crate::editor_view::Sev::Weak);
            return;
        };
        if self.open_file(&path)
            && let Some(a) = anchor
            && let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(path.as_path()))
        {
            self.docs[i].preview.pending_anchor = Some(a.to_lowercase());
        }
    }

    /// The code on the left, the rendered document on the right.
    pub fn draw_split_markup(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, idx: usize, focused: bool, own_view: bool, markup: Markup) -> bool {
        let pal = self.pal;
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        let w = ((full.width() - GAP) / 2.0).floor();
        let left = Rect::from_min_size(full.min, Vec2::new(w, full.height()));
        let right = Rect::from_min_size(pos2(full.min.x + w + GAP, full.min.y), Vec2::new(full.width() - w - GAP, full.height()));
        ui.painter().rect_filled(Rect::from_min_max(pos2(left.max.x, full.min.y), pos2(right.min.x, full.max.y)), CornerRadius::ZERO, pal.hover);
        let mut pane = ui.new_child(egui::UiBuilder::new().id_salt(("markup_code", idx)).max_rect(left).layout(egui::Layout::top_down(egui::Align::Min)));
        pane.set_clip_rect(left);
        let clicked = self.draw_editor(&mut pane, ctx, idx, focused, own_view);
        let mut view = ui.new_child(egui::UiBuilder::new().id_salt(("markup_view", idx)).max_rect(right).layout(egui::Layout::top_down(egui::Align::Min)));
        view.set_clip_rect(right);
        self.draw_preview(&mut view, ctx, idx, markup);
        self.sync_scroll(ctx, idx, right);
        clicked
    }

    /// Keep the code and the preview showing the same place: whichever moved last leads.
    fn sync_scroll(&mut self, ctx: &egui::Context, idx: usize, preview_rect: Rect) {
        let rev = self.docs[idx].ed.revision();
        let d = &mut self.docs[idx];
        let top = d.view.top_line_f;
        let pv = &mut d.preview;
        if pv.blocks_y.is_empty() {
            ctx.request_repaint();
            return;
        }
        let over_preview = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| preview_rect.contains(p));
        let editor_moved = (top - pv.last_editor_top).abs() > 0.05 || pv.last_rev != rev;
        let preview_moved = (pv.scroll - pv.last_preview_y).abs() > 1.5;
        pv.last_rev = rev;
        if preview_moved && (over_preview || !editor_moved) {
            if let Some(l) = pv.y_to_line(pv.scroll) {
                d.view.scroll_to_line_f = Some(l);
                pv.last_editor_top = l;
            }
            pv.last_preview_y = pv.scroll;
            ctx.request_repaint();
        } else if editor_moved {
            if let Some(y) = pv.line_to_y(top) {
                pv.force_scroll = Some(y);
                pv.last_preview_y = y;
                ctx.request_repaint();
            }
            pv.last_editor_top = top;
        }
    }
}
