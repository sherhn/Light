//! Studio colours: neutral charcoal and soft sage accents, and the syntax colours of the familiar dark and light
//! IDE schemes (their Python overlay included) taken one to one.

use std::sync::LazyLock;

use eframe::egui::{self, Color32};
use ide_core::settings::{CustomTheme, Settings, Theme};
use lisp_syntax::highlight::Class;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    // chrome
    pub window_bg: Color32,   // main window / tool windows
    pub panel_bg: Color32,    // side panels
    pub editor_bg: Color32,
    pub gutter_bg: Color32,
    pub border: Color32,
    pub hover: Color32,
    pub selected: Color32,    // selected row in lists / trees
    pub accent: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_on_accent: Color32,
    pub tab_active_bg: Color32,
    pub popup_bg: Color32,
    pub input_bg: Color32,
    // glass look
    pub base_top: Color32,
    pub base_bottom: Color32,
    pub glass: Color32,
    pub sheen: Color32,
    /// The second colour of the brand (the first is `accent`): links, information, the blue glow.
    pub blue: Color32,
    /// The glows behind the glass: top-left and bottom-right.
    pub glow_a: Color32,
    pub glow_b: Color32,
    // editor
    pub caret: Color32,
    pub line_number: Color32,
    pub line_number_active: Color32,
    pub current_line: Color32,
    pub selection: Color32,
    pub match_bracket: Color32,
    pub find_match: Color32,
    pub find_current: Color32,
    pub indent_guide: Color32,
    pub whitespace: Color32,
    // semantic
    pub error: Color32,
    pub warning: Color32,
    pub weak_warning: Color32,
    pub ok: Color32,
    pub modified: Color32,
    pub git_modified: Color32,
    pub git_added: Color32,
    pub git_untracked: Color32,
    // syntax
    pub plain: Color32,
    pub comment: Color32,
    pub string: Color32,
    pub number: Color32,
    pub keyword_form: Color32,
    pub builtin: Color32,
    pub definition: Color32,
    pub keyword_arg: Color32,
    pub constant: Color32,
    pub special_var: Color32,
    pub prefix: Color32,
    pub type_name: Color32,
    pub preproc: Color32,
    pub heading: Color32,
    pub strong: Color32,
    pub emphasis: Color32,
    pub code: Color32,
    pub link: Color32,
    pub quote: Color32,
    /// Marker comments: `TODO`, `INFO`, `NOTE`... and `FIXME`, `ALERT`, `HACK`...
    pub todo: Color32,
    pub alert: Color32,
    pub rainbow: [Color32; 6],
}

const fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// One colour of the palette that the user's own theme can change: the key it is stored under (the name of the
/// field), the group and the name shown in Settings, and whether it is meant to be translucent.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub translucent: bool,
}

/// Lists the colours of the palette once: the table of [`SLOTS`] and the lookup by key come from the same lines.
macro_rules! palette_slots {
    ($($group:literal { $($field:ident $label:literal $alpha:literal),* $(,)? })*) => {
        const FIELD_SLOTS: &[Slot] = &[$($(Slot { key: stringify!($field), group: $group, label: $label, translucent: $alpha != 0 }),*),*];

        impl Palette {
            fn field(&self, key: &str) -> Option<Color32> {
                match key {
                    $($(stringify!($field) => Some(self.$field),)*)*
                    _ => None,
                }
            }

            fn set_field(&mut self, key: &str, color: Color32) -> bool {
                match key {
                    $($(stringify!($field) => self.$field = color,)*)*
                    _ => return false,
                }
                true
            }
        }
    };
}

