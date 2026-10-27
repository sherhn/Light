//! Studio colours: neutral charcoal, soft sage accents and restrained syntax colours.
//! The light palette uses the same hues on a quiet off-white ground.

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
            border: Color32::from_rgba_unmultiplied(213, 225, 218, 22),
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
            line_number: c(0x69746E),
            line_number_active: c(0xBBC7BD),
            current_line: Color32::from_rgba_unmultiplied(179, 214, 173, 9),
            selection: Color32::from_rgba_unmultiplied(151, 181, 164, 65),
            match_bracket: Color32::from_rgba_unmultiplied(179, 214, 173, 55),
            find_match: c(0x343E2D),
            find_current: c(0x506044),
            indent_guide: Color32::from_rgba_unmultiplied(213, 225, 218, 16),
            whitespace: Color32::from_rgba_unmultiplied(213, 225, 218, 44),
            error: c(0xE29797),
            warning: c(0xD6BF8D),
            weak_warning: c(0x9AA49E),
            ok: c(0xB3D6AD),
            modified: c(0x9FBACA),
            git_modified: c(0x7FB6E6),
            git_added: c(0x92DA88),
            git_untracked: c(0xE5A574),
            plain: c(0xD6DDD8),
            comment: c(0x84938A),
            string: c(0xB3CEAA),
            number: c(0xC9B396),
            keyword_form: c(0xB7A7C9),
            builtin: c(0xA8C8BC),
            definition: c(0xA8C4D2),
            keyword_arg: c(0xC1B3CF),
            constant: c(0xD2B69A),
            special_var: c(0xC1B3CF),
            prefix: c(0xD2B69A),
            type_name: c(0xABC6C1),
            preproc: c(0xC8BF9B),
            heading: c(0xB3D6AD),
            strong: c(0xEFF3F0),
            emphasis: c(0xB9CAD2),
            code: c(0xB0CCC2),
            link: c(0xA8C4D2),
            quote: c(0x9AA49E),
            todo: c(0xA8C4D2),
            alert: c(0xD6B58F),
            rainbow: [c(0xD6BF8D), c(0xB7A7C9), c(0xA8C4D2), c(0xB3CEAA), c(0xD2AA95), c(0xABC6C1)],
        }
    }

    pub fn light() -> Palette {
        Palette {
            dark: false,
            window_bg: c(0xEDF0EC),
            panel_bg: Color32::from_rgba_unmultiplied(244, 246, 242, 244),
            editor_bg: c(0xF8FAF6),
            gutter_bg: c(0xF8FAF6),
            border: Color32::from_rgba_unmultiplied(42, 57, 46, 30),
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
            line_number: c(0x8A978D),
            line_number_active: c(0x526658),
            current_line: Color32::from_rgba_unmultiplied(75, 105, 79, 10),
            selection: Color32::from_rgba_unmultiplied(113, 143, 125, 55),
            match_bracket: Color32::from_rgba_unmultiplied(75, 105, 79, 44),
            find_match: c(0xE2E8CE),
            find_current: c(0xC8D5AC),
            indent_guide: Color32::from_rgba_unmultiplied(42, 57, 46, 20),
            whitespace: Color32::from_rgba_unmultiplied(42, 57, 46, 55),
            error: c(0xAC4650),
            warning: c(0x876621),
            weak_warning: c(0x6F7B72),
            ok: c(0x46674B),
            modified: c(0x486779),
            git_modified: c(0x2B6592),
            git_added: c(0x2F7A3A),
            git_untracked: c(0x9B5224),
            plain: c(0x2D3931),
            comment: c(0x6C7B6E),
            string: c(0x466C40),
            number: c(0x8A643E),
            keyword_form: c(0x795789),
            builtin: c(0x3E6D5E),
            definition: c(0x3F657B),
            keyword_arg: c(0x7A608C),
            constant: c(0x925B38),
            special_var: c(0x7A608C),
            prefix: c(0x925B38),
            type_name: c(0x3C706B),
            preproc: c(0x7B6D3D),
            heading: c(0x46674B),
            strong: c(0x1E2A23),
            emphasis: c(0x4E6778),
            code: c(0x3E6D5E),
            link: c(0x3F657B),
            quote: c(0x637168),
            todo: c(0x3F657B),
            alert: c(0x94612F),
            rainbow: [c(0x876621), c(0x795789), c(0x3F657B), c(0x466C40), c(0x925B38), c(0x3C706B)],
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
