//! Pictures in the editor. A raster image (PNG, JPEG, GIF, BMP, WebP, ICO) opens in a tab of its own; an SVG file is
//! text, so it opens as code with its drawing beside it (the same three modes as a Markdown document). Both are shown
//! by one widget: a chequered ground (so transparency is visible), the picture fitted to the pane or at a zoom the user
//! chose, dragged with the mouse, and a bar with its size and the zoom buttons.
//!
//! SVG is drawn by resvg in a worker thread, at the size it is shown (and again when the zoom changes), so a heavy
//! drawing never freezes the window and stays sharp when zoomed in.

use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;

use eframe::egui::{self, Align2, Color32, ColorImage, CornerRadius, Mesh, Pos2, Rect, Sense, Stroke, StrokeKind, TextureFilter, TextureHandle, TextureOptions, Vec2, pos2, vec2};
use resvg::{tiny_skia, usvg};

use crate::fonts;
use crate::i18n::{self, tr};
use crate::theme::Palette;

/// Largest picture file that opens (the decoded picture takes about four bytes a pixel, so this is a few hundred MB).
pub const MAX_PICTURE_BYTES: u64 = 64 * 1024 * 1024;

/// The sides of what an SVG is drawn into never go over this, whatever the zoom.
const MAX_SVG_SIDE: usize = 4096;
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 64.0;
/// Space kept around a picture that is fitted to the pane, in points.
const MARGIN: f32 = 20.0;
const BAR_H: f32 = 30.0;
/// From this zoom on a raster image shows its pixels as squares instead of smoothing them.
const PIXEL_ZOOM: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Raster,
    Svg,
}

/// What a file's name says it is, if it is a picture the IDE shows.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" => Kind::Raster,
        "svg" => Kind::Svg,
        _ => return None,
    })
}

// ---------------------------------------------------------------------- decoding

pub struct Raster {
    pub rgba: image::RgbaImage,
    /// "PNG", "JPEG"... (empty when the format is not one of the usual ones).
    pub format: &'static str,
}

/// Decode a raster image of any of the usual formats (told apart by its content, not by its name).
pub fn decode_raster(bytes: &[u8]) -> Result<Raster, String> {
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let format = match reader.format() {
        Some(image::ImageFormat::Png) => "PNG",
        Some(image::ImageFormat::Jpeg) => "JPEG",
        Some(image::ImageFormat::Gif) => "GIF",
        Some(image::ImageFormat::Bmp) => "BMP",
        Some(image::ImageFormat::WebP) => "WebP",
        Some(image::ImageFormat::Ico) => "ICO",
        _ => "",
    };
    let picture = reader.decode().map_err(|e| e.to_string())?;
    Ok(Raster { rgba: picture.into_rgba8(), format })
}

/// A picture too big for the graphics card, shrunk to fit it.
fn fit_side(rgba: image::RgbaImage, max_side: usize) -> image::RgbaImage {
    let (w, h) = rgba.dimensions();
    let longest = w.max(h) as usize;
    if longest <= max_side {
        return rgba;
    }
    let k = max_side as f32 / longest as f32;
    let (nw, nh) = (((w as f32 * k).floor() as u32).max(1), ((h as f32 * k).floor() as u32).max(1));
    image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Triangle)
}

fn color_image(rgba: &image::RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
}

// ---------------------------------------------------------------------- SVG

/// The fonts SVG text is drawn with: the ones bundled with the IDE, and the system's when the drawing has any text.
fn font_database(with_system: bool) -> Arc<usvg::fontdb::Database> {
    static BUNDLED: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    static SYSTEM: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    fn bundled_db() -> usvg::fontdb::Database {
        let mut db = usvg::fontdb::Database::new();
        db.load_font_data(fonts::UI.to_vec());
        db.load_font_data(fonts::UI_SEMIBOLD.to_vec());
        db.load_font_data(fonts::MONO.to_vec());
        db.set_sans_serif_family("Inter");
        db.set_serif_family("Inter");
        db.set_monospace_family("JetBrains Mono");
        db
    }
    if with_system {
        SYSTEM
            .get_or_init(|| {
                let mut db = bundled_db();
                db.load_system_fonts();
                // generic names the system lacks (`Arial`, `Times New Roman`) fall back to the bundled fonts
                db.set_sans_serif_family("Inter");
                db.set_serif_family("Inter");
                db.set_monospace_family("JetBrains Mono");
                Arc::new(db)
            })
            .clone()
    } else {
        BUNDLED.get_or_init(|| Arc::new(bundled_db())).clone()
    }
}