palette_slots! {
    "Window and panels" {
        window_bg "Window background" 0,
        panel_bg "Side panels" 1,
        editor_bg "Editor background" 0,
        gutter_bg "Gutter" 0,
        popup_bg "Popups and menus" 0,
        input_bg "Input fields" 0,
        tab_active_bg "Active tab" 1,
        border "Borders" 1,
        hover "Hover highlight" 1,
        selected "Selected row" 1,
        accent "Accent" 0,
        text_on_accent "Text on accent" 0,
        text "Text" 0,
        text_dim "Secondary text" 0,
        blue "Second brand color (links, information)" 0,
    }
    "Glass and glow" {
        base_top "Background, top" 0,
        base_bottom "Background, bottom" 0,
        glass "Glass cards" 1,
        sheen "Glass sheen" 1,
        glow_a "Glow, top left" 0,
        glow_b "Glow, bottom right" 0,
    }
    "Editor" {
        caret "Caret" 0,
        line_number "Line numbers" 0,
        line_number_active "Current line number" 0,
        current_line "Current line" 1,
        selection "Selection" 1,
        match_bracket "Matching bracket" 1,
        find_match "Search match" 0,
        find_current "Current search match" 0,
        indent_guide "Indent guides" 1,
        whitespace "Whitespace marks" 1,
        todo "TODO, INFO, NOTE markers" 0,
        alert "FIXME, ALERT, HACK markers" 0,
    }
    "Messages and status" {
        error "Error" 0,
        warning "Warning" 0,
        weak_warning "Weak warning" 0,
        ok "Success" 0,
        modified "Modified file" 0,
        git_modified "Git: modified" 0,
        git_added "Git: added" 0,
        git_untracked "Git: untracked" 0,
    }
    "Syntax" {
        plain "Plain text" 0,
        comment "Comments" 0,
        string "Strings" 0,
        number "Numbers" 0,
        keyword_form "Special forms and control keywords" 0,
        keyword_arg "Keywords (:key)" 0,
        builtin "Built-in functions" 0,
        definition "Definitions" 0,
        constant "Constants" 0,
        special_var "Special variables" 0,
        prefix "Prefixes and reader macros" 0,
        type_name "Types" 0,
        preproc "Preprocessor" 0,
        heading "Headings" 0,
        strong "Bold" 0,
        emphasis "Italic" 0,
        code "Inline code" 0,
        link "Links" 0,
        quote "Quotes" 0,
    }
}

/// The six colours of nested brackets are slots of their own, `rainbow1` to `rainbow6`.
const RAINBOW_SLOTS: [Slot; 6] = [
    Slot { key: "rainbow1", group: "Rainbow brackets", label: "Nesting level 1", translucent: false },
    Slot { key: "rainbow2", group: "Rainbow brackets", label: "Nesting level 2", translucent: false },
    Slot { key: "rainbow3", group: "Rainbow brackets", label: "Nesting level 3", translucent: false },
    Slot { key: "rainbow4", group: "Rainbow brackets", label: "Nesting level 4", translucent: false },
    Slot { key: "rainbow5", group: "Rainbow brackets", label: "Nesting level 5", translucent: false },
    Slot { key: "rainbow6", group: "Rainbow brackets", label: "Nesting level 6", translucent: false },
];

/// Every colour a custom theme can change, grouped as Settings shows them.
pub static SLOTS: LazyLock<Vec<Slot>> = LazyLock::new(|| FIELD_SLOTS.iter().chain(RAINBOW_SLOTS.iter()).copied().collect());

/// Which of the rainbow colours `key` names.
fn rainbow_index(key: &str) -> Option<usize> {
    key.strip_prefix("rainbow").and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=6).contains(n)).map(|n| n - 1)
}

