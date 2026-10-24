//! Markdown inlines: code spans, emphasis (the CommonMark delimiter algorithm, with `~~strike~~`), links and images
//! (inline, reference and autolinks), the HTML tags READMEs use, entities and line breaks.

use std::collections::HashMap;

use crate::html::{self, decode_entities};
use crate::model::{Inline, normalize, plain_text};

/// Link reference definitions: normalized label -> (url, title).
#[derive(Default, Debug)]
pub struct Refs(pub HashMap<String, (String, String)>);

pub fn normalize_label(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

enum Node {
    Inl(Inline),
    Delim { ch: char, n: usize, orig: usize, open: bool, close: bool },
    /// `[` or `![` waiting for its `]`; `start` is the char index after it.
    Bracket { image: bool, active: bool, start: usize },
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace() && !c.is_control() && !c.is_ascii())
}

pub fn parse_inlines(src: &str, refs: &Refs) -> Vec<Inline> {
    let chars: Vec<char> = src.chars().collect();
    normalize(scan(&chars, refs))
}

fn literal(n: Node) -> Option<Inline> {
    match n {
        Node::Inl(i) => Some(i),
        Node::Delim { ch, n, .. } => (n > 0).then(|| Inline::Text(ch.to_string().repeat(n))),
        Node::Bracket { image, .. } => Some(Inline::Text(if image { "![" } else { "[" }.into())),
    }
}

fn nodes_to_inlines(nodes: Vec<Node>) -> Vec<Inline> {
    nodes.into_iter().filter_map(literal).collect()
}

fn emphasis(mut v: Vec<Node>) -> Vec<Inline> {
    let mut j = 0;
    while j < v.len() {
        let (ch, d_n, d_orig, d_open) = match &v[j] {
            Node::Delim { ch, n, orig, open, close: true } if *n > 0 => (*ch, *n, *orig, *open),
            _ => {
                j += 1;
                continue;
            }
        };
        let mut found = None;
        let mut k = j;
        while k > 0 {
            k -= 1;
            if let Node::Delim { ch: oc, n: on, orig: oo, open: true, close: oclose } = &v[k]
                && *oc == ch
                && *on > 0
            {
                // "rule of 3": a delimiter that can both open and close does not pair with one whose lengths add up to a multiple of 3
                let odd = (*oclose || d_open) && (oo + d_orig) % 3 == 0 && !(oo % 3 == 0 && d_orig % 3 == 0);
                if ch == '~' || !odd {
                    found = Some(k);
                    break;
                }
            }
        }
        let Some(k) = found else {
            j += 1;
            continue;
        };
        let on = match &v[k] {
            Node::Delim { n, .. } => *n,
            _ => unreachable!("an opener is a delimiter"),
        };
        let use_n = if ch == '~' { on.min(d_n).min(2) } else if on >= 2 && d_n >= 2 { 2 } else { 1 };
        let inner = nodes_to_inlines(v.drain(k + 1..j).collect());
        let wrapped = match (ch, use_n) {
            ('~', _) => Inline::Strike(inner),
            (_, 2) => Inline::Strong(inner),
            _ => Inline::Emph(inner),
        };
        // the closer now sits at k + 1
        if let Node::Delim { n, .. } = &mut v[k] {
            *n -= use_n;
        }
        if let Node::Delim { n, .. } = &mut v[k + 1] {
            *n -= use_n;
        }
        v.insert(k + 1, Node::Inl(wrapped));
        let closer = k + 2;
        if matches!(&v[closer], Node::Delim { n: 0, .. }) {
            v.remove(closer);
        }
        let opener_left = !matches!(&v[k], Node::Delim { n: 0, .. });
        if !opener_left {
            v.remove(k);
        }
        j = if opener_left { k + 2 } else { k + 1 };
    }
    nodes_to_inlines(v)
}

