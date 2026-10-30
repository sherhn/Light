//! The Linux shell of the Terminal window: the distribution's login shell in a pseudo console (see [`crate::pty`]), its
//! screen kept by a VT emulator and drawn cell by cell, with the keys, the mouse wheel and the clipboard of a terminal.
//! Everything a terminal of the distribution does works: bash with its prompt, history and completion, `export` and
//! `source` that last, colours, `sudo` asking for its password, full-screen programs (nano, htop, less, vim), Ctrl+C.

use eframe::egui::{self, Color32, CornerRadius, Event, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2};

use crate::pty::{Pty, PtyEvent};
use crate::theme::Palette;

/// Lines kept above the screen for scrolling back.
const SCROLLBACK: usize = 5000;

pub struct Shell {
    pty: Pty,
    parser: vt100::Parser,
    /// The distribution and the folder it started in.
    pub distro: String,
    pub cwd: String,
    /// Rows and columns of its screen.
    size: (u16, u16),
    /// The shell ended, with its exit code when known.
    pub exited: Option<Option<u32>>,
    /// The text selected with the mouse: from where the button went down to where it is, in cells of the screen shown.
    selection: Option<((u16, u16), (u16, u16))>,
}

impl Shell {
    /// Start the login shell of `distro` in the Linux folder `cwd`, `cols` x `rows` cells big. `ctx` is woken when it
    /// writes something.
    pub fn start(distro: &str, cwd: &str, cols: u16, rows: u16, ctx: &egui::Context) -> std::io::Result<Shell> {
        // `wsl.exe -d DISTRO --cd DIR`: the distribution's own shell, as its own terminal opens it (in the UI tests a fake
        // stands in for wsl.exe)
        let base = lisp_runtime::wsl::wsl_command();
        let program = base.get_program().to_os_string();
        let mut args: Vec<String> = base.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        args.extend(["-d".to_string(), distro.to_string(), "--cd".to_string(), cwd.to_string()]);
        let ctx = ctx.clone();
        let pty = Pty::spawn(&program, &args, None, &[], cols, rows, move || ctx.request_repaint())?;
        Ok(Shell { pty, parser: vt100::Parser::new(rows.max(2), cols.max(2), SCROLLBACK), distro: distro.to_string(), cwd: cwd.to_string(), size: (rows, cols), exited: None, selection: None })
    }

    /// Take what the shell wrote; true when something came.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Some(event) = self.pty.try_recv() {
            match event {
                PtyEvent::Output(bytes) => {
                    self.parser.process(&bytes);
                    any = true;
                }
                PtyEvent::Exited(code) => {
                    self.exited = Some(code);
                    any = true;
                }
            }
        }
        any
    }

    pub fn write(&self, bytes: &[u8]) {
        if self.exited.is_none() {
            self.pty.write(bytes);
        }
    }

    /// Type text as if it were pasted: in one piece for programs that ask for that (bracketed paste), lines ended by
    /// Enter.
    pub fn paste(&self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.parser.screen().bracketed_paste() {
            self.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.write(text.as_bytes());
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) != self.size && rows >= 2 && cols >= 2 {
            self.size = (rows, cols);
            self.parser.screen_mut().set_size(rows, cols);
            self.pty.resize(cols, rows);
        }
    }

    /// All the text of the screen shown (for the tests and for "copy all").
    pub fn text(&self) -> String {
        self.parser.screen().contents()
    }

    fn selected_text(&self) -> Option<String> {
        let ((r0, c0), (r1, c1)) = self.selection?;
        let (start, end) = if (r0, c0) <= (r1, c1) { ((r0, c0), (r1, c1)) } else { ((r1, c1), (r0, c0)) };
        let text = self.parser.screen().contents_between(start.0, start.1, end.0, end.1 + 1);
        (!text.is_empty()).then_some(text)
    }
}