impl Palette {
    pub fn dark() -> Palette {
        Palette {
            dark: true,
            window_bg: c(0x101213),
            // glass: the ground shows through
            panel_bg: Color32::from_rgba_unmultiplied(24, 27, 29, 244),
            editor_bg: c(0x121416),
            gutter_bg: c(0x121416),
            border: Color32::from_rgb(10, 13, 11),
            hover: Color32::from_rgba_unmultiplied(179, 214, 173, 18),
            selected: Color32::from_rgba_unmultiplied(179, 214, 173, 35),
            accent: c(0xB3D6AD),
            text: c(0xE5E9E7),
            text_dim: c(0x9AA49E),
            text_on_accent: c(0x172019),
            tab_active_bg: Color32::from_rgba_unmultiplied(179, 214, 173, 23),
            popup_bg: c(0x1D2123),
            input_bg: c(0x141719),
            base_top: c(0x141718),
            base_bottom: c(0x101213),
            // dialogs hold dense text over a busy screen, and there is no blur: more see-through than this and the
            // words behind show through the words in front
            glass: Color32::from_rgba_unmultiplied(29, 33, 35, 252),
            sheen: Color32::from_rgba_unmultiplied(224, 233, 227, 24),
            blue: c(0x9FBACA),
            glow_a: c(0x26362D),
            glow_b: c(0x242F34),
            caret: c(0xC1E0B9),
            line_number: c(0x7F8C84),
            line_number_active: c(0xBBC7BD),
            current_line: Color32::from_rgba_unmultiplied(179, 214, 173, 14),
            selection: Color32::from_rgba_unmultiplied(151, 181, 164, 65),
            match_bracket: Color32::from_rgba_unmultiplied(179, 214, 173, 55),
            find_match: c(0x343E2D),
            find_current: c(0x506044),
            indent_guide: Color32::from_rgba_unmultiplied(213, 225, 218, 16),
            whitespace: Color32::from_rgba_unmultiplied(213, 225, 218, 44),
            error: c(0xF75464),
            warning: c(0xD6BF8D),
            weak_warning: c(0x9AA49E),
            ok: c(0xB3D6AD),
            modified: c(0x9FBACA),
            git_modified: c(0x7FB6E6),
            git_added: c(0x92DA88),
            git_untracked: c(0xE5A574),
            plain: c(0xBCBEC4),
            comment: c(0x7A7E85),
            string: c(0x6AAB73),
            number: c(0x2AACB8),
            keyword_form: c(0xCF8E6D),
            builtin: c(0x8888C6),
            definition: c(0x56A8F5),
            keyword_arg: c(0xAA4926),
            constant: c(0xC77DBB),
            special_var: c(0xB200B2),
            prefix: c(0xBCBEC4),
            type_name: c(0xBCBEC4),
            preproc: c(0xB3AE60),
            heading: c(0xCF8E6D),
            strong: c(0xBCBEC4),
            emphasis: c(0xBCBEC4),
            code: c(0xBCBEC4),
            link: c(0x56A8F5),
            quote: c(0x7A7E85),
            todo: c(0x8BB33D),
            alert: c(0x8BB33D),
            rainbow: [c(0xF2C744), c(0xCE97F8), c(0x6FBBFF), c(0x8FDB63), c(0xFF9E66), c(0x5ADBD0)],
        }
    }