/// A drawing of an SVG.
pub struct Drawn {
    /// The size the SVG says it has, in pixels at a zoom of 100%.
    pub size: [f32; 2],
    pub image: ColorImage,
}

fn parse_svg(text: &str, dir: Option<&Path>) -> Result<usvg::Tree, String> {
    let mut options = usvg::Options { resources_dir: dir.map(Path::to_path_buf), font_family: "Inter".into(), ..usvg::Options::default() };
    // loading every font of the system takes a moment: only a drawing with text pays for it
    options.fontdb = font_database(text.contains("<text"));
    usvg::Tree::from_str(text, &options).map_err(|e| e.to_string())
}

/// Draw the SVG `text` at `scale` (device pixels per pixel of its own size); `dir` is where files it refers to are.
pub fn draw_svg(text: &str, dir: Option<&Path>, scale: f32, max_side: usize) -> Result<Drawn, String> {
    // a broken or hostile drawing must never take the editor down with it
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw_svg_inner(text, dir, scale, max_side))).unwrap_or_else(|_| Err("the drawing could not be rendered".into()))
}

fn draw_svg_inner(text: &str, dir: Option<&Path>, scale: f32, max_side: usize) -> Result<Drawn, String> {
    if text.trim().is_empty() {
        return Err("the file is empty".into());
    }
    let tree = parse_svg(text, dir)?;
    let (w, h) = (tree.size().width(), tree.size().height());
    if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
        return Err("the drawing has no size".into());
    }
    let scale = scale.max(0.001).min(max_side as f32 / w).min(max_side as f32 / h);
    let (pw, ph) = (((w * scale).ceil() as u32).max(1), ((h * scale).ceil() as u32).max(1));
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).ok_or("the drawing is too large")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(pw as f32 / w, ph as f32 / h), &mut pixmap.as_mut());
    Ok(Drawn { size: [w, h], image: ColorImage::from_rgba_premultiplied([pw as usize, ph as usize], pixmap.data()) })
}

/// The step of the scales an SVG is drawn at: a power of the square root of two, so that zooming smoothly does not
/// draw it again at every wheel click.
fn scale_step(zoom: f32) -> i32 {
    ((zoom.max(MIN_ZOOM).log2() * 2.0).ceil() as i32).clamp(-12, 14)
}

fn step_scale(step: i32) -> f32 {
    2f32.powf(step as f32 / 2.0)
}

/// What the SVG widget needs from the document it belongs to.
pub struct SvgIn<'a> {
    pub text: &'a str,
    /// Changes with every edit of the text.
    pub rev: u64,
    /// Where the files the drawing refers to are.
    pub dir: Option<&'a Path>,
}

/// What an SVG is drawn for: the revision of the text and the step of the scale.
type SvgKey = (u64, i32);

#[derive(Default)]
struct SvgJob {
    /// What the texture shows: the revision of the text and the step of the scale.
    shown: Option<SvgKey>,
    /// What the last frame asked for.
    wanted: Option<SvgKey>,
    running: Option<(SvgKey, mpsc::Receiver<Result<Drawn, String>>)>,
}

// ---------------------------------------------------------------------- the widget

static TEXTURE_IDS: AtomicU64 = AtomicU64::new(1);

pub struct ImageView {
    kind: Kind,
    /// `None`: fitted to the pane. Otherwise device pixels per pixel of the picture.
    pub zoom: Option<f32>,
    /// Where the picture's centre is, relative to the centre of the pane (points).
    pan: Vec2,
    /// Size of the picture in pixels at a zoom of 100%, once known.
    natural: Option<[f32; 2]>,
    texture: Option<TextureHandle>,
    /// A raster image waiting for its texture (decoded, not yet uploaded).
    waiting: Option<image::RgbaImage>,
    /// What the raster texture holds (kept to be uploaded again when the filter changes).
    uploaded: Option<Arc<ColorImage>>,
    nearest: bool,
    format: &'static str,
    bytes: u64,
    /// Why there is nothing (or nothing new) to show.
    pub error: Option<String>,
    job: SvgJob,
    name: String,
    /// The zoom of the last frame (what `zoom: None` came to), for the tests and the bar.
    pub shown_zoom: f32,
}

impl ImageView {
    fn blank(kind: Kind) -> ImageView {
        ImageView {
            kind,
            zoom: None,
            pan: Vec2::ZERO,
            natural: None,
            texture: None,
            waiting: None,
            uploaded: None,
            nearest: false,
            format: "",
            bytes: 0,
            error: None,
            job: SvgJob::default(),
            name: format!("picture-{}", TEXTURE_IDS.fetch_add(1, Ordering::Relaxed)),
            shown_zoom: 1.0,
        }
    }

