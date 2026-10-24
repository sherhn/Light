//! The colours of Light: black, a green in the spirit of Sber's, a blue, and liquid glass - translucent cards with a
//! light rim over a dark ground that glows green in one corner and blue in the other. A dark scheme ("Night") and a
//! light one ("Day") with the same hues.

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
            window_bg: c(0x080B0C),
            // glass: the ground shows through
            panel_bg: Color32::from_rgba_unmultiplied(22, 30, 31, 214),
            editor_bg: c(0x0B1011),
            gutter_bg: c(0x0E1415),
            border: Color32::from_white_alpha(18),
            hover: Color32::from_rgba_unmultiplied(170, 255, 205, 20),
            selected: Color32::from_rgba_unmultiplied(45, 190, 96, 52),
            accent: c(0x2DBE60),
            text: c(0xE9EFEC),
            text_dim: c(0x8A9892),
            text_on_accent: c(0x04140A),
            tab_active_bg: Color32::from_rgba_unmultiplied(170, 255, 205, 30),
            popup_bg: c(0x141B1C),
            input_bg: c(0x1A2324),
            base_top: c(0x0B1412),
            base_bottom: c(0x070A0F),
            // dialogs hold dense text over a busy screen, and there is no blur: more see-through than this and the
            // words behind show through the words in front
            glass: Color32::from_rgba_unmultiplied(24, 33, 34, 252),
            sheen: Color32::from_white_alpha(46),
            blue: c(0x3B82F6),
            glow_a: c(0x1FD66B),
            glow_b: c(0x2F6BFF),
            caret: c(0x5CF0A0),
            line_number: c(0x3E4C47),
            line_number_active: c(0xA4B6AE),
            current_line: Color32::from_rgba_unmultiplied(170, 255, 205, 10),
            selection: Color32::from_rgba_unmultiplied(59, 130, 246, 92),
            match_bracket: Color32::from_rgba_unmultiplied(45, 190, 96, 80),
            find_match: c(0x1F4A33),
            find_current: c(0x2A6B47),
            indent_guide: Color32::from_white_alpha(12),
            whitespace: Color32::from_white_alpha(36),
            error: c(0xFF5C6C),
            warning: c(0xF5C451),
            weak_warning: c(0x6F7F78),
            ok: c(0x2DBE60),
            modified: c(0x3B82F6),
            git_modified: c(0x5B9CFF),
            git_added: c(0x2DBE60),
            git_untracked: c(0xFF7B6B),
            plain: c(0xCFD8D4),
            comment: c(0x62736C),
            string: c(0x7ED492),
            number: c(0x5AD1E6),
            keyword_form: c(0x6BA6FF),
            builtin: c(0x3DDC97),
            definition: c(0xF5D67A),
            keyword_arg: c(0xB78CFF),
            constant: c(0xFF9E7A),
            special_var: c(0xB78CFF),
            prefix: c(0xFF9E7A),
            type_name: c(0x4FD8C8),
            preproc: c(0xC7B25A),
            heading: c(0x35D07F),
            strong: c(0xFFFFFF),
            emphasis: c(0xA7C8FF),
            code: c(0x6FE0C8),
            link: c(0x5CA0FF),
            quote: c(0x8A9892),
            todo: c(0x6CB6FF),
            alert: c(0xFFB454),
            rainbow: [c(0xF5D67A), c(0xB78CFF), c(0x6BA6FF), c(0x7ED492), c(0xFF9E7A), c(0x5AD1E6)],
        }
    }

    pub fn light() -> Palette {
        Palette {
            dark: false,
            window_bg: c(0xE3ECE8),
            panel_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 190),
            editor_bg: c(0xFBFDFC),
            gutter_bg: c(0xF1F6F3),
            border: Color32::from_black_alpha(18),
            hover: Color32::from_rgba_unmultiplied(20, 120, 70, 18),
            selected: Color32::from_rgba_unmultiplied(33, 160, 56, 40),
            accent: c(0x1E9A44),
            text: c(0x0E1613),
            text_dim: c(0x5F6D67),
            text_on_accent: c(0xFFFFFF),
            tab_active_bg: Color32::from_rgba_unmultiplied(20, 120, 70, 26),
            popup_bg: c(0xFFFFFF),
            input_bg: c(0xE8F0EC),
            base_top: c(0xEAF3EE),
            base_bottom: c(0xDDE8F2),
            glass: Color32::from_rgba_unmultiplied(255, 255, 255, 252),
            sheen: Color32::from_white_alpha(220),
            blue: c(0x2563EB),
            glow_a: c(0x3FD487),
            glow_b: c(0x5B8CFF),
            caret: c(0x0E1613),
            line_number: c(0xB2BFB8),
            line_number_active: c(0x56655E),
            current_line: Color32::from_rgba_unmultiplied(33, 160, 56, 14),
            selection: Color32::from_rgba_unmultiplied(37, 99, 235, 66),
            match_bracket: Color32::from_rgba_unmultiplied(33, 160, 56, 60),
            find_match: c(0xCDEBC9),
            find_current: c(0xA9DBA3),
            indent_guide: Color32::from_black_alpha(14),
            whitespace: Color32::from_black_alpha(40),
            error: c(0xDB3B4B),
            warning: c(0xB88A0B),
            weak_warning: c(0x8C8C8C),
            ok: c(0x1E9A44),
            modified: c(0x2563EB),
            git_modified: c(0x2F5FC4),
            git_added: c(0x1E9A44),
            git_untracked: c(0xB5483B),
            plain: c(0x0B1210),
            comment: c(0x7E8C85),
            string: c(0x13792F),
            number: c(0x0A6E8C),
            keyword_form: c(0x1D4ED8),
            builtin: c(0x0B7A56),
            definition: c(0x8A5A00),
            keyword_arg: c(0x6D3FC4),
            constant: c(0xB4491E),
            special_var: c(0x6D3FC4),
            prefix: c(0xB4491E),
            type_name: c(0x00756A),
            preproc: c(0x8A6D00),
            heading: c(0x12823F),
            strong: c(0x0B0D10),
            emphasis: c(0x1F57B8),
            code: c(0x0B7A6B),
            link: c(0x1750EB),
            quote: c(0x6B707C),
            todo: c(0x1F6FD0),
            alert: c(0xC25E00),
            rainbow: [c(0xB8860B), c(0x7C3FC4), c(0x1D4ED8), c(0x1E8A3D), c(0xC25E00), c(0x00808A)],
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
        match s.theme {
            Theme::Custom => Palette::custom(&s.custom_theme),
            t => Palette::for_theme(t),
        }
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
        v.window_fill = self.popup_bg;
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
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            style.spacing.interact_size = egui::vec2(36.0, 30.0);
            style.spacing.menu_margin = egui::Margin::same(6);
            style.spacing.window_margin = egui::Margin::same(12);
            style.spacing.scroll = egui::style::ScrollStyle::floating();
            style.spacing.scroll.floating_width = 6.0;
            style.spacing.scroll.bar_width = 10.0;
            style.interaction.selectable_labels = false;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
