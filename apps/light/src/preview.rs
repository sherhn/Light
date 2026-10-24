//! The rendered preview of Markdown and reStructuredText documents: the document model of `light-markup` drawn with
//! egui (headings, paragraphs with links, lists, tables, code with the editor's colours, quotes, admonitions,
//! images), and the bookkeeping that keeps it scrolled together with the code next to it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use light_markup::{Align as TableAlign, Block, BlockKind, Definition, Document, Inline, Markup};
use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, RichText, Sense, Stroke, TextureHandle, TextureOptions, Vec2, text::LayoutJob};
use lisp_syntax::highlight::Span;

use crate::fonts;
use crate::theme::Palette;

/// What a Markdown or reStructuredText tab shows: the code, the code with the rendered document beside it, or only the
/// rendered document (the three buttons of the editor's top right corner).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewMode {
    Code,
    Split,
    Preview,
}

impl PreviewMode {
    pub fn name(self) -> &'static str {
        match self {
            PreviewMode::Code => "code",
            PreviewMode::Split => "split",
            PreviewMode::Preview => "preview",
        }
    }

    pub fn from_name(s: &str) -> Option<PreviewMode> {
        Some(match s {
            "code" => PreviewMode::Code,
            "split" => PreviewMode::Split,
            "preview" => PreviewMode::Preview,
            _ => return None,
        })
    }
}

/// The markup a language's files are previewed as: what its module says, or (for modules from before the preview
/// existed) what its id says.
pub fn markup_of(lang: &light_lang::Language) -> Option<Markup> {
    lang.preview().and_then(Markup::from_name).or(match lang.id() {
        "markdown" => Some(Markup::Markdown),
        "rst" => Some(Markup::Rst),
        _ => None,
    })
}

#[derive(Default)]
pub struct PreviewState {
    /// This tab's choice; `None`: whatever the settings say.
    pub mode: Option<PreviewMode>,
    parsed: Option<(u64, Markup, Arc<Document>)>,
    /// Scroll offset of the last frame (points from the top of the document).
    pub scroll: f32,
    /// Scroll to this offset in the next frame.
    pub force_scroll: Option<f32>,
    /// Source line and vertical position of every top-level block, from the last frame.
    pub blocks_y: Vec<(usize, f32)>,
    pub content_h: f32,
    pub anchors: HashMap<String, f32>,
    /// A link to `#heading` was clicked: scroll there once its position is known.
    pub pending_anchor: Option<String>,
    images: HashMap<PathBuf, Option<TextureHandle>>,
    code: HashMap<u64, Vec<Span>>,
    images_rev: u64,
    /// Where each side was when the other was last made to follow it (editor: first visible line, preview: offset).
    pub last_editor_top: f32,
    pub last_preview_y: f32,
    /// The document's revision when the two sides were last compared.
    pub last_rev: u64,
    /// Scroll to this source line as soon as the layout is known (after switching from the code).
    pub pending_line: Option<f32>,
}

impl PreviewState {
    /// The document for `text`, parsed again only when the text changed.
    pub fn document(&mut self, text: &str, rev: u64, markup: Markup) -> Arc<Document> {
        if let Some((r, m, d)) = &self.parsed
            && *r == rev
            && *m == markup
        {
            return d.clone();
        }
        let doc = Arc::new(markup.parse(text));
        self.parsed = Some((rev, markup, doc.clone()));
        doc
    }

    /// Vertical position of the (fractional) source line `line`.
    pub fn line_to_y(&self, line: f32) -> Option<f32> {
        let b = &self.blocks_y;
        if b.is_empty() {
            return None;
        }
        let i = b.partition_point(|(l, _)| (*l as f32) <= line).saturating_sub(1);
        let (l0, y0) = b[i];
        let (l1, y1) = b.get(i + 1).copied().unwrap_or((l0 + 1, self.content_h.max(y0)));
        if l1 <= l0 {
            return Some(y0);
        }
        Some(y0 + ((line - l0 as f32) / (l1 - l0) as f32).clamp(0.0, 1.0) * (y1 - y0))
    }

    /// The (fractional) source line shown at vertical position `y`.
    pub fn y_to_line(&self, y: f32) -> Option<f32> {
        let b = &self.blocks_y;
        if b.is_empty() {
            return None;
        }
        let i = b.partition_point(|(_, by)| *by <= y).saturating_sub(1);
        let (l0, y0) = b[i];
        let Some(&(l1, y1)) = b.get(i + 1) else { return Some(l0 as f32) };
        if y1 <= y0 {
            return Some(l0 as f32);
        }
        Some(l0 as f32 + ((y - y0) / (y1 - y0)).clamp(0.0, 1.0) * (l1 - l0) as f32)
    }
}

