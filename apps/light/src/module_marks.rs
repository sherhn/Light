//! Vector marks, independent of old cached typographic module covers.
use eframe::egui;
pub fn source(id: &str) -> Option<&'static str> {
    Some(match id {
        "python" => include_str!("../assets/module-marks/python.svg"),
        "c-cpp" => include_str!("../assets/module-marks/cplusplus.svg"),
        "shell-make" => include_str!("../assets/module-marks/gnubash.svg"),
        "javascript" => include_str!("../assets/module-marks/javascript.svg"),
        "docker" => include_str!("../assets/module-marks/docker.svg"),
        "html" => include_str!("../assets/module-marks/html5.svg"),
        "css" => include_str!("../assets/module-marks/css.svg"),
        "markdown" => include_str!("../assets/module-marks/markdown.svg"),
        "json" => include_str!("../assets/module-marks/json.svg"),
        "toml" => include_str!("../assets/module-marks/toml.svg"),
        "yaml" => include_str!("../assets/module-marks/yaml.svg"),
        "dotenv" => include_str!("../assets/module-marks/dotenv.svg"),
        "lisp" => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="none" stroke="black" stroke-width="2.5" d="M5 3h4l7 17h4M12 10L5 21"/></svg>"#,
        "assembler" => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g fill="none" stroke="black" stroke-width="2"><rect x="5" y="5" width="14" height="14" rx="2"/><rect x="9" y="9" width="6" height="6"/><path d="M8 1v4m8-4v4M8 19v4m8-4v4M1 8h4m-4 8h4m14-8h4m-4 8h4"/></g></svg>"#,
        "http" => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g fill="none" stroke="black" stroke-width="1.8"><circle cx="12" cy="12" r="10"/><ellipse cx="12" cy="12" rx="4" ry="10"/><path d="M2 12h20M4 6h16M4 18h16"/></g></svg>"#,
        "ini" | "restructuredtext" => r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g fill="none" stroke="black" stroke-width="2"><path d="M14 2H5v20h14V7zM14 2v5h5M8 12h8m-8 4h8"/></g></svg>"#,
        _ => return None,
    })
}

pub fn texture(ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
    let key = egui::Id::unique(("module_vector_mark", id));
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) { return Some(texture); }
    let tree = resvg::usvg::Tree::from_str(source(id)?, &Default::default()).ok()?;
    let mut pixels = resvg::tiny_skia::Pixmap::new(256, 256)?;
    let transform = resvg::tiny_skia::Transform::from_scale(256.0 / tree.size().width(), 256.0 / tree.size().height());
    resvg::render(&tree, transform, &mut pixels.as_mut());
    let rgba: Vec<u8> = pixels.data().as_chunks::<4>().0.iter().flat_map(|p| [255, 255, 255, p[3]]).collect();
    let texture = ctx.load_texture(id, egui::ColorImage::from_rgba_unmultiplied([256, 256], &rgba), Default::default());
    ctx.data_mut(|d| d.insert_temp(key, texture.clone()));
    Some(texture)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_official_modules_have_visible_vector_marks() {
        let ctx = egui::Context::default();
        let index = ide_core::modules::Index::parse(include_str!("../../../registry/index.json")).unwrap();
        for entry in index.modules { assert!(texture(&ctx, &entry.id).is_some(), "{}", entry.id); }
    }
}