/// What the keys a terminal sends for `key` with `modifiers` (`app_cursor`: the program asked for the arrows' other
/// form). Ctrl+C, Ctrl+V and Ctrl+X come as the copy, paste and cut events instead.
pub fn key_bytes(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    // the modifier number of xterm: 1 + shift + 2 alt + 4 ctrl
    let code = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
    let csi = |end: char| -> Vec<u8> {
        if code > 1 {
            format!("\x1b[1;{code}{end}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{end}").into_bytes()
        } else {
            format!("\x1b[{end}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> { if code > 1 { format!("\x1b[{n};{code}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() } };
    let bytes = match key {
        Key::Enter => b"\r".to_vec(),
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Backspace if m.ctrl => b"\x17".to_vec(),
        Key::Backspace if m.alt => b"\x1b\x7f".to_vec(),
        Key::Backspace => b"\x7f".to_vec(),
        Key::Escape => b"\x1b".to_vec(),
        Key::ArrowUp => csi('A'),
        Key::ArrowDown => csi('B'),
        Key::ArrowRight => csi('C'),
        Key::ArrowLeft => csi('D'),
        Key::Home => csi('H'),
        Key::End => csi('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => b"\x1bOP".to_vec(),
        Key::F2 => b"\x1bOQ".to_vec(),
        Key::F3 => b"\x1bOR".to_vec(),
        Key::F4 => b"\x1bOS".to_vec(),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        Key::Space if m.ctrl => vec![0],
        // Ctrl with a letter: its control character (Ctrl+D ends the shell, Ctrl+R searches the history, ...)
        k if m.ctrl && !m.alt => {
            let name = k.name();
            let c = name.chars().next().filter(|_| name.len() == 1)?;
            match c {
                'A'..='Z' if !matches!(c, 'C' | 'V' | 'X') => vec![c as u8 & 0x1f],
                '[' => vec![0x1b],
                '\\' => vec![0x1c],
                ']' => vec![0x1d],
                _ => return None,
            }
        }
        // Alt with a letter: Escape before it (readline's word moves: Alt+B, Alt+F)
        k if m.alt && !m.ctrl => {
            let name = k.name();
            let c = name.chars().next().filter(|_| name.len() == 1 && name.chars().all(|c| c.is_ascii_alphanumeric()))?;
            let c = if m.shift { c } else { c.to_ascii_lowercase() };
            vec![0x1b, c as u8]
        }
        _ => return None,
    };
    Some(bytes)
}

/// The colour of a cell: one of the 16 colours of the theme, the 256-colour cube and greys, or as given.
fn color(c: vt100::Color, fallback: Color32, pal: &Palette) -> Color32 {
    const DARK: [u32; 16] = [0x3b4046, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xc8ccd4, 0x5c6370, 0xf07a84, 0xb5e890, 0xf2d48b, 0x7cc2ff, 0xdc9af0, 0x6fd2de, 0xffffff];
    const LIGHT: [u32; 16] = [0x383a42, 0xd8463a, 0x3f8f3e, 0xb07800, 0x3a6fe0, 0x9e2aa0, 0x0b7fae, 0x80838c, 0x696c77, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0x202227];
    let rgb = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
    match c {
        vt100::Color::Default => fallback,
        vt100::Color::Idx(i) if i < 16 => rgb(if pal.dark { DARK[i as usize] } else { LIGHT[i as usize] }),
        vt100::Color::Idx(i) if i < 232 => {
            let i = i - 16;
            let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            Color32::from_rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        vt100::Color::Idx(i) => {
            let g = 8 + (i - 232) * 10;
            Color32::from_rgb(g, g, g)
        }
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    }
}

/// What a frame of the shell's view asks of the window.
#[derive(Default)]
pub struct ViewOut {
    /// The view has the keyboard.
    pub focused: bool,
    /// Text was put on the clipboard.
    pub copied: Option<String>,
    /// Enter was pressed after the shell ended: start a new one.
    pub restart: bool,
}

/// Draw the shell into `rect` and give it the keyboard, the mouse and the clipboard while it has the focus.
pub fn view(ui: &mut egui::Ui, rect: Rect, shell: &mut Shell, pal: &Palette, font_size: f32) -> ViewOut {
    let mut out = ViewOut::default();
    let font = egui::FontId::monospace(font_size);
    let (cell_w, line_h) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    let inner = rect.shrink2(Vec2::new(10.0, 6.0));
    let cols = ((inner.width() / cell_w).floor() as u16).clamp(10, 1000);
    let rows = ((inner.height() / line_h).floor() as u16).clamp(3, 500);
    shell.resize(rows, cols);

    let id = egui::Id::unique("linux_shell_view");
    let resp = ui.interact(rect, id, Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Text);
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    let focused = resp.has_focus();
    out.focused = focused;
    if focused {
        // Tab, the arrows and Escape belong to the shell, not to moving the focus between widgets
        ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
    }
    let cell_at = |p: Pos2| -> (u16, u16) {
        let r = ((p.y - inner.min.y) / line_h).floor().clamp(0.0, (rows - 1) as f32) as u16;
        let c = ((p.x - inner.min.x) / cell_w).floor().clamp(0.0, (cols - 1) as f32) as u16;
        (r, c)
    };
    // selecting with the mouse
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let at = cell_at(p);
        shell.selection = Some((at, at));
    }
    if resp.dragged()
        && let (Some(p), Some(sel)) = (resp.interact_pointer_pos(), shell.selection.as_mut())
    {
        sel.1 = cell_at(p);
    }
    if resp.clicked() {
        shell.selection = None;
    }
    // the wheel: back through what scrolled away, or the arrows of a full-screen program
    if resp.hovered() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy.abs() > 0.5 {
            let lines = ((dy.abs() / line_h).ceil() as usize).max(1);
            if shell.parser.screen().alternate_screen() {
                let key = if dy > 0.0 { "\x1b[A" } else { "\x1b[B" };
                shell.write(key.repeat(lines).as_bytes());
            } else {
                let back = shell.parser.screen().scrollback();
                let to = if dy > 0.0 { back + lines } else { back.saturating_sub(lines) };
                shell.parser.screen_mut().set_scrollback(to);
            }
        }
    }

    // the keyboard
    if focused {
        let events = ui.input(|i| i.events.clone());
        let app_cursor = shell.parser.screen().application_cursor();
        let mut typed = false;
        for event in &events {
            match event {
                Event::Text(text) => {
                    let text: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !text.is_empty() {
                        shell.write(text.as_bytes());
                        typed = true;
                    }
                }
                Event::Key { key, pressed: true, modifiers, .. } => {
                    if shell.exited.is_some() {
                        if *key == Key::Enter {
                            out.restart = true;
                        }
                        continue;
                    }
                    if modifiers.ctrl && matches!(key, Key::C | Key::V | Key::X) {
                        continue;
                    }
                    if let Some(bytes) = key_bytes(*key, *modifiers, app_cursor) {
                        shell.write(&bytes);
                        typed = true;
                    }
                }
                // Ctrl+C copies what is selected; with nothing selected it interrupts, as in a terminal
                Event::Copy => match shell.selected_text() {
                    Some(text) => {
                        out.copied = Some(text);
                        shell.selection = None;
                    }
                    None => {
                        shell.write(b"\x03");
                        typed = true;
                    }
                },
                Event::Cut => {
                    shell.write(b"\x18");
                    typed = true;
                }
                Event::Paste(text) => {
                    shell.paste(text);
                    typed = true;
                }
                _ => {}
            }
        }
        // what was typed is the shell's alone
        ui.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Text(_) | Event::Key { .. } | Event::Copy | Event::Cut | Event::Paste(_))));
        if typed {
            // typing brings the view back to the bottom
            shell.parser.screen_mut().set_scrollback(0);
            shell.selection = None;
        }
    }

    // the screen
    let painter = ui.painter_at(rect);
    let screen = shell.parser.screen();
    let fallback_fg = pal.plain;
    let (selection_start, selection_end) = match shell.selection {
        Some((a, b)) if a != b => (Some(a.min(b)), Some(a.max(b))),
        _ => (None, None),
    };
    for row in 0..rows {
        let mut job = egui::text::LayoutJob::default();
        let y = inner.min.y + row as f32 * line_h;
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else { break };
            if cell.is_wide_continuation() {
                continue;
            }
            let (mut fg, mut bg) = (color(cell.fgcolor(), fallback_fg, pal), color(cell.bgcolor(), Color32::TRANSPARENT, pal));
            if cell.bold() && matches!(cell.fgcolor(), vt100::Color::Idx(0..8)) {
                // bold text in one of the first eight colours is shown in its bright version
                if let vt100::Color::Idx(i) = cell.fgcolor() {
                    fg = color(vt100::Color::Idx(i + 8), fallback_fg, pal);
                }
            }
            if cell.dim() {
                fg = fg.gamma_multiply(0.6);
            }
            if cell.inverse() {
                let back = if bg == Color32::TRANSPARENT { pal.editor_bg } else { bg };
                (fg, bg) = (back, fg);
            }
            let selected = match (selection_start, selection_end) {
                (Some(s), Some(e)) => (row, col) >= s && (row, col) <= e,
                _ => false,
            };
            if selected {
                bg = pal.selection;
            }
            let text = if cell.has_contents() { cell.contents() } else { " " };
            let format = egui::TextFormat { font_id: font.clone(), color: fg, background: bg, underline: if cell.underline() { Stroke::new(1.0, fg) } else { Stroke::NONE }, italics: cell.italic(), ..Default::default() };
            job.append(text, 0.0, format);
        }
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(Pos2::new(inner.min.x, y), galley, fallback_fg);
    }
    // the cursor: a block while the view has the keyboard, an outline otherwise
    if !screen.hide_cursor() && screen.scrollback() == 0 && shell.exited.is_none() {
        let (r, c) = screen.cursor_position();
        let cursor = Rect::from_min_size(Pos2::new(inner.min.x + c as f32 * cell_w, inner.min.y + r as f32 * line_h), Vec2::new(cell_w, line_h));
        if focused {
            painter.rect_filled(cursor, CornerRadius::same(2), pal.caret.gamma_multiply(0.55));
        } else {
            painter.rect_stroke(cursor, CornerRadius::same(2), Stroke::new(1.0, pal.text_dim), egui::StrokeKind::Inside);
        }
    }
    if screen.scrollback() > 0 {
        let note = crate::i18n::fmt("{} lines back - type or scroll down to return", &[&screen.scrollback()]);
        let g = painter.layout_no_wrap(note, crate::fonts::ui(11.5), pal.text_on_accent);
        let r = Rect::from_min_size(Pos2::new(rect.max.x - g.size().x - 30.0, rect.min.y + 6.0), g.size() + Vec2::new(16.0, 6.0));
        painter.rect_filled(r, CornerRadius::same(8), pal.accent.gamma_multiply(0.9));
        painter.galley(r.min + Vec2::new(8.0, 3.0), g, pal.text_on_accent);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_become_what_a_terminal_sends() {
        let none = Modifiers::NONE;
        assert_eq!(key_bytes(Key::Enter, none, false).unwrap(), b"\r");
        assert_eq!(key_bytes(Key::Backspace, none, false).unwrap(), b"\x7f");
        assert_eq!(key_bytes(Key::ArrowUp, none, false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes(Key::ArrowUp, none, true).unwrap(), b"\x1bOA");
        assert_eq!(key_bytes(Key::ArrowLeft, Modifiers::CTRL, false).unwrap(), b"\x1b[1;5D");
        assert_eq!(key_bytes(Key::Delete, none, false).unwrap(), b"\x1b[3~");
        assert_eq!(key_bytes(Key::Tab, Modifiers::SHIFT, false).unwrap(), b"\x1b[Z");
        assert_eq!(key_bytes(Key::D, Modifiers::CTRL, false).unwrap(), [4]);
        assert_eq!(key_bytes(Key::R, Modifiers::CTRL, false).unwrap(), [0x12]);
        assert_eq!(key_bytes(Key::B, Modifiers::ALT, false).unwrap(), b"\x1bb");
        assert_eq!(key_bytes(Key::F5, none, false).unwrap(), b"\x1b[15~");
        // copy, paste and cut come as their own events
        assert!(key_bytes(Key::C, Modifiers::CTRL, false).is_none());
        assert!(key_bytes(Key::V, Modifiers::CTRL, false).is_none());
        // plain letters are text, not keys
        assert!(key_bytes(Key::A, none, false).is_none());
    }

    #[test]
    fn colours_come_from_the_theme_and_the_256_colour_table() {
        let pal = Palette::dark();
        assert_eq!(color(vt100::Color::Default, Color32::RED, &pal), Color32::RED);
        assert_eq!(color(vt100::Color::Idx(1), Color32::RED, &pal), Color32::from_rgb(0xe0, 0x6c, 0x75));
        assert_eq!(color(vt100::Color::Idx(16), Color32::RED, &pal), Color32::from_rgb(0, 0, 0));
        assert_eq!(color(vt100::Color::Idx(231), Color32::RED, &pal), Color32::from_rgb(255, 255, 255));
        assert_eq!(color(vt100::Color::Idx(232), Color32::RED, &pal), Color32::from_rgb(8, 8, 8));
        assert_eq!(color(vt100::Color::Rgb(1, 2, 3), Color32::RED, &pal), Color32::from_rgb(1, 2, 3));
    }
}