    /// The view of an SVG document (drawn from the text it is given each frame).
    pub fn new_svg() -> ImageView {
        ImageView::blank(Kind::Svg)
    }

    /// The view of the raster image in `path`. A file that cannot be read or decoded gives a view that says why.
    pub fn open(path: &Path) -> ImageView {
        let mut v = ImageView::blank(Kind::Raster);
        v.load(path);
        v
    }

    /// Read the picture again (the file changed); the zoom and position stay.
    pub fn load(&mut self, path: &Path) {
        self.waiting = None;
        self.uploaded = None;
        self.texture = None;
        self.error = None;
        match std::fs::read(path).map_err(|e| e.to_string()).and_then(|bytes| {
            self.bytes = bytes.len() as u64;
            decode_raster(&bytes)
        }) {
            Ok(r) => {
                self.natural = Some([r.rgba.width() as f32, r.rgba.height() as f32]);
                self.format = r.format;
                self.waiting = Some(r.rgba);
            }
            Err(e) => {
                self.natural = None;
                self.error = Some(e);
            }
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Size of the picture in pixels at a zoom of 100%.
    pub fn natural(&self) -> Option<[f32; 2]> {
        self.natural
    }

    /// Is there a picture on screen (as opposed to an error or a picture still being drawn)?
    pub fn has_picture(&self) -> bool {
        self.texture.is_some()
    }

    /// Nothing more is going to change without the user: the picture (or why there is none) is on screen, and an
    /// SVG is drawn for the text and the zoom it has now.
    pub fn settled(&self) -> bool {
        let shown = self.texture.is_some() || self.error.is_some();
        match self.kind {
            Kind::Raster => shown && self.waiting.is_none(),
            Kind::Svg => shown && self.job.running.is_none() && self.job.shown == self.job.wanted,
        }
    }

    /// The text of the bar: what the picture is.
    pub fn info(&self) -> String {
        let Some([w, h]) = self.natural else { return String::new() };
        let size = format!("{} \u{d7} {} px", w.round(), h.round());
        match self.kind {
            Kind::Svg => format!("SVG \u{b7} {size}"),
            Kind::Raster => {
                let kind = if self.format.is_empty() { size } else { format!("{} \u{b7} {size}", self.format) };
                format!("{kind} \u{b7} {}", human_bytes(self.bytes))
            }
        }
    }

    fn fit_zoom(&self, pane: Vec2, ppp: f32) -> f32 {
        let Some([w, h]) = self.natural else { return 1.0 };
        let room = (pane - Vec2::splat(2.0 * MARGIN)).max(Vec2::splat(40.0));
        let z = (room.x * ppp / w).min(room.y * ppp / h);
        // a small raster image is shown as it is; a drawing grows to the pane
        (if self.kind == Kind::Raster { z.min(1.0) } else { z }).clamp(MIN_ZOOM, MAX_ZOOM)
    }

    /// Draw the picture in the space left in `ui`. `svg` is the text of the document (SVG only). `keys`: the keys
    /// `+`, `-`, `0` and `1` zoom (only where nothing else takes typing).
    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette, svg: Option<SvgIn>, keys: bool) {
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        let painter = ui.painter_at(full);
        painter.rect_filled(full, CornerRadius::ZERO, pal.editor_bg);
        let view = Rect::from_min_max(full.min, pos2(full.max.x, (full.max.y - BAR_H).max(full.min.y)));
        let bar = Rect::from_min_max(pos2(full.min.x, view.max.y), full.max);

        let fit = self.fit_zoom(view.size(), ppp);
        let zoom = self.zoom.unwrap_or(fit);
        self.shown_zoom = zoom;
        match self.kind {
            Kind::Raster => self.upload_raster(&ctx, zoom),
            Kind::Svg => {
                if let Some(input) = &svg {
                    self.drive_svg(&ctx, input, zoom);
                }
            }
        }
        // the picture may have just told its size: the fit follows
        let fit = self.fit_zoom(view.size(), ppp);
        let zoom = self.zoom.unwrap_or(fit);
        self.shown_zoom = zoom;

        let resp = ui.interact(view, ui.scope_id().with(("picture", &self.name)), Sense::click_and_drag());
        // dragging moves the picture
        if resp.dragged() {
            self.pan += resp.drag_delta();
        }
        // the wheel (and a pinch) zooms around the pointer
        if resp.hovered()
            && let Some(q) = ctx.input(|i| i.pointer.hover_pos())
        {
            let (scroll, pinch) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = pinch * (scroll * 0.0025).exp();
            if (factor - 1.0).abs() > 1e-4 {
                self.zoom_around(view, q, zoom, zoom * factor);
            }
        }
        if resp.double_clicked() {
            if self.zoom.is_some() {
                self.zoom = None;
                self.pan = Vec2::ZERO;
            } else {
                self.zoom_around(view, view.center(), zoom, 1.0);
            }
        }
        if keys && !ctx.text_edit_focused() {
            let (plus, minus, fit_key, one) = ctx.input(|i| {
                let plain = !i.modifiers.any();
                (plain && (i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)), plain && i.key_pressed(egui::Key::Minus), plain && i.key_pressed(egui::Key::Num0), plain && i.key_pressed(egui::Key::Num1))
            });
            if plus {
                self.zoom_around(view, view.center(), zoom, zoom * 1.25);
            } else if minus {
                self.zoom_around(view, view.center(), zoom, zoom / 1.25);
            } else if fit_key {
                self.zoom = None;
                self.pan = Vec2::ZERO;
            } else if one {
                self.zoom_around(view, view.center(), zoom, 1.0);
            }
        }
        let zoom = self.zoom.unwrap_or(fit);
        self.shown_zoom = zoom;

        // where the picture is
        let vp = painter.with_clip_rect(view);
        if let (Some([w, h]), Some(tex)) = (self.natural, self.texture.as_ref()) {
            let disp = vec2(w, h) * zoom / ppp;
            self.pan = clamp_pan(self.pan, disp, view.size());
            let r = Rect::from_center_size(view.center() + self.pan, disp);
            paint_chequers(&vp, r.intersect(view), r.min, pal.dark);
            vp.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, pal.border), StrokeKind::Outside);
            vp.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        } else if self.error.is_none() {
            vp.text(view.center(), Align2::CENTER_CENTER, tr("Loading..."), fonts::ui(13.0), pal.text_dim);
        }
        if let Some(e) = &self.error {
            self.paint_error(&vp, view, pal, e, self.texture.is_some());
        }
        self.paint_bar(ui, &painter, bar, pal, zoom, view);
    }

    /// Set the zoom to `to`, keeping the point of the picture under `anchor` where it is.
    fn zoom_around(&mut self, view: Rect, anchor: Pos2, from: f32, to: f32) {
        let to = to.clamp(MIN_ZOOM, MAX_ZOOM);
        let k = to / from.max(1e-6);
        let centre = view.center() + self.pan;
        self.pan = (anchor + (centre - anchor) * k) - view.center();
        self.zoom = Some(to);
    }

    fn upload_raster(&mut self, ctx: &egui::Context, zoom: f32) {
        if let Some(rgba) = self.waiting.take() {
            let rgba = fit_side(rgba, ctx.input(|i| i.max_texture_side));
            let image = Arc::new(color_image(&rgba));
            self.nearest = zoom >= PIXEL_ZOOM;
            self.texture = Some(ctx.load_texture(&self.name, image.clone(), raster_options(self.nearest)));
            self.uploaded = Some(image);
        } else if let (Some(tex), Some(image)) = (self.texture.as_mut(), self.uploaded.as_ref()) {
            let nearest = zoom >= PIXEL_ZOOM;
            if nearest != self.nearest {
                self.nearest = nearest;
                tex.set(image.clone(), raster_options(nearest));
            }
        }
    }

    /// Collect a finished drawing and start the next one when the text or the zoom asks for it.
    fn drive_svg(&mut self, ctx: &egui::Context, input: &SvgIn, zoom: f32) {
        if let Some((key, rx)) = self.job.running.take() {
            match rx.try_recv() {
                Ok(Ok(drawn)) => {
                    self.error = None;
                    self.natural = Some(drawn.size);
                    let options = TextureOptions { magnification: TextureFilter::Linear, minification: TextureFilter::Linear, ..TextureOptions::LINEAR };
                    match self.texture.as_mut() {
                        Some(t) => t.set(drawn.image, options),
                        None => self.texture = Some(ctx.load_texture(&self.name, drawn.image, options)),
                    }
                    self.job.shown = Some(key);
                }
                Ok(Err(e)) => {
                    // the last good drawing stays on screen, with the reason it is not the current one
                    self.error = Some(e);
                    self.job.shown = Some(key);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.job.running = Some((key, rx));
                    ctx.request_repaint_after(Duration::from_millis(30));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("the drawing could not be rendered".into());
                    self.job.shown = Some(key);
                }
            }
        }
        let want = (input.rev, scale_step(zoom));
        self.job.wanted = Some(want);
        if self.job.running.is_none() && self.job.shown != Some(want) {
            let (tx, rx) = mpsc::channel();
            let (text, dir, scale, side) = (input.text.to_string(), input.dir.map(Path::to_path_buf), step_scale(want.1), MAX_SVG_SIDE.min(ctx.input(|i| i.max_texture_side)));
            std::thread::spawn(move || {
                let _ = tx.send(draw_svg(&text, dir.as_deref(), scale, side));
            });
            self.job.running = Some((want, rx));
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }

    fn paint_error(&self, p: &egui::Painter, view: Rect, pal: &Palette, msg: &str, over_picture: bool) {
        let text = i18n::fmt("Cannot show this picture: {}", &[&msg]);
        if over_picture {
            // a strip over the last good picture
            let g = p.layout(text, fonts::ui(12.5), pal.text_on_accent, (view.width() - 24.0).max(40.0));
            let strip = Rect::from_min_size(view.min, vec2(view.width(), g.size().y + 14.0));
            p.rect_filled(strip, CornerRadius::ZERO, pal.error.gamma_multiply(0.92));
            p.galley(pos2(strip.min.x + 12.0, strip.min.y + 7.0), g, pal.text_on_accent);
        } else {
            let g = p.layout(text, fonts::ui(13.0), pal.error, (view.width() - 48.0).clamp(40.0, 640.0));
            p.galley(pos2(view.center().x - g.size().x / 2.0, view.center().y - g.size().y / 2.0), g, pal.error);
        }
    }

    fn paint_bar(&mut self, ui: &mut egui::Ui, p: &egui::Painter, bar: Rect, pal: &Palette, zoom: f32, view: Rect) {
        p.rect_filled(bar, CornerRadius::ZERO, pal.panel_bg);
        p.hline(bar.x_range(), bar.min.y, Stroke::new(1.0, pal.border));
        let font = fonts::ui(12.0);
        // from the right edge to the left: + 100% - 1:1 Fit
        let mut row = BarRow { p, pal, font: font.clone(), bar, x: bar.max.x - 10.0, owner: &self.name };
        let zoom_in = row.button(ui, "+", 24.0, tr("Zoom in"), "in");
        row.label(&format!("{}%", (zoom * 100.0).round()), 52.0);
        let zoom_out = row.button(ui, "\u{2212}", 24.0, tr("Zoom out"), "out");
        row.x -= 8.0;
        let actual = row.button(ui, "1:1", 32.0, tr("Actual size"), "actual");
        let fit = row.button(ui, tr("Fit"), 34.0, tr("Fit to window"), "fit");
        let left_end = row.x;
        let info = self.info();
        if !info.is_empty() && left_end - bar.min.x > 90.0 {
            let clip = p.with_clip_rect(Rect::from_min_max(bar.min, pos2(left_end, bar.max.y)));
            clip.text(pos2(bar.min.x + 12.0, bar.center().y), Align2::LEFT_CENTER, info, font, pal.text_dim);
        }
        if zoom_in {
            self.zoom_around(view, view.center(), zoom, zoom * 1.25);
        } else if zoom_out {
            self.zoom_around(view, view.center(), zoom, zoom / 1.25);
        } else if actual {
            self.zoom_around(view, view.center(), zoom, 1.0);
        } else if fit {
            self.zoom = None;
            self.pan = Vec2::ZERO;
        }
    }
}