pub struct Env<'a> {
    pub pal: &'a Palette,
    /// Where relative image paths and links start.
    pub base_dir: Option<&'a Path>,
    pub font_size: f32,
    /// Highlight `code` written in the language named `lang`.
    pub highlight: &'a dyn Fn(&str, &str) -> Vec<Span>,
    /// Revision of the document (loaded images are dropped when it changes).
    pub rev: u64,
}

#[derive(Default)]
pub struct Out {
    /// A link was clicked.
    pub link: Option<String>,
}

struct Cx<'a, 'b> {
    env: &'a Env<'b>,
    st: &'a mut PreviewState,
    out: Out,
    anchors: HashMap<String, f32>,
    origin_y: f32,
    id: u64,
}

const MAX_W: f32 = 860.0;

pub fn show(ui: &mut egui::Ui, env: &Env, doc: &Document, st: &mut PreviewState) -> Out {
    let pal = env.pal;
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, CornerRadius::ZERO, pal.editor_bg);
    if st.images_rev != env.rev {
        st.images.clear();
        st.images_rev = env.rev;
    }
    if let Some(l) = st.pending_line {
        match st.line_to_y(l) {
            Some(y) => {
                st.force_scroll = Some(y);
                st.pending_line = None;
            }
            None => ui.ctx().request_repaint(),
        }
    }
    if let Some(a) = st.pending_anchor.take() {
        match st.anchors.get(&a) {
            Some(y) => st.force_scroll = Some((*y - 12.0).max(0.0)),
            None => st.pending_anchor = Some(a),
        }
    }
    let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("preview_scroll");
    let forced = st.force_scroll.take();
    if let Some(y) = forced {
        area = area.vertical_scroll_offset(y);
    }
    let id = ui.scope_id().value();
    let mut cx = Cx { env, st, out: Out::default(), anchors: HashMap::new(), origin_y: 0.0, id };
    let mut ys: Vec<(usize, f32)> = vec![];
    let res = area.show(ui, |ui| {
        cx.origin_y = ui.cursor().min.y;
        let pad = ((ui.available_width() - MAX_W) / 2.0).max(28.0);
        ui.add_space(24.0);
        ui.horizontal_top(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_max_width((ui.available_width() - pad).clamp(120.0, MAX_W));
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 0.0);
                let mut first = true;
                for b in &doc.blocks {
                    if !first {
                        ui.add_space(gap(&b.kind));
                    }
                    first = false;
                    ys.push((b.line, ui.cursor().min.y - cx.origin_y));
                    block(ui, &mut cx, b);
                }
            });
        });
        ui.add_space(full.height() * 0.5);
    });
    let Cx { out, anchors, .. } = cx;
    st.anchors = anchors;
    st.blocks_y = ys;
    st.scroll = res.state.clamped_offset().y;
    st.content_h = res.content_size.y;
    if forced.is_some() {
        // the area may have clamped the offset: this is where it really is, not a move of the user's
        st.last_preview_y = st.scroll;
    }
    out
}

/// The space above a block.
fn gap(kind: &BlockKind) -> f32 {
    match kind {
        BlockKind::Heading { level, .. } => if *level <= 2 { 26.0 } else { 20.0 },
        _ => 14.0,
    }
}

fn base(cx: &Cx) -> f32 {
    (cx.env.font_size + 1.0).clamp(13.0, 22.0)
}