    pub fn light() -> Palette {
        Palette {
            dark: false,
            window_bg: c(0xEDF0EC),
            panel_bg: Color32::from_rgba_unmultiplied(244, 246, 242, 244),
            editor_bg: c(0xF8FAF6),
            gutter_bg: c(0xF8FAF6),
            border: Color32::from_rgba_unmultiplied(42, 57, 46, 100),
            hover: Color32::from_rgba_unmultiplied(75, 105, 79, 16),
            selected: Color32::from_rgba_unmultiplied(75, 105, 79, 30),
            accent: c(0x46674B),
            text: c(0x26322B),
            text_dim: c(0x637168),
            text_on_accent: c(0xF8FAF6),
            tab_active_bg: Color32::from_rgba_unmultiplied(75, 105, 79, 23),
            popup_bg: c(0xF8FAF6),
            input_bg: c(0xE6EBE4),
            base_top: c(0xF3F5F0),
            base_bottom: c(0xEAEEEA),
            glass: Color32::from_rgba_unmultiplied(244, 246, 242, 252),
            sheen: Color32::from_rgba_unmultiplied(255, 255, 255, 160),
            blue: c(0x486779),
            glow_a: c(0xCEDBCE),
            glow_b: c(0xD6DEE0),
            caret: c(0x26322B),
            line_number: c(0x718176),
            line_number_active: c(0x526658),
            current_line: Color32::from_rgba_unmultiplied(75, 105, 79, 16),
            selection: Color32::from_rgba_unmultiplied(113, 143, 125, 55),
            match_bracket: Color32::from_rgba_unmultiplied(75, 105, 79, 44),
            find_match: c(0xE2E8CE),
            find_current: c(0xC8D5AC),
            indent_guide: Color32::from_rgba_unmultiplied(42, 57, 46, 20),
            whitespace: Color32::from_rgba_unmultiplied(42, 57, 46, 55),
            error: c(0xF50000),
            warning: c(0x876621),
            weak_warning: c(0x6F7B72),
            ok: c(0x46674B),
            modified: c(0x486779),
            git_modified: c(0x2B6592),
            git_added: c(0x2F7A3A),
            git_untracked: c(0x9B5224),
            plain: c(0x080808),
            comment: c(0x8C8C8C),
            string: c(0x067D17),
            number: c(0x1750EB),
            keyword_form: c(0x0033B3),
            builtin: c(0x000080),
            definition: c(0x00627A),
            keyword_arg: c(0x660099),
            constant: c(0x871094),
            special_var: c(0xB200B2),
            prefix: c(0x080808),
            type_name: c(0x080808),
            preproc: c(0x9E880D),
            heading: c(0x0033B3),
            strong: c(0x080808),
            emphasis: c(0x080808),
            code: c(0x080808),
            link: c(0x006DCC),
            quote: c(0x8C8C8C),
            todo: c(0x008DDE),
            alert: c(0x008DDE),
            rainbow: [c(0x9A6A00), c(0x8A2FC9), c(0x0F62D0), c(0x0E7D18), c(0xC2410C), c(0x0A7E8C)],
        }
    }

    pub fn for_theme(t: Theme) -> Palette {
        match t {
            Theme::Dark | Theme::Custom => Palette::dark(),
            Theme::Light => Palette::light(),
        }
    }

    /// The palette the settings ask for, the user's own theme included.
    pub fn for_settings(s: &Settings) -> Palette {
        let palette = match s.theme {
            Theme::Custom => Palette::custom(&s.custom_theme),
            t => Palette::for_theme(t),
        };
        let mut palette=palette.with_contrast(s.theme_contrast);
        if !s.show_borders { palette.border=Color32::TRANSPARENT; }
        palette
    }

    /// Adjust readability while preserving backgrounds, hues and the user's stored color definitions. Brightening
    /// nudges the editor text towards white or black only a little: the syntax colours are vivid, and washing them
    /// out would make them pale, not readable. The chrome text stays on the stronger mix it always had.
    pub fn with_contrast(mut self, percent: u16) -> Palette {
        let amount = percent.clamp(75, 300) as f32 / 100.0;
        if amount == 1.0 { return self }
        fn foreground(color: Color32, background: Color32, amount: f32, chrome: bool) -> Color32 {
            let [r, g, b, a] = color.to_srgba_unmultiplied();
            let bright = 0.2126 * background.r() as f32 + 0.7152 * background.g() as f32 + 0.0722 * background.b() as f32;
            let pole = if bright < 128.0 { Color32::WHITE } else { Color32::BLACK };
            let (target, weight) = if amount > 1.0 { (pole, (1.0 - 1.0 / amount) * if chrome { 0.65 } else { 0.25 }) } else { (background, (1.0 - amount) * 0.65) };
            let mix = |value: u8, target: u8| (value as f32 + (target as f32 - value as f32) * weight).round() as u8;
            Color32::from_rgba_unmultiplied(mix(r, target.r()), mix(g, target.g()), mix(b, target.b()), a)
        }
        for slot in SLOTS.iter() {
            let editor = matches!(slot.group, "Syntax" | "Messages and status" | "Rainbow brackets") || matches!(slot.key, "caret" | "line_number" | "line_number_active" | "todo" | "alert");
            let chrome = matches!(slot.key, "text" | "text_dim" | "accent" | "blue");
            if (editor || chrome) && let Some(color) = self.color(slot.key) {
                self.set_color(slot.key, foreground(color, if editor { self.editor_bg } else { self.window_bg }, amount, chrome));
            }
        }
        self.text_on_accent = foreground(self.text_on_accent, self.accent, amount, true);
        for key in ["hover", "selected", "tab_active_bg", "current_line", "selection", "match_bracket", "indent_guide", "whitespace"] {
            let [r,g,b,a] = self.color(key).unwrap().to_srgba_unmultiplied();
            self.set_color(key, Color32::from_rgba_unmultiplied(r,g,b,(a as f32 * amount).round().min(255.0) as u8));
        }
        self
    }