fn scan(chars: &[char], refs: &Refs) -> Vec<Inline> {
    let n = chars.len();
    let mut nodes: Vec<Node> = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    macro_rules! flush {
        () => {
            if !text.is_empty() {
                nodes.push(Node::Inl(Inline::Text(std::mem::take(&mut text))));
            }
        };
    }
    while i < n {
        let c = chars[i];
        match c {
            '\\' => {
                if i + 1 < n && chars[i + 1].is_ascii_punctuation() {
                    text.push(chars[i + 1]);
                    i += 2;
                } else if i + 1 < n && chars[i + 1] == '\n' {
                    flush!();
                    nodes.push(Node::Inl(Inline::HardBreak));
                    i += 2;
                } else {
                    text.push('\\');
                    i += 1;
                }
            }
            '`' => {
                let mut run = 0;
                while i + run < n && chars[i + run] == '`' {
                    run += 1;
                }
                // the closing run has exactly as many backticks
                let mut j = i + run;
                let mut close = None;
                while j < n {
                    if chars[j] == '`' {
                        let mut r = 0;
                        while j + r < n && chars[j + r] == '`' {
                            r += 1;
                        }
                        if r == run {
                            close = Some(j);
                            break;
                        }
                        j += r;
                    } else {
                        j += 1;
                    }
                }
                match close {
                    Some(j) => {
                        flush!();
                        let mut code: String = chars[i + run..j].iter().map(|c| if *c == '\n' { ' ' } else { *c }).collect();
                        if code.len() > 2 && code.starts_with(' ') && code.ends_with(' ') && !code.trim().is_empty() {
                            code = code[1..code.len() - 1].to_string();
                        }
                        nodes.push(Node::Inl(Inline::Code(code)));
                        i = j + run;
                    }
                    None => {
                        text.push_str(&"`".repeat(run));
                        i += run;
                    }
                }
            }
            '*' | '_' | '~' => {
                let mut run = 0;
                while i + run < n && chars[i + run] == c {
                    run += 1;
                }
                if c == '~' && run > 2 {
                    text.push_str(&"~".repeat(run));
                    i += run;
                    continue;
                }
                let prev = if i == 0 { ' ' } else { chars[i - 1] };
                let next = if i + run >= n { ' ' } else { chars[i + run] };
                let (pw, nw) = (prev.is_whitespace(), next.is_whitespace());
                let (pp, np) = (is_punct(prev), is_punct(next));
                let left = !nw && (!np || pw || pp);
                let right = !pw && (!pp || nw || np);
                let (open, close) = if c == '_' { (left && (!right || pp), right && (!left || np)) } else { (left, right) };
                flush!();
                nodes.push(Node::Delim { ch: c, n: run, orig: run, open, close });
                i += run;
            }
            '[' => {
                flush!();
                nodes.push(Node::Bracket { image: false, active: true, start: i + 1 });
                i += 1;
            }
            '!' if chars.get(i + 1) == Some(&'[') => {
                flush!();
                nodes.push(Node::Bracket { image: true, active: true, start: i + 2 });
                i += 2;
            }
            ']' => {
                flush!();
                let opener = nodes.iter().rposition(|n| matches!(n, Node::Bracket { .. }));
                let Some(oi) = opener else {
                    text.push(']');
                    i += 1;
                    continue;
                };
                let (image, active, start) = match &nodes[oi] {
                    Node::Bracket { image, active, start } => (*image, *active, *start),
                    _ => unreachable!("rposition found a bracket"),
                };
                let tail = if active { link_tail(chars, i + 1, refs, &chars[start..i]) } else { None };
                match tail {
                    Some((next, url, title)) => {
                        let inner: Vec<Node> = nodes.drain(oi + 1..).collect();
                        nodes.pop();
                        let children = emphasis(inner);
                        let node = if image { Inline::Image { src: url, alt: plain_text(&children), title } } else { Inline::Link { url, title, children } };
                        nodes.push(Node::Inl(node));
                        if !image {
                            // links do not nest
                            for n in nodes.iter_mut() {
                                if let Node::Bracket { image: false, active, .. } = n {
                                    *active = false;
                                }
                            }
                        }
                        i = next;
                    }
                    None => {
                        nodes[oi] = Node::Inl(Inline::Text(if image { "![" } else { "[" }.into()));
                        text.push(']');
                        i += 1;
                    }
                }
            }
            '<' => {
                if let Some((len, node)) = angle(&chars[i..], refs) {
                    flush!();
                    if let Some(n) = node {
                        nodes.push(Node::Inl(n));
                    }
                    i += len;
                } else {
                    text.push('<');
                    i += 1;
                }
            }
            '&' => {
                let end = chars[i..].iter().take(12).position(|c| *c == ';');
                let decoded = end.and_then(|e| html::entity(&chars[i + 1..i + e].iter().collect::<String>()).map(|c| (e, c)));
                match decoded {
                    Some((e, ch)) => {
                        text.push(ch);
                        i += e + 1;
                    }
                    None => {
                        text.push('&');
                        i += 1;
                    }
                }
            }
            '\n' => {
                let hard = text.ends_with("  ");
                let trimmed = text.trim_end_matches(' ').len();
                text.truncate(trimmed);
                flush!();
                nodes.push(Node::Inl(if hard { Inline::HardBreak } else { Inline::SoftBreak }));
                i += 1;
            }
            'h' if (starts(chars, i, "http://") || starts(chars, i, "https://")) && (i == 0 || !chars[i - 1].is_alphanumeric()) && !nodes.iter().any(|n| matches!(n, Node::Bracket { active: true, .. })) => {
                let mut j = i;
                while j < n && !chars[j].is_whitespace() && chars[j] != '<' {
                    j += 1;
                }
                // trailing punctuation belongs to the sentence, and so does an unmatched ")"
                loop {
                    let last = chars[j - 1];
                    let opens = chars[i..j].iter().filter(|c| **c == '(').count();
                    let closes = chars[i..j].iter().filter(|c| **c == ')').count();
                    if matches!(last, '.' | ',' | ':' | ';' | '!' | '?' | '"' | '\'' | '*' | '_' | '~') || (last == ')' && closes > opens) {
                        j -= 1;
                    } else {
                        break;
                    }
                }
                if j - i > 8 {
                    flush!();
                    let url: String = chars[i..j].iter().collect();
                    nodes.push(Node::Inl(Inline::Link { url: url.clone(), title: String::new(), children: vec![Inline::Text(url)] }));
                    i = j;
                } else {
                    text.push(c);
                    i += 1;
                }
            }
            _ => {
                text.push(c);
                i += 1;
            }
        }
    }
    flush!();
    emphasis(nodes)
}

