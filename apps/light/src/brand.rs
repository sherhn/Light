//! The Light logo: window icon and the logo drawn in the title bar, welcome screen and About dialog. It comes in two
//! versions: the light ribbon for dark backgrounds and the dark one for light backgrounds.

use std::io::Cursor;

use eframe::egui::{self, Color32, Rect, TextureHandle, TextureOptions, pos2};
use ide_core::settings::LogoStyle;

const ICON_256: &[u8] = include_bytes!("../../../assets/icon/light-256.png");
const LOGO_128: &[u8] = include_bytes!("../../../assets/icon/light-128.png");
const DARK_ICON_256: &[u8] = include_bytes!("../../../assets/icon/light-dark-256.png");
const DARK_LOGO_128: &[u8] = include_bytes!("../../../assets/icon/light-dark-128.png");

/// The two versions of the logo, as textures.
#[derive(Default)]
pub struct Logos {
    pub light: Option<TextureHandle>,
    pub dark: Option<TextureHandle>,
}

impl Logos {
    /// The logo to draw on the window's own surfaces: the dark one on a light theme unless a version is chosen.
    pub fn for_theme(&self, style: LogoStyle, dark_theme: bool) -> Option<&TextureHandle> {
        if use_dark_logo(style, !dark_theme) { self.dark.as_ref().or(self.light.as_ref()) } else { self.light.as_ref() }
    }
}

/// Does a background (light or not) take the dark logo, given the version chosen in the settings?
pub fn use_dark_logo(style: LogoStyle, light_background: bool) -> bool {
    match style {
        LogoStyle::Auto => light_background,
        LogoStyle::Light => false,
        LogoStyle::Dark => true,
    }
}

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

/// The window's icon: the dark logo for a light taskbar.
pub fn window_icon(dark: bool) -> Option<egui::IconData> {
    let (width, height, rgba) = decode_png(if dark { DARK_ICON_256 } else { ICON_256 })?;
    Some(egui::IconData { rgba, width, height })
}

pub fn load_logos(ctx: &egui::Context) -> Logos {
    let load = |name: &str, png: &[u8]| {
        let (w, h, rgba) = decode_png(png)?;
        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
        Some(ctx.load_texture(name, img, TextureOptions::LINEAR))
    };
    Logos { light: load("light-logo", LOGO_128), dark: load("light-logo-dark", DARK_LOGO_128) }
}

/// Is the system's taskbar light? Windows says so in the user's personalization ("Choose your default Windows mode"),
/// at once. The other systems are asked in the background (see `system_theme`), so here nothing is known of them.
pub fn taskbar_is_light() -> Option<bool> {
    #[cfg(windows)]
    {
        windows::system_uses_light_theme()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// macOS: put the logo (the dark one for a light Dock) in the Dock as the application's icon. Returns false, having
/// changed nothing, when it cannot be done: not on the main thread, no application yet, the picture cannot be made.
#[cfg(target_os = "macos")]
pub fn set_dock_icon(dark: bool) -> bool {
    use objc2::MainThreadMarker;
    use objc2::AnyThread as _;
    use objc2_app_kit::{NSApplication, NSBitmapImageRep, NSDeviceRGBColorSpace, NSImage};
    use objc2_foundation::NSSize;

    let Some((width, height, mut rgba)) = decode_png(if dark { DARK_ICON_256 } else { ICON_256 }) else { return false };
    // the bitmap of AppKit takes its colours with the alpha multiplied in
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u32;
        for channel in &mut pixel[..3] {
            *channel = ((*channel as u32 * alpha + 127) / 255) as u8;
        }
    }
    // a failure of any kind leaves the Dock as it is
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(main_thread) = MainThreadMarker::new() else { return false };
        // SAFETY: AppKit is used on the main thread (the marker says so); the bitmap makes and owns its own memory,
        // which is filled only after its size, row length and pointer have been checked to hold the picture
        unsafe {
            let Some(rep) = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                width as isize,
                height as isize,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                (width * 4) as isize,
                32,
            ) else {
                return false;
            };
            let data = rep.bitmapData();
            if data.is_null() || rep.bytesPerRow() != (width * 4) as isize || rgba.len() != (width * height * 4) as usize {
                return false;
            }
            std::ptr::copy_nonoverlapping(rgba.as_ptr(), data, rgba.len());
            let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(width as f64, height as f64));
            image.addRepresentation(&rep);
            NSApplication::sharedApplication(main_thread).setApplicationIconImage(Some(&image));
        }
        true
    }))
    .unwrap_or(false)
}