/// The buttons of the bar, laid out from its right edge to the left.
struct BarRow<'a> {
    p: &'a egui::Painter,
    pal: &'a Palette,
    font: egui::FontId,
    bar: Rect,
    /// The left edge of what is placed so far.
    x: f32,
    owner: &'a str,
}

impl BarRow<'_> {
    fn button(&mut self, ui: &egui::Ui, label: &str, w: f32, tip: &str, id: &str) -> bool {
        let r = Rect::from_min_size(pos2(self.x - w, self.bar.center().y - 11.0), vec2(w, 22.0));
        self.x -= w + 4.0;
        let resp = ui.interact(r, ui.scope_id().with(("picture_button", self.owner, id)), Sense::click());
        if resp.hovered() {
            self.p.rect_filled(r, CornerRadius::same(6), self.pal.hover);
        }
        self.p.text(r.center(), Align2::CENTER_CENTER, label, self.font.clone(), if resp.hovered() { self.pal.text } else { self.pal.text_dim });
        resp.on_hover_text(tip).clicked()
    }

    fn label(&mut self, text: &str, w: f32) {
        let r = Rect::from_min_size(pos2(self.x - w, self.bar.center().y - 11.0), vec2(w, 22.0));
        self.x -= w + 4.0;
        self.p.text(r.center(), Align2::CENTER_CENTER, text, self.font.clone(), self.pal.text);
    }
}

