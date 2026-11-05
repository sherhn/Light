//! File badges shared by the tree, tabs, search, dialogs and Quick View.
//! The embedded catalog needs no module installation, file reads or network access.
use std::{path::Path, sync::LazyLock};
use eframe::egui::{self, Color32, Painter, Rect};
use serde::Deserialize;
use ide_core::settings::FileIconStyle;
use crate::{icons::{self, Icon}, theme::Palette};

#[derive(Deserialize)]
pub struct FileType { pub id: String, pub name: String, pub extensions: Vec<String>, pub files: Vec<String> }
pub static TYPES: LazyLock<Vec<FileType>> = LazyLock::new(|| serde_json::from_str(include_str!("../../../assets/file-types.json")).expect("checked file icon catalog"));

pub fn apply(ctx: &egui::Context, style: FileIconStyle) { ctx.data_mut(|data|data.insert_temp(egui::Id::unique("file_icon_style"),style)); }
pub fn style(ctx: &egui::Context) -> FileIconStyle { ctx.data(|data|data.get_temp(egui::Id::unique("file_icon_style"))).unwrap_or_default() }

pub fn family(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_string_lossy();
    // Match filenames before extensions; prefer the most specific wildcard.
    let exact = TYPES.iter().find(|kind| kind.files.iter().any(|pattern| !pattern.contains('*') && pattern.eq_ignore_ascii_case(&name)));
    let wildcard = || TYPES.iter().flat_map(|kind| kind.files.iter().filter(|p| p.contains('*') && light_lang::matches_file_name(p, &name)).map(move |p| (p.chars().filter(|c| *c != '*').count(), kind))).max_by_key(|(n,_)| *n).map(|(_,kind)|kind);
    let extension = || path.extension().and_then(|e| e.to_str()).and_then(|ext| TYPES.iter().find(|kind| kind.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))));
    exact.or_else(wildcard).or_else(extension).map(|kind| kind.id.as_str())
}
pub fn name(path: &Path) -> Option<&'static str> {
    let id = family(path)?;
    TYPES.iter().find(|kind| kind.id == id).map(|kind|kind.name.as_str())
}

fn language_family(language: &str) -> Option<&'static str> {
    Some(match language {
        "c" | "cpp" => "c-cpp", "shell" | "make" => "shell-make", "nasm" | "gas" => "assembler",
        "rst" => "restructuredtext", "jsonc" => "json", "jsx" => "javascript", "dockerfile" | "compose" | "dockerignore" => "docker",
        "common-lisp" | "scheme" | "clojure" | "emacs-lisp" => "lisp",
        name => return TYPES.iter().find(|kind| kind.id == name).map(|kind| kind.id.as_str()),
    })
}

fn color(pal: &Palette, family: &str) -> Color32 {
    match family {
        "lisp" | "css" | "yaml" => pal.keyword_form,
        "python" | "javascript" | "json" => pal.number,
        "c-cpp" | "docker" | "http" => pal.definition,
        "html" | "assembler" | "toml" => pal.alert,
        "shell-make" | "dotenv" | "ini" => pal.string,
        _ => pal.blue,
    }
}
fn neutral(pal: &Palette) -> Color32 {
    let color=pal.text_dim;
    let gray=(0.2126*color.r() as f32+0.7152*color.g() as f32+0.0722*color.b() as f32).round() as u8;
    Color32::from_gray(gray)
}

pub fn paint(painter: &Painter, rect: Rect, path: Option<&Path>, language: Option<&str>, pal: &Palette, muted: bool) {
    let style=style(painter.ctx());
    if style==FileIconStyle::Standard {
        icons::paint(painter,rect,Icon::File,neutral(pal).gamma_multiply(if muted {0.55}else{1.0}));
        return;
    }
    let family = path.and_then(family).or_else(|| language.and_then(language_family));
    let tint = if style==FileIconStyle::Monochrome {neutral(pal)} else {family.map_or(pal.text_dim, |id| color(pal,id))}.gamma_multiply(if muted { 0.55 } else { 1.0 });
    if let Some(id) = family && let Some(texture) = crate::module_marks::texture(painter.ctx(),id) {
        painter.image(texture.id(),rect.shrink(0.5),Rect::from_min_max(egui::Pos2::ZERO,egui::pos2(1.0,1.0)),tint);
    } else {
        let icon = if path.is_some_and(|p| crate::imageview::kind_of(p).is_some()) { Icon::Preview } else if family.is_some() || language.is_some_and(|l| l != "plain") { Icon::Code } else { Icon::File };
        icons::paint(painter,rect,icon,tint);
    }
}