fn starts(chars: &[char], i: usize, s: &str) -> bool {
    s.chars().enumerate().all(|(k, c)| chars.get(i + k) == Some(&c))
}

/// What follows a `]`: `(url "title")`, `[label]`, `[]` or nothing (a shortcut reference). Returns the index to go on at.
fn link_tail(chars: &[char], j: usize, refs: &Refs, label_raw: &[char]) -> Option<(usize, String, String)> {
    let n = chars.len();
    let lookup = |label: &[char]| refs.0.get(&normalize_label(&label.iter().collect::<String>())).cloned();
    match chars.get(j) {
        Some('(') => {
            let mut i = j + 1;
            let skip_ws = |i: &mut usize| {
                while *i < n && chars[*i].is_whitespace() {
                    *i += 1;
                }
            };
            skip_ws(&mut i);
            let mut url = String::new();
            if chars.get(i) == Some(&'<') {
                i += 1;
                while i < n && chars[i] != '>' && chars[i] != '\n' {
                    url.push(chars[i]);
                    i += 1;
                }
                if chars.get(i) != Some(&'>') {
                    return None;
                }
                i += 1;
            } else {
                let mut depth = 0;
                while i < n && !chars[i].is_whitespace() {
                    match chars[i] {
                        '\\' if i + 1 < n && chars[i + 1].is_ascii_punctuation() => {
                            url.push(chars[i + 1]);
                            i += 2;
                            continue;
                        }
                        '(' => depth += 1,
                        ')' => {
                            if depth == 0 {
                                break;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                    url.push(chars[i]);
                    i += 1;
                }
            }
            skip_ws(&mut i);
            let mut title = String::new();
            if let Some(&q) = chars.get(i).filter(|c| matches!(c, '"' | '\'' | '(')) {
                let close = if q == '(' { ')' } else { q };
                i += 1;
                while i < n && chars[i] != close {
                    if chars[i] == '\\' && i + 1 < n {
                        i += 1;
                    }
                    title.push(chars[i]);
                    i += 1;
                }
                if i >= n {
                    return None;
                }
                i += 1;
                skip_ws(&mut i);
            }
            (chars.get(i) == Some(&')')).then(|| (i + 1, decode_entities(&url), title))
        }
        Some('[') => {
            let mut i = j + 1;
            let s = i;
            while i < n && chars[i] != ']' && chars[i] != '[' {
                i += 1;
            }
            if chars.get(i) != Some(&']') {
                return lookup(label_raw).map(|(u, t)| (j, u, t));
            }
            let label = if i == s { label_raw } else { &chars[s..i] };
            match lookup(label) {
                Some((u, t)) => Some((i + 1, u, t)),
                None => lookup(label_raw).map(|(u, t)| (j, u, t)),
            }
        }
        _ => lookup(label_raw).map(|(u, t)| (j, u, t)),
    }
}

/// Something that starts with `<`: an autolink, an HTML tag, a comment. Returns its length and what it becomes.
fn angle(chars: &[char], refs: &Refs) -> Option<(usize, Option<Inline>)> {
    // <https://...> and <me@example.com>
    if let Some(close) = chars.iter().position(|c| *c == '>' || c.is_whitespace()).filter(|p| chars[*p] == '>') {
        let inner: String = chars[1..close].iter().collect();
        let is_uri = inner.split_once(':').is_some_and(|(scheme, rest)| scheme.len() >= 2 && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) && !rest.is_empty());
        if is_uri {
            return Some((close + 1, Some(Inline::Link { url: inner.clone(), title: String::new(), children: vec![Inline::Text(inner)] })));
        }
        if inner.contains('@') && !inner.starts_with('@') && !inner.ends_with('@') && !inner.contains('<') {
            return Some((close + 1, Some(Inline::Link { url: format!("mailto:{inner}"), title: String::new(), children: vec![Inline::Text(inner)] })));
        }
    }
    let tag = html::parse_tag(chars)?;
    if tag.name.is_empty() {
        return Some((tag.len, None));
    }
    if tag.closing {
        return Some((tag.len, None));
    }
    let after = &chars[tag.len..];
    match tag.name.as_str() {
        "br" => Some((tag.len, Some(Inline::HardBreak))),
        "img" => {
            let src = tag.attr("src").unwrap_or("").to_string();
            (!src.is_empty()).then(|| (tag.len, Some(Inline::Image { src, alt: tag.attr("alt").unwrap_or("").to_string(), title: tag.attr("title").unwrap_or("").to_string() })))
        }
        name @ ("a" | "b" | "strong" | "i" | "em" | "code" | "kbd" | "del" | "s" | "strike" | "u" | "mark" | "small" | "sub" | "sup" | "span" | "font" | "big" | "tt" | "samp" | "var") => {
            // the matching closing tag, if there is one on this line of text
            let close: Vec<char> = format!("</{name}").chars().collect();
            let end = (0..after.len()).find(|&p| after[p..].len() >= close.len() && after[p..p + close.len()].iter().zip(&close).all(|(a, b)| a.to_ascii_lowercase() == *b));
            let Some(end) = end else { return Some((tag.len, None)) };
            let gt = after[end..].iter().position(|c| *c == '>')?;
            let inner = &after[..end];
            let total = tag.len + end + gt + 1;
            let node = match name {
                "a" => {
                    let kids = normalize(scan(inner, refs));
                    match tag.attr("href") {
                        Some(h) if !h.is_empty() => Inline::Link { url: h.to_string(), title: tag.attr("title").unwrap_or("").to_string(), children: kids },
                        _ => return Some((tag.len, None)),
                    }
                }
                "b" | "strong" => Inline::Strong(normalize(scan(inner, refs))),
                "i" | "em" | "var" => Inline::Emph(normalize(scan(inner, refs))),
                "del" | "s" | "strike" => Inline::Strike(normalize(scan(inner, refs))),
                "code" | "kbd" | "tt" | "samp" => Inline::Code(inner.iter().collect()),
                _ => return Some((tag.len, None)),
            };
            Some((total, Some(node)))
        }
        _ => Some((tag.len, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<Inline> {
        parse_inlines(s, &Refs::default())
    }

    fn t(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    #[test]
    fn plain_and_escapes() {
        assert_eq!(p("a \\*b\\* c"), vec![t("a *b* c")]);
        assert_eq!(p("1 &lt; 2 &amp; 3"), vec![t("1 < 2 & 3")]);
    }

    #[test]
    fn emphasis_and_strong() {
        assert_eq!(p("*a* **b** ***c***"), vec![Inline::Emph(vec![t("a")]), t(" "), Inline::Strong(vec![t("b")]), t(" "), Inline::Emph(vec![Inline::Strong(vec![t("c")])])]);
        assert_eq!(p("snake_case_name"), vec![t("snake_case_name")]);
        assert_eq!(p("_a_ and __b__"), vec![Inline::Emph(vec![t("a")]), t(" and "), Inline::Strong(vec![t("b")])]);
        assert_eq!(p("2 * 3 * 4"), vec![t("2 * 3 * 4")]);
        assert_eq!(p("**a *b* c**"), vec![Inline::Strong(vec![t("a "), Inline::Emph(vec![t("b")]), t(" c")])]);
        assert_eq!(p("~~gone~~"), vec![Inline::Strike(vec![t("gone")])]);
    }

    #[test]
    fn code_spans() {
        assert_eq!(p("use `a*b` here"), vec![t("use "), Inline::Code("a*b".into()), t(" here")]);
        assert_eq!(p("`` a`b ``"), vec![Inline::Code("a`b".into())]);
        assert_eq!(p("a ` b"), vec![t("a ` b")]);
    }

    #[test]
    fn links_and_images() {
        assert_eq!(p("[Light](https://x.org/a_b \"T\")"), vec![Inline::Link { url: "https://x.org/a_b".into(), title: "T".into(), children: vec![t("Light")] }]);
        assert_eq!(p("![logo](a/b.png)"), vec![Inline::Image { src: "a/b.png".into(), alt: "logo".into(), title: String::new() }]);
        assert_eq!(p("[![b](i.svg)](https://x)"), vec![Inline::Link { url: "https://x".into(), title: String::new(), children: vec![Inline::Image { src: "i.svg".into(), alt: "b".into(), title: String::new() }] }]);
        assert_eq!(p("[a] (b)"), vec![t("[a] (b)")]);
        assert_eq!(p("[a](b(c)d)"), vec![Inline::Link { url: "b(c)d".into(), title: String::new(), children: vec![t("a")] }]);
        assert_eq!(p("<https://x.org>"), vec![Inline::Link { url: "https://x.org".into(), title: String::new(), children: vec![t("https://x.org")] }]);
    }

    #[test]
    fn reference_links() {
        let mut refs = Refs::default();
        refs.0.insert("docs".into(), ("https://d".into(), String::new()));
        let v = parse_inlines("[the docs][Docs] and [docs] and [x][]", &refs);
        assert!(matches!(&v[0], Inline::Link { url, .. } if url == "https://d"));
        assert!(matches!(&v[2], Inline::Link { url, .. } if url == "https://d"));
        assert_eq!(v[3], t(" and [x][]"));
    }

    #[test]
    fn bare_urls() {
        assert_eq!(p("see https://x.org/a."), vec![t("see "), Inline::Link { url: "https://x.org/a".into(), title: String::new(), children: vec![t("https://x.org/a")] }, t(".")]);
        // not inside a link text
        let v = p("[https://x.org](https://y.org)");
        assert!(matches!(&v[0], Inline::Link { url, .. } if url == "https://y.org"));
    }

    #[test]
    fn breaks() {
        assert_eq!(p("a\nb"), vec![t("a"), Inline::SoftBreak, t("b")]);
        assert_eq!(p("a  \nb"), vec![t("a"), Inline::HardBreak, t("b")]);
        assert_eq!(p("a\\\nb"), vec![t("a"), Inline::HardBreak, t("b")]);
    }

    #[test]
    fn html_tags() {
        assert_eq!(p("a<br>b"), vec![t("a"), Inline::HardBreak, t("b")]);
        assert_eq!(p("<b>bold</b> and <code>x</code>"), vec![Inline::Strong(vec![t("bold")]), t(" and "), Inline::Code("x".into())]);
        assert_eq!(p("<a href=\"https://x\">go</a>"), vec![Inline::Link { url: "https://x".into(), title: String::new(), children: vec![t("go")] }]);
        assert_eq!(p("<img src=\"a.png\" alt=\"A\">"), vec![Inline::Image { src: "a.png".into(), alt: "A".into(), title: String::new() }]);
        assert_eq!(p("x <!-- hidden --> y <span class=\"k\">z</span>"), vec![t("x  y z")]);
    }
}