fn block(ui: &mut egui::Ui, cx: &mut Cx, b: &Block) {
    let pal = cx.env.pal;
    match &b.kind {
        BlockKind::Heading { level, inlines, anchor } => {
            let y = ui.cursor().min.y - cx.origin_y;
            cx.anchors.insert(anchor.clone(), y);
            let size = base(cx) * [1.9, 1.5, 1.25, 1.1, 1.0, 0.92][(*level as usize - 1).min(5)];
            let color = if *level == 6 { pal.text_dim } else { pal.text };
            text(ui, cx, inlines, size, color, true);
            if *level <= 2 {
                ui.add_space(6.0);
                let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::ZERO, pal.border);
            }
        }
        BlockKind::Paragraph(inlines) => {
            if is_image_row(inlines) {
                image_row(ui, cx, inlines);
            } else {
                let size = base(cx);
                text(ui, cx, inlines, size, pal.text, false);
            }
        }
        BlockKind::Code { lang, text } => code_block(ui, cx, lang, text, b.line),
        BlockKind::List { ordered, start, items } => {
            let size = base(cx);
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    ui.add_space(5.0);
                }
                ui.horizontal_top(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(if *ordered { 30.0 } else { 26.0 }, size * 1.6), Sense::hover());
                    let c = r.center();
                    match (item.checked, *ordered) {
                        (Some(done), _) => {
                            let box_r = egui::Rect::from_center_size(egui::pos2(r.max.x - 12.0, c.y), Vec2::splat(size * 0.95));
                            ui.painter().rect_stroke(box_r, CornerRadius::same(4), Stroke::new(1.3, if done { pal.accent } else { pal.text_dim }), egui::StrokeKind::Inside);
                            if done {
                                ui.painter().rect_filled(box_r.shrink(1.3), CornerRadius::same(3), pal.accent);
                                crate::icons::paint(ui.painter(), box_r.shrink(2.0), crate::icons::Icon::Check, pal.text_on_accent);
                            }
                        }
                        (None, true) => {
                            ui.painter().text(egui::pos2(r.max.x - 6.0, c.y), egui::Align2::RIGHT_CENTER, format!("{}.", start + n as u64), fonts::ui(size), pal.text_dim);
                        }
                        (None, false) => {
                            ui.painter().circle_filled(egui::pos2(r.max.x - 10.0, c.y), 2.6, pal.text_dim);
                        }
                    }
                    ui.vertical(|ui| {
                        let mut first = true;
                        for b in &item.blocks {
                            if !first {
                                ui.add_space(8.0);
                            }
                            first = false;
                            block(ui, cx, b);
                        }
                    });
                });
            }
        }
        BlockKind::Quote(blocks) => {
            let r = ui.horizontal_top(|ui| {
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    let mut first = true;
                    for b in blocks {
                        if !first {
                            ui.add_space(10.0);
                        }
                        first = false;
                        block(ui, cx, b);
                    }
                });
            });
            let rect = r.response.rect;
            ui.painter().rect_filled(egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + 3.0, rect.max.y)), CornerRadius::same(2), pal.text_dim.gamma_multiply(0.55));
        }
        BlockKind::Admonition { kind, title, blocks } => admonition(ui, cx, kind, title, blocks),
        BlockKind::Rule => {
            ui.add_space(4.0);
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::ZERO, pal.border);
            ui.add_space(4.0);
        }
        BlockKind::Table { align, header, rows } => table(ui, cx, align, header, rows, b.line),
        BlockKind::Definitions(defs) => definitions(ui, cx, defs),
    }
}

fn definitions(ui: &mut egui::Ui, cx: &mut Cx, defs: &[Definition]) {
    let size = base(cx);
    let color = cx.env.pal.text;
    for (n, d) in defs.iter().enumerate() {
        if n > 0 {
            ui.add_space(12.0);
        }
        text(ui, cx, &d.term, size, color, true);
        ui.horizontal_top(|ui| {
            ui.add_space(26.0);
            ui.vertical(|ui| {
                let mut first = true;
                for b in &d.blocks {
                    if !first {
                        ui.add_space(8.0);
                    }
                    first = false;
                    block(ui, cx, b);
                }
            });
        });
    }
}