pub fn selectable(ui: &mut egui::Ui, path: &Path, pal: &Palette, selected: bool) -> egui::Response {
    let style=style(ui.ctx());let family=if style==FileIconStyle::Standard {None}else{family(path)};
    let tint=if style==FileIconStyle::Colored {family.map_or(pal.text_dim,|id|color(pal,id))}else{neutral(pal)};
    let label=path.file_name().unwrap_or_default().to_string_lossy();
    let button=if let Some(texture)=crate::module_marks::texture(ui.ctx(),family.unwrap_or("default-file")) {
        egui::Button::image_and_text(egui::Image::new((texture.id(),egui::Vec2::splat(16.0))).tint(tint),label)
    } else { egui::Button::new(label) };
    let description=name(path).map_or_else(||path.display().to_string(),|name|format!("{name}\n{}",path.display()));
    ui.add(button.selected(selected).frame(false)).on_hover_text(description)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monochrome_icons_stay_neutral_in_dark_light_and_custom_palettes() {
        for mut palette in [Palette::dark(),Palette::light()] {
            palette.text_dim=Color32::from_rgb(200,100,150);
            let ctx=egui::Context::default();
            let mut output=ctx.run_ui(Default::default(),|ui| {
                let ctx=ui.ctx();
                apply(ctx,FileIconStyle::Monochrome);
                let painter=egui::Painter::new(ctx.clone(),egui::LayerId::new(egui::Order::Foreground,egui::Id::unique("file-icon-test")),Rect::EVERYTHING);
                paint(&painter,Rect::from_min_size(egui::Pos2::ZERO,egui::Vec2::splat(16.0)),Some(Path::new("code.py")),None,&palette,false);
            });
            let colors:Vec<_>=output.shapes.iter().filter_map(|shape|if let egui::epaint::Shape::Mesh(mesh)=&shape.shape {Some(mesh.vertices.iter().map(|v|v.color))}else{None}).flatten().collect();
            output.textures_delta.clear();
            assert!(!colors.is_empty());
            assert!(colors.iter().all(|c|c.r()==c.g()&&c.g()==c.b()));
            assert_eq!(style(&ctx),FileIconStyle::Monochrome);
        }
    }
    #[test]
    fn standard_mode_uses_the_same_document_glyph_for_code_images_and_unknown_files() {
        let ctx=egui::Context::default();crate::fonts::install(&ctx);
        let mut output=ctx.run_ui(Default::default(),|ui| {
            let ctx=ui.ctx();apply(ctx,FileIconStyle::Standard);
            let painter=egui::Painter::new(ctx.clone(),egui::LayerId::new(egui::Order::Foreground,egui::Id::unique("standard-file-icon-test")),Rect::EVERYTHING);
            for (index,name) in ["code.py","picture.png","unknown.data"].iter().enumerate() {
                paint(&painter,Rect::from_min_size(egui::pos2(index as f32*20.0,0.0),egui::Vec2::splat(16.0)),Some(Path::new(name)),None,&Palette::dark(),false);
            }
        });
        let glyphs:Vec<_>=output.shapes.iter().filter_map(|shape|if let egui::epaint::Shape::Text(text)=&shape.shape {Some(text.galley.job.text.clone())}else{None}).collect();
        output.textures_delta.clear();
        assert_eq!(glyphs,vec![icons::glyph(Icon::File).unwrap().to_string();3]);
    }
    #[test]
    fn all_catalog_extensions_and_special_filenames_have_badges() {
        for kind in TYPES.iter() {
            for extension in &kind.extensions { assert!(family(Path::new(&format!("example.{extension}"))).is_some(),"{extension}"); }
            for name in &kind.files { assert!(family(Path::new(&name.replace('*',"example"))).is_some(),"{name}"); }
            assert!(kind.id == "source" || crate::module_marks::source(&kind.id).is_some(),"{}",kind.id);
        }
        for (name,id) in [("tool.LSP","lisp"),("file.PYI","python"),("Dockerfile.dev","docker"),("compose.prod.yaml","docker"),("Makefile","shell-make"),(".env.local","dotenv"),("settings.jsonc","json"),(".editorconfig","ini"),("main.cpp","c-cpp"),("request.http","http")] {
            assert_eq!(family(Path::new(name)),Some(id),"{name}");
        }
        assert_eq!(family(Path::new("notes.unknown")),None);
    }
}
