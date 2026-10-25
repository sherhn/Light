//! Fonts: JetBrains Mono for code and Inter for the UI (both SIL OFL 1.1, bundled unmodified,
//! see assets/fonts). egui's built-in fonts stay as fallbacks for any missing glyph.

use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

const MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
const UI: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const UI_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
const ICONS: &[u8] = include_bytes!("../../../assets/icons/lucide.ttf");

pub fn semibold_family() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, semibold_family())
}

pub fn icons_family() -> FontFamily {
    FontFamily::Name("icons".into())
}

/// Lucide icon font at `size` px.
pub fn icons(size: f32) -> FontId {
    FontId::new(size, icons_family())
}

pub fn ui(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    // egui shapes with the font's default GSUB features and exposes no feature switch.
    // Hide the substitution table in the in-memory code font, keeping the bundled
    // original intact. This also covers terminal, diff and read-only code widgets.
    fonts.font_data.insert("jetbrains-mono".into(), Arc::new(FontData::from_owned(literal_code_font())));
    fonts.font_data.insert("inter".into(), Arc::new(FontData::from_static(UI)));
    fonts.font_data.insert("inter-semibold".into(), Arc::new(FontData::from_static(UI_SEMIBOLD)));
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "jetbrains-mono".into());
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    // semibold: Inter SemiBold first, then the regular chain for fallback glyphs
    let mut chain = vec!["inter-semibold".to_string()];
    chain.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(semibold_family(), chain);
    fonts.font_data.insert("lucide".into(), Arc::new(FontData::from_static(ICONS)));
    fonts.families.insert(icons_family(), vec!["lucide".to_string()]);
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(TextStyle::Body, ui(13.5));
        style.text_styles.insert(TextStyle::Button, ui(13.5));
        style.text_styles.insert(TextStyle::Small, ui(11.5));
        style.text_styles.insert(TextStyle::Heading, semibold(18.0));
        style.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.5));
    });
}

fn literal_code_font() -> Vec<u8> {
    let mut font = MONO.to_vec();
    let count = u16::from_be_bytes([font[4], font[5]]) as usize;
    let (tables, _) = font[12..12 + count * 16].as_chunks_mut::<16>();
    for table in tables {
        if &table[..4] == b"GSUB" {
            table[..4].copy_from_slice(b"NoGS");
        }
    }
    font
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_are_valid_truetype() {
        for f in [MONO, UI, UI_SEMIBOLD, ICONS] {
            assert!(matches!(&f[..4], [0, 1, 0, 0] | b"OTTO"), "not a TrueType/OpenType file");
            assert!(f.len() > 100_000);
        }
    }

    #[test]
    fn license_files_ship_with_the_fonts() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts");
        for f in ["JetBrainsMono-OFL.txt", "Inter-OFL.txt", "README.md"] {
            assert!(dir.join(f).is_file(), "{f} must accompany the bundled fonts");
        }
        assert!(dir.join("../icons/LICENSE").is_file(), "the Lucide ISC licence must accompany the icon font");
    }

    #[test]
    fn operators_keep_individual_visible_glyphs() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.fonts_mut(|f| {
                for text in [">>", "!=", "=>", "->", "===", "<=", "!=="] {
                    let g = f.layout_no_wrap(text.into(), FontId::monospace(14.0), egui::Color32::WHITE);
                    let glyphs = &g.rows[0].glyphs;
                    assert_eq!(glyphs.len(), text.len());
                    assert!(glyphs.iter().all(|g| g.advance_width > 0.0), "ligature in {text}");
                }
            });
        });
        output.textures_delta.clear();
    }
}