fn admonition(ui: &mut egui::Ui, cx: &mut Cx, kind: &str, title: &str, blocks: &[Block]) {
    let pal = cx.env.pal;
    let (color, label) = match kind {
        "warning" | "caution" | "attention" | "danger" | "error" => (pal.warning, kind),
        "tip" | "hint" | "important" => (pal.ok, kind),
        "seealso" => (pal.blue, "see also"),
        _ => (pal.blue, kind),
    };
    let label = if title.is_empty() { capitalize(label) } else { title.to_string() };
    egui::Frame::new().fill(color.gamma_multiply(0.10)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
        ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, label, fonts::semibold(base(cx) * 0.95), color);
        ui.add_space(4.0);
        let mut first = true;
        for b in blocks {
            if !first {
                ui.add_space(8.0);
            }
            first = false;
            block(ui, cx, b);
        }
    });
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn table(ui: &mut egui::Ui, cx: &mut Cx, align: &[TableAlign], header: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>], line: usize) {
    let pal = cx.env.pal;
    let size = base(cx) * 0.96;
    let cols = align.len().max(1);
    let col_w = (ui.available_width() / cols as f32 - 24.0).clamp(90.0, MAX_W);
    egui::Frame::new().stroke(Stroke::new(1.0, pal.border)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(2)).show(ui, |ui| {
        egui::Grid::new(("md_table", cx.id, line)).striped(true).spacing(Vec2::new(24.0, 9.0)).min_col_width(40.0).show(ui, |ui| {
            let put = |ui: &mut egui::Ui, cx: &mut Cx, row: &[Vec<Inline>], bold: bool| {
                for (c, cell) in row.iter().enumerate() {
                    let a = align.get(c).copied().unwrap_or_default();
                    let layout = match a {
                        TableAlign::Left => egui::Layout::top_down(egui::Align::Min),
                        TableAlign::Center => egui::Layout::top_down(egui::Align::Center),
                        TableAlign::Right => egui::Layout::top_down(egui::Align::Max),
                    };
                    ui.with_layout(layout, |ui| {
                        ui.set_max_width(col_w);
                        text(ui, cx, cell, size, pal.text, bold);
                    });
                }
                ui.end_row();
            };
            if !header.is_empty() {
                put(ui, cx, header, true);
            }
            for r in rows {
                put(ui, cx, r, false);
            }
        });
    });
}

fn code_block(ui: &mut egui::Ui, cx: &mut Cx, lang: &str, code: &str, line: usize) {
    let pal = cx.env.pal;
    let size = (base(cx) * 0.92).round();
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (lang, code).hash(&mut h);
        h.finish()
    };
    if cx.st.code.len() > 300 {
        cx.st.code.clear();
    }
    let spans = cx.st.code.entry(key).or_insert_with(|| if lang.is_empty() { vec![] } else { (cx.env.highlight)(lang, code) }).clone();
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let font = FontId::monospace(size);
    let mut pos = 0;
    let push = |job: &mut LayoutJob, from: usize, to: usize, color: Color32| {
        if to > from {
            job.append(&code[from..to], 0.0, egui::TextFormat { line_height: Some((size * 1.5).round()), ..egui::TextFormat::simple(font.clone(), color) });
        }
    };
    for s in &spans {
        if s.start < pos || s.end > code.len() || !code.is_char_boundary(s.start) || !code.is_char_boundary(s.end) {
            continue;
        }
        push(&mut job, pos, s.start, pal.plain);
        push(&mut job, s.start, s.end, pal.syntax(s.class, s.depth, false));
        pos = s.end;
    }
    push(&mut job, pos, code.len(), pal.plain);
    egui::Frame::new().fill(pal.input_bg).corner_radius(CornerRadius::same(8)).stroke(Stroke::new(1.0, pal.border)).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if !lang.is_empty() {
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 0.0), Sense::hover());
            ui.painter().text(egui::pos2(r.max.x - 2.0, r.min.y - 2.0), egui::Align2::RIGHT_TOP, lang, fonts::ui(11.0), pal.text_dim);
        }
        egui::ScrollArea::horizontal().id_salt(("md_code", cx.id, line)).auto_shrink([false, true]).show(ui, |ui| {
            ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Extend));
        });
    });
}

// ---------------------------------------------------------------------- inline text