fn raster_options(nearest: bool) -> TextureOptions {
    TextureOptions {
        magnification: if nearest { TextureFilter::Nearest } else { TextureFilter::Linear },
        minification: TextureFilter::Linear,
        wrap_mode: egui::TextureWrapMode::ClampToEdge,
        // a big picture shown small stays smooth
        mipmap_mode: Some(TextureFilter::Linear),
    }
}

/// The picture cannot be dragged away: along an axis where it is smaller than the pane it stays in the middle, where
/// it is bigger its edges can come to the pane's edges (and a margin more).
fn clamp_pan(pan: Vec2, disp: Vec2, pane: Vec2) -> Vec2 {
    let axis = |p: f32, d: f32, v: f32| if d <= v { 0.0 } else { let lim = (d - v) / 2.0 + MARGIN; p.clamp(-lim, lim) };
    vec2(axis(pan.x, disp.x, pane.x), axis(pan.y, disp.y, pane.y))
}

/// The squares behind a picture that show where it is transparent.
fn paint_chequers(p: &egui::Painter, area: Rect, origin: Pos2, dark: bool) {
    if area.width() <= 0.0 || area.height() <= 0.0 {
        return;
    }
    const TILE: f32 = 10.0;
    let (a, b) = if dark { (Color32::from_rgb(0x2a, 0x2b, 0x2e), Color32::from_rgb(0x38, 0x3a, 0x3e)) } else { (Color32::WHITE, Color32::from_rgb(0xe4, 0xe6, 0xea)) };
    p.rect_filled(area, CornerRadius::ZERO, a);
    let mut mesh = Mesh::default();
    let (c0, c1) = (((area.min.x - origin.x) / TILE).floor() as i32, ((area.max.x - origin.x) / TILE).ceil() as i32);
    let (r0, r1) = (((area.min.y - origin.y) / TILE).floor() as i32, ((area.max.y - origin.y) / TILE).ceil() as i32);
    for r in r0..r1 {
        for c in c0..c1 {
            if (r + c).rem_euclid(2) == 1 {
                let tile = Rect::from_min_size(pos2(origin.x + c as f32 * TILE, origin.y + r as f32 * TILE), Vec2::splat(TILE)).intersect(area);
                if tile.width() > 0.0 && tile.height() > 0.0 {
                    mesh.add_colored_rect(tile, b);
                }
            }
        }
    }
    p.add(egui::Shape::mesh(mesh));
}

