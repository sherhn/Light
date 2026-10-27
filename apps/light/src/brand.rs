//! The Light logo: window icon and the logo drawn in the title bar, welcome screen and About dialog.

use std::io::Cursor;

use eframe::egui::{self, Color32, Rect, TextureHandle, TextureOptions, pos2};

const ICON_256: &[u8] = include_bytes!("../../../assets/icon/light-256.png");
const LOGO_128: &[u8] = include_bytes!("../../../assets/icon/light-128.png");

/// Decode a PNG of any common kind (palette, grey, RGB, with or without alpha, 8 or 16 bits) to RGBA8.
pub fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}

pub fn window_icon() -> Option<egui::IconData> {
    let (width, height, rgba) = decode_png(ICON_256)?;
    Some(egui::IconData { rgba, width, height })
}

pub fn load_logo(ctx: &egui::Context) -> Option<TextureHandle> {
    let (w, h, rgba) = decode_png(LOGO_128)?;
    let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    Some(ctx.load_texture("light-logo", img, TextureOptions::LINEAR))
}

/// Paint the logo into `rect`; falls back to a plain accent tile if the texture is missing.
pub fn paint_logo(painter: &egui::Painter, rect: Rect, tex: Option<&TextureHandle>, accent: Color32) {
    match tex {
        Some(t) => {
            // A neutral backing keeps the approved white mark legible in both themes.
            painter.rect_filled(rect, egui::CornerRadius::same((rect.width() * 0.18) as u8), Color32::from_rgb(0x10, 0x12, 0x13));
            painter.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            painter.rect_filled(rect, egui::CornerRadius::same((rect.width() * 0.22) as u8), accent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_icons_decode_to_rgba() {
        let (w, h, px) = decode_png(ICON_256).expect("256 icon");
        assert_eq!((w, h), (256, 256));
        assert_eq!(px.len(), 256 * 256 * 4);
        let (w, h, _) = decode_png(LOGO_128).expect("128 logo");
        assert_eq!((w, h), (128, 128));
        // The supplied ribbon has a transparent surround and opaque white planes.
        assert_eq!(px[3], 0);
        assert!(px.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 255));
        assert!(px.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 0));
    }

    #[test]
    fn window_icon_is_available() {
        assert!(window_icon().is_some());
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_png(b"not a png").is_none());
    }

    #[test]
    fn covers_of_any_png_kind_decode() {
        fn encode(color: png::ColorType, data: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            let mut enc = png::Encoder::new(&mut out, 2, 1);
            enc.set_color(color);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(data).unwrap();
            out
        }
        assert_eq!(decode_png(&encode(png::ColorType::Rgb, &[1, 2, 3, 4, 5, 6])).unwrap().2, [1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(decode_png(&encode(png::ColorType::Rgba, &[1, 2, 3, 4, 5, 6, 7, 8])).unwrap().2, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(decode_png(&encode(png::ColorType::Grayscale, &[9, 8])).unwrap().2, [9, 9, 9, 255, 8, 8, 8, 255]);
        assert_eq!(decode_png(&encode(png::ColorType::GrayscaleAlpha, &[9, 1, 8, 2])).unwrap().2, [9, 9, 9, 1, 8, 8, 8, 2]);
    }
}
