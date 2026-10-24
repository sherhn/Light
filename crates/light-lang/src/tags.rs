//! Markup structure from grammar spans: quoted attributes, comments and raw text never become tags.

use lisp_syntax::highlight::{Class, Span};

pub(crate) struct Tag {
    pub name: String,
    pub start: usize,
    pub end: usize,
    pub closing: bool,
    pub empty: bool,
}

pub(crate) fn scan(text: &str, spans: &[Span], void_tags: &[String]) -> Vec<Tag> {
    let mut tags = Vec::new();
    let mut pending: Option<(usize, String, bool)> = None;
    for (i, span) in spans.iter().enumerate() {
        if span.class != Class::Preproc {
            continue;
        }
        match &text[span.start..span.end] {
            "<" | "</" => {
                pending = spans.get(i + 1).filter(|name| name.start == span.end && name.class == Class::SpecialForm)
                    .map(|name| (span.start, text[name.start..name.end].to_ascii_lowercase(), span.end - span.start == 2));
            }
            ">" | "/>" => {
                if let Some((start, name, closing)) = pending.take() {
                    let empty = text[span.start..span.end].starts_with('/') || void_tags.iter().any(|v| v.eq_ignore_ascii_case(&name));
                    tags.push(Tag { name, start, end: span.end, closing, empty });
                }
            }
            _ => {}
        }
    }
    tags
}