fn human_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    let v = n as f64;
    if v < KB {
        format!("{n} B")
    } else if v < KB * KB {
        format!("{:.1} KB", v / KB)
    } else {
        format!("{:.1} MB", v / KB / KB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba_at(img: &ColorImage, x: usize, y: usize) -> [u8; 4] {
        img.pixels[y * img.size[0] + x].to_array()
    }

    fn encode(format: image::ImageFormat, w: u32, h: u32) -> Vec<u8> {
        // a red picture with a blue pixel in the corner and a half transparent one next to it
        let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 0, image::Rgba([0, 0, 255, 255]));
        let mut out = Cursor::new(Vec::new());
        match format {
            // these have no transparency or no colour: write what they can hold
            image::ImageFormat::Jpeg | image::ImageFormat::Bmp => image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(img).to_rgb8()).write_to(&mut out, format).unwrap(),
            _ => img.write_to(&mut out, format).unwrap(),
        }
        out.into_inner()
    }

    #[test]
    fn kinds_follow_the_extension_in_any_case() {
        for (name, kind) in [("a.png", Some(Kind::Raster)), ("A.JPG", Some(Kind::Raster)), ("x.jpeg", Some(Kind::Raster)), ("x.gif", Some(Kind::Raster)), ("x.bmp", Some(Kind::Raster)), ("x.WebP", Some(Kind::Raster)), ("x.ico", Some(Kind::Raster)), ("logo.SVG", Some(Kind::Svg)), ("x.txt", None), ("png", None), ("x.svg.bak", None)] {
            assert_eq!(kind_of(Path::new(name)), kind, "{name}");
        }
    }

    #[test]
    fn every_raster_format_decodes() {
        for (format, name) in [(image::ImageFormat::Png, "PNG"), (image::ImageFormat::Gif, "GIF"), (image::ImageFormat::Bmp, "BMP"), (image::ImageFormat::Jpeg, "JPEG"), (image::ImageFormat::WebP, "WebP"), (image::ImageFormat::Ico, "ICO")] {
            let (w, h) = if format == image::ImageFormat::Ico { (32, 32) } else { (20, 12) };
            let r = decode_raster(&encode(format, w, h)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(r.format, name);
            assert_eq!(r.rgba.dimensions(), (w, h), "{name}");
            // the colour survives (JPEG only roughly)
            let px = r.rgba.get_pixel(w - 1, h - 1).0;
            assert!(px[0] > 200 && px[1] < 60 && px[2] < 60 && px[3] == 255, "{name}: {px:?}");
        }
    }

    #[test]
    fn a_format_is_known_by_content_not_by_name() {
        // a PNG saved with a wrong extension still opens; nonsense does not
        assert!(decode_raster(&encode(image::ImageFormat::Png, 4, 4)).is_ok());
        assert!(decode_raster(b"not a picture at all").is_err());
        assert!(decode_raster(b"").is_err());
        let mut cut = encode(image::ImageFormat::Png, 40, 40);
        cut.truncate(cut.len() / 2);
        assert!(decode_raster(&cut).is_err(), "a cut file is an error, not a half picture");
    }

    #[test]
    fn transparency_is_kept() {
        let mut img = image::RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([10, 20, 30, 0]));
        img.put_pixel(1, 0, image::Rgba([10, 20, 30, 128]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        let r = decode_raster(&out.into_inner()).unwrap();
        assert_eq!(r.rgba.get_pixel(0, 0).0[3], 0);
        assert_eq!(r.rgba.get_pixel(1, 0).0[3], 128);
        let c = color_image(&r.rgba);
        assert_eq!(c.pixels[0].a(), 0);
        assert_eq!(c.pixels[1].a(), 128);
    }

    #[test]
    fn a_huge_picture_is_shrunk_to_the_graphics_card() {
        let big = image::RgbaImage::new(300, 100);
        let small = fit_side(big, 150);
        assert_eq!(small.dimensions(), (150, 50));
        let fits = fit_side(image::RgbaImage::new(30, 10), 150);
        assert_eq!(fits.dimensions(), (30, 10));
        // a very thin one keeps at least a pixel
        assert_eq!(fit_side(image::RgbaImage::new(4000, 1), 100).dimensions(), (100, 1));
    }

    const RED_SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10" viewBox="0 0 20 10"><rect width="10" height="10" fill="#ff0000"/></svg>"##;

    #[test]
    fn an_svg_is_drawn_at_the_asked_scale() {
        let d = draw_svg(RED_SQUARE, None, 1.0, 4096).unwrap();
        assert_eq!(d.size, [20.0, 10.0]);
        assert_eq!(d.image.size, [20, 10]);
        // the left half is red, the right half transparent
        assert_eq!(rgba_at(&d.image, 2, 5), [255, 0, 0, 255]);
        assert_eq!(rgba_at(&d.image, 15, 5)[3], 0);
        let big = draw_svg(RED_SQUARE, None, 4.0, 4096).unwrap();
        assert_eq!(big.size, [20.0, 10.0], "the size the SVG says does not depend on the scale");
        assert_eq!(big.image.size, [80, 40]);
        assert_eq!(rgba_at(&big.image, 30, 20), [255, 0, 0, 255]);
        assert_eq!(rgba_at(&big.image, 60, 20)[3], 0);
    }

    #[test]
    fn an_svg_is_never_drawn_bigger_than_the_limit() {
        let d = draw_svg(RED_SQUARE, None, 1000.0, 256).unwrap();
        assert_eq!(d.image.size, [256, 128]);
    }

    #[test]
    fn the_viewbox_gives_the_size_when_there_is_no_width() {
        let d = draw_svg(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 24"><circle cx="12" cy="12" r="12" fill="#00f"/></svg>"##, None, 1.0, 4096).unwrap();
        assert_eq!(d.size, [48.0, 24.0]);
    }

    #[test]
    fn broken_svg_is_an_error_not_a_crash() {
        for bad in ["", "   \n", "not xml", "<svg", "<html><body>hi</body></html>", r#"<svg xmlns="http://www.w3.org/2000/svg" width="0" height="0"/>"#] {
            assert!(draw_svg(bad, None, 1.0, 4096).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn svg_text_is_drawn_with_the_bundled_fonts() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40"><text x="4" y="30" font-size="28" fill="#000">Hi</text></svg>"##;
        let d = draw_svg(svg, None, 1.0, 4096).unwrap();
        assert!(d.image.pixels.iter().any(|p| p.a() > 200), "the letters have pixels");
        // the same with the generic family names a drawing usually asks for
        let generic = svg.replace("font-size", "font-family=\"sans-serif\" font-size");
        assert!(draw_svg(&generic, None, 1.0, 4096).unwrap().image.pixels.iter().any(|p| p.a() > 200));
    }

    #[test]
    fn svg_pictures_next_to_it_are_found() {
        let dir = std::env::temp_dir().join(format!("light-svg-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        std::fs::write(dir.join("dot.png"), png.into_inner()).unwrap();
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8"><image href="dot.png" width="8" height="8"/></svg>"#;
        let d = draw_svg(svg, Some(&dir), 1.0, 4096).unwrap();
        assert_eq!(rgba_at(&d.image, 4, 4), [0, 255, 0, 255]);
        // without the folder the picture is simply missing
        let lost = draw_svg(svg, None, 1.0, 4096).unwrap();
        assert_eq!(rgba_at(&lost.image, 4, 4)[3], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scales_come_in_steps_that_are_never_below_the_zoom() {
        for z in [0.03_f32, 0.1, 0.37, 0.5, 0.99, 1.0, 1.01, 1.4, 2.0, 3.3, 10.0] {
            let s = step_scale(scale_step(z));
            assert!(s >= z * 0.999, "zoom {z} drawn at {s}");
            assert!(s < z * 1.5 || s <= 0.0155, "zoom {z} drawn too big at {s}");
        }
        assert_eq!(scale_step(1.0), 0);
        assert_eq!(scale_step(2.0), 2);
    }

    #[test]
    fn the_picture_cannot_be_dragged_out_of_sight() {
        let pane = vec2(400.0, 300.0);
        // smaller than the pane: stays in the middle
        assert_eq!(clamp_pan(vec2(50.0, -30.0), vec2(100.0, 100.0), pane), Vec2::ZERO);
        // bigger: its edge can reach the pane's edge and a margin more, not further
        let p = clamp_pan(vec2(5000.0, -5000.0), vec2(1000.0, 800.0), pane);
        assert_eq!(p, vec2((1000.0 - 400.0) / 2.0 + MARGIN, -((800.0 - 300.0) / 2.0 + MARGIN)));
        // only one axis is bigger
        let p = clamp_pan(vec2(10.0, 10.0), vec2(1000.0, 100.0), pane);
        assert_eq!(p, vec2(10.0, 0.0));
    }

    #[test]
    fn a_small_picture_is_shown_as_it_is_and_a_big_one_is_fitted() {
        let mut v = ImageView::blank(Kind::Raster);
        v.natural = Some([100.0, 50.0]);
        assert_eq!(v.fit_zoom(vec2(800.0, 600.0), 1.0), 1.0);
        v.natural = Some([2000.0, 1000.0]);
        let z = v.fit_zoom(vec2(840.0, 640.0), 1.0);
        assert!((z - 0.4).abs() < 1e-4, "{z}");
        // on a screen with twice the pixels a pixel of the picture is half a point at 100%
        let z2 = v.fit_zoom(vec2(840.0, 640.0), 2.0);
        assert!((z2 - 0.8).abs() < 1e-4, "{z2}");
        // a drawing grows to the pane
        let mut s = ImageView::new_svg();
        s.natural = Some([16.0, 16.0]);
        assert!(s.fit_zoom(vec2(840.0, 640.0), 1.0) > 30.0);
    }

    #[test]
    fn the_bar_tells_what_the_picture_is() {
        let mut v = ImageView::blank(Kind::Raster);
        assert_eq!(v.info(), "");
        v.natural = Some([640.0, 480.0]);
        v.format = "PNG";
        v.bytes = 31_900;
        assert_eq!(v.info(), "PNG \u{b7} 640 \u{d7} 480 px \u{b7} 31.2 KB");
        let mut s = ImageView::new_svg();
        s.natural = Some([24.0, 24.0]);
        assert_eq!(s.info(), "SVG \u{b7} 24 \u{d7} 24 px");
        assert_eq!(human_bytes(12), "12 B");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn opening_a_file_that_is_not_a_picture_says_why() {
        let dir = std::env::temp_dir().join(format!("light-img-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("fake.png");
        std::fs::write(&bad, "this is text").unwrap();
        let v = ImageView::open(&bad);
        assert!(v.error.is_some() && v.natural().is_none() && !v.has_picture());
        let missing = ImageView::open(&dir.join("nothing.png"));
        assert!(missing.error.is_some());
        let good = dir.join("ok.png");
        std::fs::write(&good, encode(image::ImageFormat::Png, 7, 5)).unwrap();
        let v = ImageView::open(&good);
        assert!(v.error.is_none());
        assert_eq!(v.natural(), Some([7.0, 5.0]));
        let _ = std::fs::remove_dir_all(dir);
    }
}
