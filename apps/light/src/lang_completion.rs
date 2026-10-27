//! Completion in the languages modules bring: the words of the language (keywords, built-ins, types - what its grammar
//! names), the names defined in the project, and the identifiers of the file being edited. No server, no model: the
//! vocabulary is data and the rest is read from the project.

use std::collections::HashSet;

use lisp_syntax::highlight::Class;

use crate::app::App;
use crate::state::{CItem, Completion};

/// The most items the popup holds.
const MAX_ITEMS: usize = 60;
/// A file bigger than this is only read around the caret for its identifiers.
const BIG_FILE: usize = 300_000;

/// The badge of an item: `f` function, `m` keyword or macro, `v` variable or constant, `C` type, `s` anything else.
fn kind_of_outline(kind: &str) -> &'static str {
    match kind {
        "function" | "method" => "f",
        "macro" => "m",
        "variable" | "constant" => "v",
        "class" | "struct" | "type" | "condition" => "C",
        _ => "s",
    }
}

fn kind_of_class(class: Class) -> &'static str {
    match class {
        Class::SpecialForm | Class::Keyword | Class::Preproc => "m",
        Class::Builtin => "f",
        Class::Type => "C",
        Class::Constant | Class::SpecialVar | Class::ConstName => "v",
        _ => "s",
    }
}

/// The identifiers of `text` that start with `low` (lower case), in the order they come, except the one ending at `caret`.
fn identifiers(text: &str, low: &str, caret: usize, lang: &light_lang::Language) -> Vec<String> {
    let (lo, hi) = if text.len() > BIG_FILE {
        let a = caret.saturating_sub(100_000);
        let b = (caret + 100_000).min(text.len());
        ((0..=a).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0), (b..=text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len()))
    } else {
        (0, text.len())
    };
    let mut out = vec![];
    let mut start: Option<usize> = None;
    for (i, c) in text[lo..hi].char_indices().map(|(i, c)| (i + lo, c)).chain(std::iter::once((hi, ' '))) {
        match (start, lang.is_identifier_char(c)) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                start = None;
                let w = &text[s..i];
                if i != caret && w.chars().next().is_some_and(|f| !f.is_ascii_digit()) && w.to_lowercase().starts_with(low) {
                    out.push(w.to_string());
                }
            }
            _ => {}
        }
    }
    out
}

impl App {
    /// Offer completions for the word before the caret in a document of a language of a module.
    pub fn refresh_language_completion(&mut self, explicit: bool) {
        let Some(d) = self.docs.get(self.active) else { return };
        let Some(lang) = d.ed.language().cloned() else {
            self.completion.active = false;
            return;
        };
        // plain text and documents have nothing to complete from
        if lang.is_plain() || crate::preview::markup_of(&lang).is_some() {
            self.completion.active = false;
            return;
        }
        let caret = d.ed.caret();
        let (s, _) = d.ed.identifier_range_at(caret);
        let prefix = d.ed.text()[s..caret].to_string();
        if (prefix.is_empty() && !explicit) || (!explicit && prefix.chars().count() < 2) || d.ed.has_selection() {
            self.completion.active = false;
            return;
        }
        // not in a comment or a string, nor in a number
        let dead = caret > 0
            && d.ed.language_spans().is_some_and(|spans| {
                let at = caret - 1;
                let i = spans.partition_point(|s| s.end <= at);
                spans.get(i).is_some_and(|s| s.start <= at && matches!(s.class, Class::Comment | Class::String | Class::Char | Class::Number))
            });
        if !explicit && dead {
            self.completion.active = false;
            return;
        }
        if prefix.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            self.completion.active = false;
            return;
        }
        let low = prefix.to_lowercase();
        let text = d.ed.text().to_string();
        let lang_id = lang.id().to_string();
        let mut seen: HashSet<String> = HashSet::new();
        let mut items: Vec<CItem> = Vec::new();
        let mut push = |name: String, kind: &str, source: u8, items: &mut Vec<CItem>| {
            let l = name.to_lowercase();
            if l.starts_with(&low) && l != low && seen.insert(l) {
                items.push(CItem { text: name, kind: kind.to_string(), source });
            }
        };
        // the language's own words come first, so that a keyword is offered as the keyword it is even when the file
        // already uses it as a word. Not after `.`, `->` or `::` (a member is not a keyword); and at the start of a
        // statement the keywords go before the names, because that is where they are used
        let before = &text[..s];
        let after_member = before.ends_with('.') || before.ends_with("->") || before.ends_with("::");
        let statement_start = before.rsplit('\n').next().unwrap_or("").trim().is_empty();
        if !after_member {
            for (w, class) in lang.words() {
                let kind = kind_of_class(*class);
                push(w.clone(), kind, if kind == "m" && statement_start { 0 } else { 3 }, &mut items);
            }
        }
        // what the project defines: the open files as they are now, the rest from the index
        let mut local: Vec<(String, String)> = vec![];
        for d in self.docs.iter_mut().filter(|d| d.diff.is_none() && d.ed.language().is_some_and(|l| l.id() == lang_id)) {
            for def in d.defs() {
                if def.name.to_lowercase().starts_with(&low) {
                    local.push((def.name.clone(), def.kind.label().to_string()));
                }
            }
        }
        for (name, kind) in local {
            push(name, kind_of_outline(&kind), 1, &mut items);
        }
        self.ensure_lang_index();
        // the project's names: in this language first, then (assembly labels for C...) the other languages'
        let all = self.hover.index.with_prefix(&prefix, 200, Some(&lang_id));
        for (name, kind, lang) in &all {
            if *lang == lang_id {
                push(name.clone(), kind_of_outline(kind), 1, &mut items);
            }
        }
        // the identifiers of this file
        for w in identifiers(&text, &low, caret, &lang) {
            push(w, "s", 2, &mut items);
        }
        for (name, kind, lang) in all {
            if lang != lang_id {
                push(name, kind_of_outline(&kind), 4, &mut items);
            }
        }
        // better sources first; among equals the ones that match the case typed, then the shorter
        items.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| (!a.text.starts_with(&prefix)).cmp(&!b.text.starts_with(&prefix))).then_with(|| a.text.len().cmp(&b.text.len())).then_with(|| a.text.cmp(&b.text)));
        items.truncate(MAX_ITEMS);
        let started = self.completion.active && self.completion.start == s && self.completion.prefix == prefix;
        let selected = if started { self.completion.selected } else { 0 };
        self.completion = Completion { active: !items.is_empty(), selected: selected.min(items.len().saturating_sub(1)), items, start: s, prefix };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_taken_from_the_text() {
        let lang = light_lang::Language::plain();
        let t = "int count = counter + 2; // count again\nvoid cnt(void);";
        assert_eq!(identifiers(t, "co", 0, &lang), vec!["count", "counter", "count"]);
        // the word being typed is not offered to itself
        assert_eq!(identifiers("cou cou", "co", 3, &lang), vec!["cou"]);
        assert_eq!(identifiers("x1 1x _y", "", 99, &lang), vec!["x1", "_y"]);
    }

    #[test]
    fn badges() {
        assert_eq!(kind_of_outline("method"), "f");
        assert_eq!(kind_of_outline("struct"), "C");
        assert_eq!(kind_of_class(Class::SpecialForm), "m");
        assert_eq!(kind_of_class(Class::Builtin), "f");
    }
}