#[derive(Clone, Default)]
struct Fmt {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

/// What a run of text starts from: its size, its colour, and whether all of it is bold (a heading).
#[derive(Clone, Copy)]
struct Base {
    size: f32,
    color: Color32,
    bold: bool,
}

struct JobOut {
    job: LayoutJob,
    /// Char ranges of the links in the text, and where they go.
    links: Vec<(usize, usize, String, String)>,
    chars: usize,
}

fn format_for(cx: &Cx, base: Base, f: &Fmt) -> egui::TextFormat {
    let pal = cx.env.pal;
    let Base { size, color, .. } = base;
    let bold = f.bold || base.bold;
    let mut tf = egui::TextFormat {
        font_id: if f.code { FontId::monospace((size * 0.92).round()) } else if bold { fonts::semibold(size) } else { fonts::ui(size) },
        color,
        italics: f.italic,
        line_height: Some((size * 1.62).round()),
        ..Default::default()
    };
    if f.code {
        tf.background = pal.input_bg;
        tf.color = if f.link { pal.link } else { pal.text };
    }
    if f.link {
        tf.color = pal.link;
        tf.underline = Stroke::new(1.0, pal.link.gamma_multiply(0.55));
    }
    if f.strike {
        tf.strikethrough = Stroke::new(1.0, color);
    }
    tf
}

fn push_inlines(cx: &Cx, out: &mut JobOut, inlines: &[Inline], base: Base, f: &Fmt, link: Option<(&str, &str)>) {
    for i in inlines {
        match i {
            Inline::Text(t) => append(cx, out, t, base, f, link),
            Inline::Code(t) => append(cx, out, t, base, &Fmt { code: true, ..f.clone() }, link),
            Inline::Emph(c) => push_inlines(cx, out, c, base, &Fmt { italic: true, ..f.clone() }, link),
            Inline::Strong(c) => push_inlines(cx, out, c, base, &Fmt { bold: true, ..f.clone() }, link),
            Inline::Strike(c) => push_inlines(cx, out, c, base, &Fmt { strike: true, ..f.clone() }, link),
            Inline::Link { url, title, children } => push_inlines(cx, out, children, base, &Fmt { link: true, ..f.clone() }, Some((url, title))),
            Inline::Image { alt, src, .. } => {
                // an image in running text: its description, as a link to it
                let shown = if alt.is_empty() { src.rsplit('/').next().unwrap_or(src).to_string() } else { alt.clone() };
                append(cx, out, &format!("[{shown}]"), base, f, link);
            }
            Inline::SoftBreak => append(cx, out, " ", base, f, link),
            Inline::HardBreak => append(cx, out, "\n", base, f, link),
        }
    }
}

fn append(cx: &Cx, out: &mut JobOut, t: &str, base: Base, f: &Fmt, link: Option<(&str, &str)>) {
    if t.is_empty() {
        return;
    }
    let n = t.chars().count();
    out.job.append(t, 0.0, format_for(cx, base, f));
    if let Some((url, title)) = link {
        match out.links.last_mut() {
            Some(l) if l.1 == out.chars && l.2 == url => l.1 += n,
            _ => out.links.push((out.chars, out.chars + n, url.to_string(), title.to_string())),
        }
    }
    out.chars += n;
}

/// A run of inlines as wrapped, selectable text; clicks on links go to `cx.out.link`.
fn text(ui: &mut egui::Ui, cx: &mut Cx, inlines: &[Inline], size: f32, color: Color32, bold: bool) {
    let mut out = JobOut { job: LayoutJob::default(), links: vec![], chars: 0 };
    push_inlines(cx, &mut out, inlines, Base { size, color, bold }, &Fmt::default(), None);
    if out.job.is_empty() {
        return;
    }
    out.job.wrap.max_width = ui.available_width();
    let galley = ui.painter().layout_job(out.job.clone());
    let resp = ui.add(egui::Label::new(out.job).wrap_mode(egui::TextWrapMode::Wrap).sense(Sense::click()));
    if out.links.is_empty() {
        return;
    }
    if let Some(p) = resp.hover_pos() {
        let idx = galley.cursor_from_pos(p - resp.rect.min).index.0;
        if let Some((_, _, url, title)) = out.links.iter().find(|(a, b, _, _)| idx >= *a && idx < *b) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            egui::Tooltip::always_open(ui.ctx().clone(), resp.layer_id, resp.id.with("link"), egui::PopupAnchor::Pointer).show(|ui| {
                ui.label(RichText::new(if title.is_empty() { url.as_str() } else { title.as_str() }).size(12.0));
            });
            if resp.clicked() {
                cx.out.link = Some(url.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------- images

fn has_image(i: &Inline) -> bool {
    match i {
        Inline::Image { .. } => true,
        Inline::Link { children, .. } => children.iter().any(has_image),
        _ => false,
    }
}

fn is_image_row(inl: &[Inline]) -> bool {
    fn plain(i: &Inline) -> bool {
        match i {
            Inline::Image { .. } | Inline::SoftBreak | Inline::HardBreak => true,
            Inline::Text(t) => t.trim().is_empty(),
            Inline::Link { children, .. } => children.iter().all(plain),
            _ => false,
        }
    }
    inl.iter().all(plain) && inl.iter().any(has_image)
}

fn image_row(ui: &mut egui::Ui, cx: &mut Cx, inl: &[Inline]) {
    fn collect<'a>(inl: &'a [Inline], link: Option<&'a str>, out: &mut Vec<(&'a str, &'a str, Option<&'a str>)>) {
        for i in inl {
            match i {
                Inline::Image { src, alt, .. } => out.push((src, alt, link)),
                Inline::Link { url, children, .. } => collect(children, Some(url), out),
                _ => {}
            }
        }
    }
    let mut items = vec![];
    collect(inl, None, &mut items);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for (src, alt, link) in items {
            if let Some(url) = image(ui, cx, src, alt, link) {
                cx.out.link = Some(url);
            }
        }
    });
}

fn resolve(base: Option<&Path>, src: &str) -> Option<PathBuf> {
    if src.contains("://") && !src.starts_with("file://") {
        return None;
    }
    let s = src.strip_prefix("file:///").or_else(|| src.strip_prefix("file://")).unwrap_or(src);
    let s = s.split(['?', '#']).next().unwrap_or(s);
    let s = percent_decode(s);
    let p = PathBuf::from(&s);
    if p.is_absolute() { Some(p) } else { base.map(|b| b.join(p)) }
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Draw an image (a PNG next to the document) or, for anything else, a chip with its description. Returns the link
/// to follow when it was clicked.
fn image(ui: &mut egui::Ui, cx: &mut Cx, src: &str, alt: &str, link: Option<&str>) -> Option<String> {
    let pal = cx.env.pal;
    let path = resolve(cx.env.base_dir, src).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")));
    let tex = path.as_ref().and_then(|p| {
        cx.st
            .images
            .entry(p.clone())
            .or_insert_with(|| {
                let bytes = std::fs::read(p).ok()?;
                let (w, h, rgba) = crate::brand::decode_png(&bytes)?;
                let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                Some(ui.ctx().load_texture(format!("preview:{}", p.display()), img, TextureOptions::LINEAR))
            })
            .clone()
    });
    let resp = match tex {
        Some(t) => {
            let nat = t.size_vec2() / ui.ctx().pixels_per_point().max(1.0);
            let max_w = ui.available_width().min(MAX_W);
            let scale = (max_w / nat.x).min(1.0).min(520.0 / nat.y.max(1.0));
            ui.add(egui::Image::new(&t).fit_to_exact_size(nat * scale).sense(Sense::click()))
        }
        None => {
            let shown = if alt.is_empty() { src.rsplit('/').next().unwrap_or(src) } else { alt };
            let g = ui.painter().layout_no_wrap(shown.to_string(), fonts::ui(12.0), pal.text);
            let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 16.0, 22.0), Sense::click());
            ui.painter().rect_filled(r, CornerRadius::same(6), pal.hover);
            ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
            ui.painter().galley(egui::pos2(r.min.x + 8.0, r.center().y - g.size().y / 2.0), g, pal.text);
            resp
        }
    };
    let resp = if link.is_some() { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    (resp.clicked()).then(|| link.map(str::to_string)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_have_names() {
        for m in [PreviewMode::Code, PreviewMode::Split, PreviewMode::Preview] {
            assert_eq!(PreviewMode::from_name(m.name()), Some(m));
        }
        assert_eq!(PreviewMode::from_name("x"), None);
    }

    #[test]
    fn scroll_positions_map_both_ways() {
        let mut st = PreviewState::default();
        assert_eq!(st.line_to_y(3.0), None);
        st.blocks_y = vec![(0, 0.0), (4, 100.0), (10, 400.0)];
        st.content_h = 500.0;
        assert_eq!(st.line_to_y(0.0), Some(0.0));
        assert_eq!(st.line_to_y(2.0), Some(50.0));
        assert_eq!(st.line_to_y(4.0), Some(100.0));
        assert_eq!(st.line_to_y(7.0), Some(250.0));
        assert_eq!(st.y_to_line(50.0), Some(2.0));
        assert_eq!(st.y_to_line(250.0), Some(7.0));
        // past the last block
        assert!(st.line_to_y(50.0).unwrap() <= 500.0);
    }

    #[test]
    fn relative_images_and_escapes_resolve() {
        let base = Path::new("/docs");
        assert_eq!(resolve(Some(base), "a%20b.png?raw=1"), Some(PathBuf::from("/docs/a b.png")));
        assert_eq!(resolve(Some(base), "https://x.org/a.png"), None);
        assert_eq!(resolve(None, "a.png"), None);
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn rows_of_images_are_told_from_text() {
        let img = Inline::Image { src: "a.png".into(), alt: "A".into(), title: String::new() };
        assert!(is_image_row(&[img.clone(), Inline::Text(" ".into()), Inline::Link { url: "u".into(), title: String::new(), children: vec![img.clone()] }]));
        assert!(!is_image_row(&[img, Inline::Text("caption".into())]));
        assert!(!is_image_row(&[Inline::Text("text".into())]));
    }
}