    /// The palette `theme` starts from, with the colours it changed put over it (a colour that is not a hex code, or a
    /// key no slot has, is ignored).
    pub fn custom(theme: &CustomTheme) -> Palette {
        let mut p = if theme.dark { Palette::dark() } else { Palette::light() };
        for key in theme.colors.keys() {
            if let Some([r, g, b, a]) = theme.get(key) {
                p.set_color(key, Color32::from_rgba_unmultiplied(r, g, b, a));
            }
        }
        p
    }

    /// The colour of the slot `key` (see [`SLOTS`]).
    pub fn color(&self, key: &str) -> Option<Color32> {
        match rainbow_index(key) {
            Some(i) => Some(self.rainbow[i]),
            None => self.field(key),
        }
    }

    /// Change the colour of the slot `key`; `false`: there is no such slot.
    pub fn set_color(&mut self, key: &str, color: Color32) -> bool {
        match rainbow_index(key) {
            Some(i) => {
                self.rainbow[i] = color;
                true
            }
            None => self.set_field(key, color),
        }
    }

    pub fn syntax(&self, class: Class, depth: u32, rainbow: bool) -> Color32 {
        match class {
            Class::Plain => self.plain,
            Class::Comment => self.comment,
            Class::String | Class::Char => self.string,
            Class::Number => self.number,
            Class::Keyword => self.keyword_arg,
            Class::SpecialForm => self.keyword_form,
            Class::Builtin => self.builtin,
            Class::Constant => self.constant,
            Class::SpecialVar | Class::ConstName => self.special_var,
            Class::Definition => self.definition,
            Class::Prefix => self.prefix,
            Class::Bracket => {
                if rainbow {
                    self.rainbow[depth as usize % self.rainbow.len()]
                } else {
                    self.plain
                }
            }
            Class::Error => self.error,
            Class::Type => self.type_name,
            Class::Preproc => self.preproc,
            Class::Heading => self.heading,
            Class::Strong => self.strong,
            Class::Emphasis => self.emphasis,
            Class::Code => self.code,
            Class::Link => self.link,
            Class::Quote => self.quote,
        }
    }

