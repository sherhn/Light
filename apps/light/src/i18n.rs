//! UI localisation. English strings are the keys; the Russian table lives in `i18n_ru.rs`.
//!
//! `tr("Text")` returns the translation of a literal for the current language (or the literal itself),
//! `fmt("Opened {0} in {1}", &[&a, &b])` translates a template first and then fills the `{0}`, `{1}` (or plain `{}`)
//! placeholders, so word order can differ between languages.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use ide_core::settings::Language;

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set(l: Language) {
    LANG.store(if l == Language::Russian { 1 } else { 0 }, Ordering::Relaxed);
}

pub fn get() -> Language {
    if LANG.load(Ordering::Relaxed) == 1 { Language::Russian } else { Language::English }
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static T: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    T.get_or_init(|| crate::i18n_ru::RU.iter().copied().collect())
}

/// The translation of `s` in language `l`, if there is one.
fn lookup(l: Language, s: &str) -> Option<&'static str> {
    if l == Language::Russian { table().get(s).copied() } else { None }
}

/// Translate a literal. Unknown strings (and everything in English mode) come back unchanged.
pub fn tr(s: &'static str) -> &'static str {
    lookup(get(), s).unwrap_or(s)
}

/// Like [`tr`] for strings that are only known at run time (e.g. the name of a running Git operation).
pub fn tr_dyn(s: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Borrowed(lookup(get(), s).unwrap_or(s))
}

/// Translate a template and substitute `{}` / `{0}`-style placeholders with `args`.
pub fn fmt(template: &'static str, args: &[&dyn Display]) -> String {
    fill(tr(template), args)
}

fn fill(t: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(t.len() + 16);
    let mut next = 0usize;
    let mut rest = t;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let name = &after[..close];
        let idx = if name.is_empty() {
            next += 1;
            Some(next - 1)
        } else {
            name.parse::<usize>().ok()
        };
        match idx.and_then(|i| args.get(i)) {
            Some(a) => out.push_str(&a.to_string()),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Names of the languages offered in Settings (always written in their own language).
pub const LANGUAGES: [(Language, &str); 2] = [(Language::English, "English"), (Language::Russian, "Русский")];

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> Vec<String> {
        let mut v = vec![];
        let mut rest = s;
        while let Some(o) = rest.find('{') {
            if let Some(c) = rest[o..].find('}') {
                v.push(rest[o..o + c + 1].to_string());
                rest = &rest[o + c + 1..];
            } else {
                break;
            }
        }
        v.sort();
        v
    }

    #[test]
    fn fills_sequential_and_indexed_placeholders() {
        assert_eq!(fill("a {} b {}", &[&1, &"x"]), "a 1 b x");
        assert_eq!(fill("{1} before {0}", &[&"a", &"b"]), "b before a");
        assert_eq!(fill("no args {7}", &[]), "no args {7}");
        assert_eq!(fill("unclosed {", &[&1]), "unclosed {");
    }

    #[test]
    fn russian_table_has_no_duplicates_and_matching_placeholders() {
        let mut seen = std::collections::HashSet::new();
        for (en, ru) in crate::i18n_ru::RU {
            assert!(seen.insert(*en), "duplicate key {en:?}");
            assert!(!ru.is_empty(), "empty translation for {en:?}");
            assert_eq!(placeholders(en), placeholders(ru), "placeholders differ: {en:?} / {ru:?}");
        }
    }

    #[test]
    fn every_command_and_category_is_translated() {
        let t = table();
        for c in ide_core::commands::COMMANDS {
            assert!(t.contains_key(c.title), "command title {:?} ({}) has no Russian translation", c.title, c.id);
        }
        for c in [ide_core::commands::Category::File, ide_core::commands::Category::Edit, ide_core::commands::Category::Help] {
            assert!(t.contains_key(c.title()), "category {:?}", c.title());
        }
    }

    #[test]
    fn lookup_translates_only_in_russian() {
        assert_ne!(lookup(Language::Russian, "Cancel"), None);
        assert_eq!(lookup(Language::English, "Cancel"), None);
        assert_eq!(lookup(Language::Russian, "not in the table at all"), None);
    }
}