/// Give the window `hwnd` the logo of the exe's icon resources (1: light, 2: dark) as its big icon, the one the taskbar
/// and Alt+Tab show, and as its small one. Returns whether both were set.
#[cfg(windows)]
pub fn set_window_icons(hwnd: isize, dark: bool, scale: f32) -> bool {
    windows::set_icons(hwnd, if dark { 2 } else { 1 }, scale)
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;

    const HKEY_CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
    const RRF_RT_REG_DWORD: u32 = 0x10;
    const IMAGE_ICON: u32 = 1;
    const WM_SETICON: u32 = 0x0080;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;
    const SM_CXICON: i32 = 11;
    const SM_CXSMICON: i32 = 49;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(key: isize, sub_key: *const u16, value: *const u16, flags: u32, kind: *mut u32, data: *mut c_void, len: *mut u32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn LoadImageW(instance: isize, name: *const u16, kind: u32, cx: i32, cy: i32, flags: u32) -> isize;
        fn SendMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetSystemMetrics(index: i32) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn system_uses_light_theme() -> Option<bool> {
        let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
        let value = wide("SystemUsesLightTheme");
        let mut data: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        // SAFETY: the strings end in NUL, and `data` is a DWORD whose size `len` gives
        let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut data as *mut u32).cast(), &mut len) };
        (status == 0).then_some(data != 0)
    }

    pub fn set_icons(hwnd: isize, resource: u16, scale: f32) -> bool {
        // SAFETY: a null name gives this exe's own module; the icon is looked up by its resource number
        // (MAKEINTRESOURCE), sized for the screen's scale; WM_SETICON takes an HICON, which the window keeps
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let mut done = 0;
            for (which, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
                let size = (GetSystemMetrics(metric) as f32 * scale.max(1.0)).round() as i32;
                let icon = LoadImageW(instance, resource as usize as *const u16, IMAGE_ICON, size, size, 0);
                if icon != 0 {
                    SendMessageW(hwnd, WM_SETICON, which, icon);
                    done += 1;
                }
            }
            done == 2
        }
    }
}

/// Paint the transparent logo into `rect`; without its picture, a chevron of `fallback` colour stands in.
pub fn paint_logo(painter: &egui::Painter, rect: Rect, tex: Option<&TextureHandle>, fallback: Color32) {
    match tex {
        Some(t) => {
            painter.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            painter.add(egui::Shape::line(vec![rect.right_top(), rect.left_center(), rect.right_bottom()], egui::Stroke::new(rect.width() * 0.16, fallback)));
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
        assert!(window_icon(false).is_some());
        assert!(window_icon(true).is_some());
    }

    #[test]
    fn the_dark_logo_is_the_dark_one() {
        // the ribbon's planes: light pixels in one, dark in the other
        let lightness = |png: &[u8]| {
            let (_, _, px) = decode_png(png).unwrap();
            let opaque: Vec<_> = px.as_chunks::<4>().0.iter().filter(|p| p[3] == 255).map(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3).collect();
            opaque.iter().sum::<u32>() / opaque.len() as u32
        };
        assert!(lightness(LOGO_128) > 200);
        assert!(lightness(DARK_LOGO_128) < 90);
    }

    #[test]
    fn the_version_follows_the_background_unless_one_is_chosen() {
        assert!(use_dark_logo(LogoStyle::Auto, true));
        assert!(!use_dark_logo(LogoStyle::Auto, false));
        assert!(!use_dark_logo(LogoStyle::Light, true));
        assert!(use_dark_logo(LogoStyle::Dark, false));
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