    /// Apply the palette to egui's global style.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.panel_fill = self.panel_bg;
        v.window_fill = self.popup_bg.to_opaque();
        v.extreme_bg_color = self.input_bg;
        v.text_edit_bg_color = Some(self.input_bg);
        v.faint_bg_color = self.hover;
        v.window_stroke = egui::Stroke::NONE;
        // flat design: popups, menus, tooltips and windows carry no shadow
        v.window_shadow = egui::Shadow::NONE;
        v.popup_shadow = egui::Shadow::NONE;
        v.weak_text_color = Some(self.text_dim);
        v.selection.bg_fill = self.selection;
        v.selection.stroke = egui::Stroke::new(1.0, self.accent);
        v.hyperlink_color = self.accent;
        v.window_corner_radius = egui::CornerRadius::same(12);
        v.menu_corner_radius = egui::CornerRadius::same(8);
        let r = egui::CornerRadius::same(6);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = r;
        }
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.fg_stroke = egui::Stroke::new(1.0, self.text);
        }
        v.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
        v.widgets.noninteractive.bg_fill = self.panel_bg;
        v.widgets.inactive.bg_fill = self.hover;
        v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
        v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
        v.widgets.hovered.bg_fill = self.hover;
        v.widgets.hovered.weak_bg_fill = self.hover;
        v.widgets.hovered.bg_stroke = egui::Stroke::NONE;
        v.widgets.active.bg_fill = self.selected;
        v.widgets.active.weak_bg_fill = self.selected;
        v.widgets.open.bg_fill = self.hover;
        v.widgets.open.weak_bg_fill = self.hover;
        ctx.set_theme(if self.dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light });
        ctx.set_visuals_of(if self.dark { egui::Theme::Dark } else { egui::Theme::Light }, v);
        ctx.all_styles_mut(|style| {
            // Menus and dialogs must never fade through the code underneath.
            style.animation_time = 0.0;
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            style.spacing.interact_size = egui::vec2(36.0, 30.0);
            style.spacing.menu_margin = egui::Margin::same(6);
            style.spacing.window_margin = egui::Margin::same(12);
            style.spacing.scroll = crate::scrollbars::style();
            style.interaction.selectable_labels = false;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color32) -> f32 {
        let channel = |v: u8| { let s = v as f32 / 255.0; if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) } };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }
    fn contrast(a: Color32, b: Color32) -> f32 {
        let (a,b) = (luminance(a),luminance(b)); (a.max(b)+0.05)/(a.min(b)+0.05)
    }
    #[test]
    fn contrast_increases_readability_in_both_palettes_and_keeps_selection_legible() {
        for base in [Palette::dark(), Palette::light()] {
            // the familiar light IDE scheme itself reads comments at 3.2:1 on white: this bar only asks that
            // comments stay clearly secondary and yet readable
            assert!(contrast(base.comment, base.editor_bg) >= 3.0);
            assert!(contrast(base.line_number, base.gutter_bg) >= 3.5);
            let low = base.with_contrast(75); let high = base.with_contrast(150);
            for key in ["plain", "text_dim", "comment", "keyword_form", "string", "line_number"] {
                assert!(contrast(low.color(key).unwrap(), base.editor_bg) < contrast(base.color(key).unwrap(), base.editor_bg), "{key}");
                assert!(contrast(high.color(key).unwrap(), base.editor_bg) > contrast(base.color(key).unwrap(), base.editor_bg), "{key}");
            }
            assert_eq!(high.editor_bg, base.editor_bg);
            assert_eq!(high.border, base.border, "outlines stay dark at every contrast");
            let [r,g,b,a] = high.selection.to_srgba_unmultiplied();
            let mix = |fg: u8, bg: u8| ((fg as u32 * a as u32 + bg as u32 * (255-a as u32))/255) as u8;
            let selection = Color32::from_rgb(mix(r,high.editor_bg.r()),mix(g,high.editor_bg.g()),mix(b,high.editor_bg.b()));
            assert!(contrast(high.plain, selection) >= 4.5);
            assert!(contrast(high.text_on_accent, high.accent) >= 4.5);
            assert_eq!(format!("{:?}",base.with_contrast(100)), format!("{base:?}"));
        }
    }
    #[test]
    fn bright_default_and_optional_outlines_keep_text_readable() {
        for theme in [Theme::Dark,Theme::Light] {
            let mut settings=Settings {theme,..Default::default()};
            assert_eq!(settings.theme_contrast,300);
            let palette=Palette::for_settings(&settings);
            assert!(contrast(palette.plain,palette.editor_bg)>=7.0);
            assert!(palette.border.r()<80 && palette.border.g()<80 && palette.border.b()<80);
            settings.show_borders=false;
            let hidden=Palette::for_settings(&settings);
            assert_eq!(hidden.border,Color32::TRANSPARENT);
            assert_eq!(hidden.plain,palette.plain);
            assert_ne!(palette.keyword_form,palette.string);
        }
    }
    #[test]
    fn syntax_colors_match_the_familiar_ide_schemes() {
        // the well-known "Dark" and "Light" editor schemes of the popular IDEs (their Python overlay included),
        // taken one to one
        let d = Palette::dark();
        assert_eq!(d.plain, c(0xBCBEC4));
        assert_eq!(d.comment, c(0x7A7E85));
        assert_eq!(d.string, c(0x6AAB73));
        assert_eq!(d.number, c(0x2AACB8));
        assert_eq!(d.keyword_form, c(0xCF8E6D));
        assert_eq!(d.keyword_arg, c(0xAA4926));
        assert_eq!(d.builtin, c(0x8888C6));
        assert_eq!(d.definition, c(0x56A8F5));
        assert_eq!(d.constant, c(0xC77DBB));
        assert_eq!(d.special_var, c(0xB200B2));
        assert_eq!(d.preproc, c(0xB3AE60));
        assert_eq!(d.todo, c(0x8BB33D));
        assert_eq!(d.error, c(0xF75464));
        let l = Palette::light();
        assert_eq!(l.plain, c(0x080808));
        assert_eq!(l.comment, c(0x8C8C8C));
        assert_eq!(l.string, c(0x067D17));
        assert_eq!(l.number, c(0x1750EB));
        assert_eq!(l.keyword_form, c(0x0033B3));
        assert_eq!(l.keyword_arg, c(0x660099));
        assert_eq!(l.builtin, c(0x000080));
        assert_eq!(l.definition, c(0x00627A));
        assert_eq!(l.constant, c(0x871094));
        assert_eq!(l.special_var, c(0xB200B2));
        assert_eq!(l.preproc, c(0x9E880D));
        assert_eq!(l.todo, c(0x008DDE));
        assert_eq!(l.error, c(0xF50000));
        // the six colours of the brackets stay distinct from one another
        for (i, a) in d.rainbow.iter().enumerate() {
            for b in d.rainbow.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }
    #[test]
    fn contrast_never_rewrites_the_users_custom_colors() {
        let mut settings = Settings { theme: Theme::Custom, ..Default::default() };
        settings.custom_theme.set("comment", [100, 150, 120, 255]);
        let saved = settings.custom_theme.clone(); let normal = Palette::for_settings(&settings);
        settings.theme_contrast = 150;
        assert_ne!(Palette::for_settings(&settings).comment, normal.comment);
        assert_eq!(settings.custom_theme, saved);
        settings.theme_contrast = 100;
        assert_eq!(Palette::for_settings(&settings).comment, Color32::from_rgb(100,150,120));
    }

    #[test]
    fn dialog_and_popup_surfaces_remain_opaque_with_transparent_custom_colors() {
        let mut palette = Palette::dark();
        palette.popup_bg = Color32::from_rgba_unmultiplied(30, 40, 50, 20);
        palette.glass = Color32::from_rgba_unmultiplied(50, 60, 70, 1);
        assert_eq!(crate::widgets::glass_frame(&palette).fill.a(), 255);
        let ctx = egui::Context::default();
        palette.apply(&ctx);
        assert_eq!(ctx.style_of(egui::Theme::Dark).visuals.window_fill.a(), 255);
        assert_eq!(ctx.global_style().animation_time, 0.0);
    }

    #[test]
    fn palettes_are_distinct_and_readable() {
        let (d, l) = (Palette::dark(), Palette::light());
        assert!(d.dark && !l.dark);
        assert_ne!(d.editor_bg, l.editor_bg);
        // text must contrast against the editor background
        fn lum(c: Color32) -> f32 {
            0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32
        }
        for p in [d, l] {
            assert!((lum(p.plain) - lum(p.editor_bg)).abs() > 100.0);
            assert!((lum(p.text) - lum(p.panel_bg)).abs() > 100.0);
            assert!((lum(p.comment) - lum(p.editor_bg)).abs() > 40.0);
        }
    }

    #[test]
    fn every_colour_of_the_palette_is_a_slot_of_the_custom_theme() {
        // the field names come from the derived Debug output, so a colour added to `Palette` and forgotten in the
        // table of slots fails here
        let dump = format!("{:?}", Palette::dark());
        let fields: Vec<&str> = dump.split(", ").filter_map(|part| part.split_once(": ")).filter_map(|(name, _)| name.rsplit(' ').next()).filter(|f| !matches!(*f, "dark" | "rainbow")).collect();
        assert!(fields.len() > 50, "the field names were not found in {dump}");
        for f in &fields {
            assert!(SLOTS.iter().any(|s| s.key == *f), "the colour {f} has no slot");
        }
        assert_eq!(FIELD_SLOTS.len(), fields.len(), "a slot for a colour the palette does not have");
        assert_eq!(SLOTS.len(), fields.len() + 6, "and the six colours of the rainbow brackets");
        let mut seen = std::collections::HashSet::new();
        for s in SLOTS.iter() {
            assert!(seen.insert(s.key), "slot {} twice", s.key);
            for p in [Palette::dark(), Palette::light()] {
                assert!(p.color(s.key).is_some(), "slot {} names no colour", s.key);
            }
        }
        assert_eq!(Palette::dark().color("nonsense"), None);
    }

    #[test]
    fn a_custom_theme_changes_only_what_it_names() {
        let base = Palette::dark();
        let mut theme = CustomTheme::default();
        assert_eq!(format!("{:?}", Palette::custom(&theme)), format!("{base:?}"), "nothing changed, nothing differs");
        theme.set("accent", [1, 2, 3, 255]);
        theme.set("hover", [10, 20, 30, 40]);
        theme.set("rainbow3", [9, 9, 9, 255]);
        theme.colors.insert("selection".into(), "nonsense".into());
        theme.colors.insert("no_such_slot".into(), "#FFFFFF".into());
        let p = Palette::custom(&theme);
        assert_eq!(p.accent, Color32::from_rgb(1, 2, 3));
        assert_eq!(p.hover, Color32::from_rgba_unmultiplied(10, 20, 30, 40));
        assert_eq!(p.rainbow[2], Color32::from_rgb(9, 9, 9));
        assert_eq!(p.selection, base.selection, "a broken colour is left alone");
        assert_eq!(p.text, base.text);
        assert!(p.dark);
        // the light palette as the starting point
        theme.dark = false;
        let l = Palette::custom(&theme);
        assert!(!l.dark);
        assert_eq!(l.text, Palette::light().text);
        assert_eq!(l.accent, Color32::from_rgb(1, 2, 3));
    }

    #[test]
    fn colours_of_slots_read_back_as_written() {
        let mut p = Palette::dark();
        for (i, s) in SLOTS.iter().enumerate() {
            let c = Color32::from_rgb(i as u8, 100, 200);
            assert!(p.set_color(s.key, c));
            assert_eq!(p.color(s.key), Some(c), "{}", s.key);
        }
        assert!(!p.set_color("rainbow7", Color32::RED));
    }

    #[test]
    fn rainbow_cycles() {
        let p = Palette::dark();
        assert_eq!(p.syntax(Class::Bracket, 0, true), p.syntax(Class::Bracket, 6, true));
        assert_ne!(p.syntax(Class::Bracket, 0, true), p.syntax(Class::Bracket, 1, true));
        assert_eq!(p.syntax(Class::Bracket, 3, false), p.plain);
    }
}
