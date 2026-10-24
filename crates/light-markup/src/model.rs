//! The document both Markdown and reStructuredText are parsed into: blocks (headings, paragraphs, lists, code, tables,
//! quotes, admonitions...) made of inline runs (emphasis, code, links, images). Every block remembers the source line
//! it starts at, so a preview can follow the caret and the scrolling of the editor.

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// 0-based source line the block starts at.
    pub line: usize,
    pub kind: BlockKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    /// A task list item: `Some(done)`.
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// A term and what it is defined as (definition lists, field lists).
#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub term: Vec<Inline>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    Heading { level: u8, inlines: Vec<Inline>, anchor: String },
    Paragraph(Vec<Inline>),
    /// `lang` is the info string's first word (empty: none).
    Code { lang: String, text: String },
    List { ordered: bool, start: u64, items: Vec<ListItem> },
    Quote(Vec<Block>),
    /// `note`, `warning`, `tip`, `important`, `caution`, `danger`...; `title` is empty for the kind's own name.
    Admonition { kind: String, title: String, blocks: Vec<Block> },
    Rule,
    Table { align: Vec<Align>, header: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
    Definitions(Vec<Definition>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Emph(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Link { url: String, title: String, children: Vec<Inline> },
    Image { src: String, alt: String, title: String },
    SoftBreak,
    HardBreak,
}

/// The text of inlines without any formatting (a heading's anchor, an image's alt text, a tooltip).
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut s = String::new();
    push_plain(inlines, &mut s);
    s
}

fn push_plain(inlines: &[Inline], out: &mut String) {
    for i in inlines {
        match i {
            Inline::Text(t) | Inline::Code(t) => out.push_str(t),
            Inline::Emph(c) | Inline::Strong(c) | Inline::Strike(c) => push_plain(c, out),
            Inline::Link { children, .. } => push_plain(children, out),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::SoftBreak => out.push(' '),
            Inline::HardBreak => out.push('\n'),
        }
    }
}

/// The text of a whole block, for searching or for a summary.
pub fn block_text(b: &Block) -> String {
    match &b.kind {
        BlockKind::Heading { inlines, .. } | BlockKind::Paragraph(inlines) => plain_text(inlines),
        BlockKind::Code { text, .. } => text.clone(),
        BlockKind::List { items, .. } => items.iter().flat_map(|i| i.blocks.iter().map(block_text)).collect::<Vec<_>>().join("\n"),
        BlockKind::Quote(b) | BlockKind::Admonition { blocks: b, .. } => b.iter().map(block_text).collect::<Vec<_>>().join("\n"),
        BlockKind::Rule => String::new(),
        BlockKind::Table { header, rows, .. } => header.iter().chain(rows.iter().flatten()).map(|c| plain_text(c)).collect::<Vec<_>>().join(" "),
        BlockKind::Definitions(d) => d.iter().map(|d| plain_text(&d.term)).collect::<Vec<_>>().join("\n"),
    }
}

/// GitHub's heading anchors: lower case, letters, digits, `-` and `_` kept, spaces become `-`, the rest dropped.
pub fn slug(text: &str) -> String {
    let mut s = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' {
            s.extend(c.to_lowercase());
        } else if c.is_whitespace() {
            s.push('-');
        }
    }
    s
}

/// Makes anchors unique the way GitHub does: the second `intro` is `intro-1`.
#[derive(Default)]
pub struct Anchors {
    seen: std::collections::HashMap<String, usize>,
}

impl Anchors {
    pub fn make(&mut self, text: &str) -> String {
        let base = slug(text);
        let n = self.seen.entry(base.clone()).or_insert(0);
        let out = if *n == 0 { base.clone() } else { format!("{base}-{n}") };
        *n += 1;
        out
    }
}

/// Every heading of the document, in order: (level, text, anchor, source line), nested blocks included.
pub fn headings(doc: &Document) -> Vec<(u8, String, String, usize)> {
    fn walk(blocks: &[Block], out: &mut Vec<(u8, String, String, usize)>) {
        for b in blocks {
            match &b.kind {
                BlockKind::Heading { level, inlines, anchor } => out.push((*level, plain_text(inlines), anchor.clone(), b.line)),
                BlockKind::Quote(c) | BlockKind::Admonition { blocks: c, .. } => walk(c, out),
                BlockKind::List { items, .. } => items.iter().for_each(|i| walk(&i.blocks, out)),
                BlockKind::Definitions(d) => d.iter().for_each(|d| walk(&d.blocks, out)),
                _ => {}
            }
        }
    }
    let mut out = vec![];
    walk(&doc.blocks, &mut out);
    out
}

/// Merge neighbouring text and drop empty text, so equal documents compare equal.
pub fn normalize(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for i in inlines {
        let i = match i {
            Inline::Emph(c) => Inline::Emph(normalize(c)),
            Inline::Strong(c) => Inline::Strong(normalize(c)),
            Inline::Strike(c) => Inline::Strike(normalize(c)),
            Inline::Link { url, title, children } => Inline::Link { url, title, children: normalize(children) },
            other => other,
        };
        match (out.last_mut(), i) {
            (_, Inline::Text(t)) if t.is_empty() => {}
            (Some(Inline::Text(prev)), Inline::Text(t)) => prev.push_str(&t),
            (_, other) => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_follow_github() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("  A  b "), "a--b");
        assert_eq!(slug("Работа с Linux (WSL)"), "работа-с-linux-wsl");
        let mut a = Anchors::default();
        assert_eq!(a.make("Intro"), "intro");
        assert_eq!(a.make("Intro"), "intro-1");
        assert_eq!(a.make("Intro"), "intro-2");
    }

    #[test]
    fn normalize_merges_text() {
        let v = normalize(vec![Inline::Text("a".into()), Inline::Text("".into()), Inline::Text("b".into()), Inline::Code("c".into()), Inline::Text("d".into())]);
        assert_eq!(v, vec![Inline::Text("ab".into()), Inline::Code("c".into()), Inline::Text("d".into())]);
    }

    #[test]
    fn plain_text_drops_formatting() {
        let v = vec![Inline::Text("a ".into()), Inline::Strong(vec![Inline::Text("b".into())]), Inline::SoftBreak, Inline::Link { url: "u".into(), title: String::new(), children: vec![Inline::Code("c".into())] }];
        assert_eq!(plain_text(&v), "a b c");
    }
}
