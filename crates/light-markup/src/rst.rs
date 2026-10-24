//! reStructuredText (the markup of Sphinx and Python documentation) parsed into a [`Document`]: sections by their
//! underlines, paragraphs, bullet / enumerated / definition / field lists, literal blocks, block quotes, line blocks,
//! grid / simple / list / csv tables and the directives people use (admonitions, `code-block`, `image`, `toctree`,
//! Sphinx's `py:function` family...). Like the Markdown parser it parses nested content by taking the indentation off
//! the lines and parsing what is left.

use crate::model::{Align, Anchors, Block, BlockKind, Definition, Document, Inline, ListItem, headings, plain_text, slug};
use crate::rst_inline::{Defs, normalize_name, parse_inlines};

#[derive(Clone, Copy)]
struct Line<'a> {
    text: &'a str,
    no: usize,
}

pub fn parse(text: &str) -> Document {
    let owned: Vec<String> = text.split('\n').map(|l| expand_tabs(l.trim_end())).collect();
    let lines: Vec<Line> = owned.iter().enumerate().map(|(no, t)| Line { text: t.as_str(), no }).collect();
    let mut defs = collect_defs(&lines);
    let mut p = Parser { defs: std::mem::take(&mut defs), styles: vec![], anchors: Anchors::default() };
    let doc = p.blocks(&lines);
    let doc = Document { blocks: doc };
    // references to section titles (`Introduction`_) resolve to the sections: parse again knowing them
    let hs = headings(&doc);
    let mut added = false;
    let mut defs = std::mem::take(&mut p.defs);
    for (_, title, anchor, _) in &hs {
        if defs.targets.entry(normalize_name(title)).or_insert_with(|| format!("#{anchor}")).starts_with('#') {
            added = true;
        }
    }
    if added && text.len() < 400_000 && text.contains("_") {
        let mut p = Parser { defs, styles: vec![], anchors: Anchors::default() };
        return Document { blocks: p.blocks(&lines) };
    }
    doc
}

fn expand_tabs(l: &str) -> String {
    if !l.contains('\t') {
        return l.to_string();
    }
    let mut out = String::new();
    let mut col = 0;
    for c in l.chars() {
        if c == '\t' {
            let n = 8 - col % 8;
            out.push_str(&" ".repeat(n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

fn indent(s: &str) -> usize {
    s.len() - s.trim_start_matches(' ').len()
}

fn blank(s: &str) -> bool {
    s.trim().is_empty()
}

/// The common character of a line made only of one punctuation character (an underline, an overline, a border).
fn punct_line(s: &str) -> Option<char> {
    let c = s.chars().next().filter(|c| c.is_ascii_punctuation())?;
    (s.chars().count() >= 2 && s.chars().all(|x| x == c)).then_some(c)
}

fn bullet(s: &str) -> Option<usize> {
    let c = s.chars().next()?;
    if !"-*+\u{2022}\u{2023}\u{2043}".contains(c) {
        return None;
    }
    let rest = &s[c.len_utf8()..];
    (rest.is_empty() || rest.starts_with(' ')).then(|| c.len_utf8() + indent(rest).max(1).min(rest.len().max(1)))
}

/// `1.` `1)` `(1)` `a.` `#.` `iv.`: (the number, width up to the text).
fn enum_marker(s: &str) -> Option<(u64, usize)> {
    let (open, body) = match s.strip_prefix('(') {
        Some(r) => (1, r),
        None => (0, s),
    };
    let end = body.find(['.', ')'])?;
    let label = &body[..end];
    if label.is_empty() || label.len() > 9 {
        return None;
    }
    if open == 1 && !body[end..].starts_with(')') {
        return None;
    }
    let ok = label == "#" || label.chars().all(|c| c.is_ascii_digit()) || (label.chars().count() == 1 && label.chars().all(|c| c.is_ascii_alphabetic())) || (label.len() <= 4 && label.chars().all(|c| "ivxlcdmIVXLCDM".contains(c)));
    if !ok {
        return None;
    }
    let after = &body[end + 1..];
    if !(after.is_empty() || after.starts_with(' ')) {
        return None;
    }
    let n = label.parse::<u64>().unwrap_or(1);
    Some((n, open + end + 1 + indent(after).max(1).min(after.len().max(1))))
}

/// `:name: text` -> (name, text).
fn field_marker(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix(':')?;
    // the name may hold escaped colons; the first colon followed by a space or the end closes it
    let mut end = None;
    let b = rest.as_bytes();
    for k in 0..b.len() {
        if b[k] == b':' && (k + 1 == b.len() || b[k + 1] == b' ') && !(k > 0 && b[k - 1] == b'\\') {
            end = Some(k);
            break;
        }
    }
    let k = end?;
    (k > 0 && !rest[..k].starts_with(' ')).then(|| (&rest[..k], rest[k + 1..].trim()))
}

fn grid_border(s: &str) -> bool {
    s.starts_with('+') && s.trim_end().ends_with('+') && s.chars().all(|c| matches!(c, '+' | '-' | '=' | ':'))
}

/// `=====  =====`: the ranges of the column groups, when there are at least two.
fn simple_border(s: &str) -> Option<Vec<(usize, usize)>> {
    if !s.starts_with('=') || !s.chars().all(|c| c == '=' || c == ' ') {
        return None;
    }
    let chars: Vec<char> = s.chars().collect();
    let mut cols = vec![];
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '=' {
            let st = i;
            while i < chars.len() && chars[i] == '=' {
                i += 1;
            }
            cols.push((st, i));
        } else {
            i += 1;
        }
    }
    (cols.len() >= 2).then_some(cols)
}

fn dedent_lines<'a>(lines: &[Line<'a>]) -> Vec<Line<'a>> {
    let min = lines.iter().filter(|l| !blank(l.text)).map(|l| indent(l.text)).min().unwrap_or(0);
    lines.iter().map(|l| Line { text: if blank(l.text) { "" } else { &l.text[min.min(l.text.len())..] }, no: l.no }).collect()
}

/// Named hyperlink targets and substitution definitions of the whole text.
fn collect_defs(lines: &[Line]) -> Defs {
    let mut d = Defs::default();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].text;
        if let Some(rest) = t.strip_prefix(".. _") {
            // `.. _name: url` or `.. _`odd: name`: url`
            let (name, after) = if let Some(r) = rest.strip_prefix('`') {
                match r.find("`:") {
                    Some(e) => (r[..e].to_string(), r[e + 2..].trim().to_string()),
                    None => (String::new(), String::new()),
                }
            } else {
                let mut e = None;
                for (k, c) in rest.char_indices() {
                    if c == ':' && !rest[..k].ends_with('\\') && (rest[k + 1..].is_empty() || rest[k + 1..].starts_with(' ')) {
                        e = Some(k);
                        break;
                    }
                }
                match e {
                    Some(e) => (rest[..e].replace("\\:", ":"), rest[e + 1..].trim().to_string()),
                    None => (String::new(), String::new()),
                }
            };
            if !name.is_empty() {
                let mut url = after;
                if url.is_empty() {
                    // the address may be on the next lines
                    let mut j = i + 1;
                    while j < lines.len() && indent(lines[j].text) > 0 && !blank(lines[j].text) {
                        url.push_str(lines[j].text.trim());
                        j += 1;
                    }
                }
                let url = if url.is_empty() { format!("#{}", slug(&name)) } else { url.replace(' ', "") };
                let url = if url.ends_with('_') && !url.contains("://") { format!("#{}", slug(url.trim_end_matches('_'))) } else { url };
                d.targets.entry(normalize_name(&name)).or_insert(url);
            }
        } else if let Some(rest) = t.strip_prefix(".. |")
            && let Some(e) = rest.find('|')
        {
            let name = &rest[..e];
            let def = rest[e + 1..].trim();
            if let Some(v) = def.strip_prefix("replace::") {
                d.subs.insert(normalize_name(name), v.trim().to_string());
            } else if let Some(v) = def.strip_prefix("image::") {
                d.subs.insert(normalize_name(name), format!("image:{}", v.trim()));
            } else if let Some(v) = def.strip_prefix("unicode::") {
                let s: String = v.split_whitespace().filter_map(|w| w.strip_prefix("0x").or_else(|| w.strip_prefix("U+")).and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32)).collect();
                d.subs.insert(normalize_name(name), s);
            }
        }
        i += 1;
    }
    d
}

struct Parser {
    defs: Defs,
    /// Section styles in the order they appeared: (character, has an overline).
    styles: Vec<(char, bool)>,
    anchors: Anchors,
}

impl Parser {
    fn inl(&self, s: &str) -> Vec<Inline> {
        parse_inlines(s, &self.defs)
    }

    fn section(&mut self, title: &str, ch: char, over: bool, line: usize) -> Block {
        let style = (ch, over);
        let level = match self.styles.iter().position(|s| *s == style) {
            Some(p) => p + 1,
            None => {
                self.styles.push(style);
                self.styles.len()
            }
        };
        let inlines = self.inl(title.trim());
        let anchor = self.anchors.make(&plain_text(&inlines));
        Block { line, kind: BlockKind::Heading { level: level.min(6) as u8, inlines, anchor } }
    }

    /// The lines that belong to a construct started at `lines[i]`: those after it that are indented, with blank lines
    /// between them when more indented text follows. Returns them and the index to go on at.
    fn indented_body<'a>(&self, lines: &[Line<'a>], i: usize) -> (Vec<Line<'a>>, usize) {
        let mut j = i + 1;
        let mut body = vec![];
        while j < lines.len() {
            let t = lines[j];
            if blank(t.text) {
                let mut k = j;
                while k < lines.len() && blank(lines[k].text) {
                    k += 1;
                }
                if k < lines.len() && indent(lines[k].text) > 0 {
                    for b in &lines[j..k] {
                        body.push(Line { text: "", no: b.no });
                    }
                    j = k;
                    continue;
                }
                break;
            }
            if indent(t.text) == 0 {
                break;
            }
            body.push(t);
            j += 1;
        }
        (body, j)
    }

    fn blocks(&mut self, lines: &[Line]) -> Vec<Block> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let l = lines[i];
            if blank(l.text) {
                i += 1;
                continue;
            }
            if indent(l.text) > 0 {
                i = self.quote(lines, i, &mut out);
                continue;
            }
            let s = l.text;
            if s.starts_with("..") && (s.len() == 2 || s[2..].starts_with(' ')) {
                i = self.explicit(lines, i, &mut out);
                continue;
            }
            // a punctuation line: the overline of a section, or a transition
            if let Some(c) = punct_line(s) {
                if i + 2 < lines.len() && !blank(lines[i + 1].text) && indent(lines[i + 1].text) == 0 && punct_line(lines[i + 1].text).is_none() && punct_line(lines[i + 2].text) == Some(c) {
                    out.push(self.section(lines[i + 1].text, c, true, l.no));
                    i += 3;
                    continue;
                }
                if s.chars().count() >= 4 && (i + 1 >= lines.len() || blank(lines[i + 1].text)) {
                    out.push(Block { line: l.no, kind: BlockKind::Rule });
                    i += 1;
                    continue;
                }
            }
            if let Some(w) = bullet(s) {
                i = self.bullets(lines, i, w, &mut out);
                continue;
            }
            if let Some((start, w)) = enum_marker(s) {
                i = self.enumerated(lines, i, start, w, &mut out);
                continue;
            }
            if field_marker(s).is_some() {
                i = self.fields(lines, i, &mut out);
                continue;
            }
            if s.starts_with("| ") || s == "|" {
                i = self.line_block(lines, i, &mut out);
                continue;
            }
            if s.starts_with(">>> ") {
                let mut j = i;
                let mut code = vec![];
                while j < lines.len() && !blank(lines[j].text) {
                    code.push(lines[j].text);
                    j += 1;
                }
                out.push(Block { line: l.no, kind: BlockKind::Code { lang: "python".into(), text: code.join("\n") } });
                i = j;
                continue;
            }
            if grid_border(s) {
                let (b, j) = self.grid_table(lines, i);
                if let Some(b) = b {
                    out.push(b);
                    i = j;
                    continue;
                }
            }
            if let Some(cols) = simple_border(s) {
                let (b, j) = self.simple_table(lines, i, &cols);
                if let Some(b) = b {
                    out.push(b);
                    i = j;
                    continue;
                }
            }
            // a section title: text and its underline
            if i + 1 < lines.len()
                && punct_line(s).is_none()
                && let Some(c) = punct_line(lines[i + 1].text)
                && indent(lines[i + 1].text) == 0
                && (lines[i + 1].text.chars().count() >= s.trim().chars().count() || lines[i + 1].text.chars().count() >= 4)
            {
                out.push(self.section(s, c, false, l.no));
                i += 2;
                continue;
            }
            // a term and its definition
            if i + 1 < lines.len() && indent(lines[i + 1].text) > 0 && !blank(lines[i + 1].text) {
                i = self.definitions(lines, i, &mut out);
                continue;
            }
            i = self.paragraph(lines, i, &mut out);
        }
        out
    }

    fn quote(&mut self, lines: &[Line], i: usize, out: &mut Vec<Block>) -> usize {
        let mut j = i;
        let mut body = vec![];
        while j < lines.len() {
            let t = lines[j];
            if blank(t.text) {
                let mut k = j;
                while k < lines.len() && blank(lines[k].text) {
                    k += 1;
                }
                if k < lines.len() && indent(lines[k].text) > 0 {
                    for b in &lines[j..k] {
                        body.push(Line { text: "", no: b.no });
                    }
                    j = k;
                    continue;
                }
                break;
            }
            if indent(t.text) == 0 {
                break;
            }
            body.push(t);
            j += 1;
        }
        let inner = dedent_lines(&body);
        let blocks = self.blocks(&inner);
        if !blocks.is_empty() {
            out.push(Block { line: lines[i].no, kind: BlockKind::Quote(blocks) });
        }
        j
    }

    fn paragraph(&mut self, lines: &[Line], i: usize, out: &mut Vec<Block>) -> usize {
        let mut j = i;
        let mut text = String::new();
        while j < lines.len() && !blank(lines[j].text) {
            if j > i {
                text.push('\n');
            }
            text.push_str(lines[j].text.trim());
            j += 1;
        }
        let literal = text.ends_with("::");
        let shown = if text == "::" {
            String::new()
        } else if let Some(t) = text.strip_suffix(" ::") {
            t.to_string()
        } else if literal {
            format!("{}:", &text[..text.len() - 2])
        } else {
            text
        };
        if !shown.is_empty() {
            out.push(Block { line: lines[i].no, kind: BlockKind::Paragraph(self.inl(&shown)) });
        }
        if literal {
            let mut k = j;
            while k < lines.len() && blank(lines[k].text) {
                k += 1;
            }
            if k < lines.len() && indent(lines[k].text) > 0 {
                let mut e = k;
                let mut body = vec![];
                while e < lines.len() {
                    if blank(lines[e].text) {
                        let mut f = e;
                        while f < lines.len() && blank(lines[f].text) {
                            f += 1;
                        }
                        if f < lines.len() && indent(lines[f].text) > 0 {
                            for b in &lines[e..f] {
                                body.push(Line { text: "", no: b.no });
                            }
                            e = f;
                            continue;
                        }
                        break;
                    }
                    if indent(lines[e].text) == 0 {
                        break;
                    }
                    body.push(lines[e]);
                    e += 1;
                }
                let inner = dedent_lines(&body);
                out.push(Block { line: lines[k].no, kind: BlockKind::Code { lang: String::new(), text: inner.iter().map(|l| l.text).collect::<Vec<_>>().join("\n").trim_matches('\n').to_string() } });
                return e;
            }
        }
        j
    }

    fn bullets(&mut self, lines: &[Line], start: usize, width: usize, out: &mut Vec<Block>) -> usize {
        let mark = lines[start].text.chars().next().unwrap_or('-');
        let mut items = vec![];
        let mut i = start;
        let mut w = width;
        loop {
            let (item, j) = self.item(lines, i, w);
            items.push(item);
            // the next item: the same bullet, at the same indentation, after at most blank lines
            let mut k = j;
            while k < lines.len() && blank(lines[k].text) {
                k += 1;
            }
            match lines.get(k).filter(|l| indent(l.text) == 0 && l.text.starts_with(mark)).and_then(|l| bullet(l.text)) {
                Some(nw) => {
                    i = k;
                    w = nw;
                }
                None => {
                    i = j;
                    break;
                }
            }
        }
        out.push(Block { line: lines[start].no, kind: BlockKind::List { ordered: false, start: 0, items } });
        i
    }

    fn enumerated(&mut self, lines: &[Line], start: usize, first: u64, width: usize, out: &mut Vec<Block>) -> usize {
        let mut items = vec![];
        let mut i = start;
        let mut w = width;
        loop {
            let (item, j) = self.item(lines, i, w);
            items.push(item);
            let mut k = j;
            while k < lines.len() && blank(lines[k].text) {
                k += 1;
            }
            match lines.get(k).filter(|l| indent(l.text) == 0).and_then(|l| enum_marker(l.text)) {
                Some((_, nw)) => {
                    i = k;
                    w = nw;
                }
                None => {
                    i = j;
                    break;
                }
            }
        }
        out.push(Block { line: lines[start].no, kind: BlockKind::List { ordered: true, start: first, items } });
        i
    }

    /// One list item: the text after the marker and the lines indented to line up with it.
    fn item(&mut self, lines: &[Line], i: usize, width: usize) -> (ListItem, usize) {
        let l = lines[i];
        let mut inner = vec![Line { text: &l.text[width.min(l.text.len())..], no: l.no }];
        let mut j = i + 1;
        while j < lines.len() {
            let t = lines[j];
            if blank(t.text) {
                let mut k = j;
                while k < lines.len() && blank(lines[k].text) {
                    k += 1;
                }
                if k < lines.len() && indent(lines[k].text) >= width {
                    for b in &lines[j..k] {
                        inner.push(Line { text: "", no: b.no });
                    }
                    j = k;
                    continue;
                }
                break;
            }
            if indent(t.text) < width {
                break;
            }
            inner.push(Line { text: &t.text[width..], no: t.no });
            j += 1;
        }
        let blocks = self.blocks(&inner);
        (ListItem { checked: None, blocks }, j)
    }

    fn fields(&mut self, lines: &[Line], start: usize, out: &mut Vec<Block>) -> usize {
        let mut defs = vec![];
        let mut i = start;
        loop {
            let (name, rest) = field_marker(lines[i].text).unwrap_or(("", ""));
            let (body, j) = self.indented_body(lines, i);
            let mut inner = vec![];
            if !rest.is_empty() {
                inner.push(Line { text: rest, no: lines[i].no });
            }
            inner.extend(dedent_lines(&body));
            let blocks = self.blocks(&inner);
            defs.push(Definition { term: vec![Inline::Strong(self.inl(name))], blocks });
            let mut k = j;
            while k < lines.len() && blank(lines[k].text) {
                k += 1;
            }
            if k < lines.len() && indent(lines[k].text) == 0 && field_marker(lines[k].text).is_some() {
                i = k;
            } else {
                i = j;
                break;
            }
        }
        out.push(Block { line: lines[start].no, kind: BlockKind::Definitions(defs) });
        i
    }

    fn definitions(&mut self, lines: &[Line], start: usize, out: &mut Vec<Block>) -> usize {
        let mut defs = vec![];
        let mut i = start;
        loop {
            let term = lines[i].text.trim();
            let (body, j) = self.indented_body(lines, i);
            let inner = dedent_lines(&body);
            let blocks = self.blocks(&inner);
            // `term : classifier`
            let term_inlines = match term.split_once(" : ") {
                Some((t, c)) => {
                    let mut v = vec![Inline::Strong(self.inl(t.trim()))];
                    v.push(Inline::Text("  ".into()));
                    v.push(Inline::Emph(self.inl(c.trim())));
                    v
                }
                None => vec![Inline::Strong(self.inl(term))],
            };
            defs.push(Definition { term: term_inlines, blocks });
            let mut k = j;
            while k < lines.len() && blank(lines[k].text) {
                k += 1;
            }
            let plain_term = |l: &str| !(l.starts_with("..") || l.starts_with(':') || bullet(l).is_some() || enum_marker(l).is_some() || punct_line(l).is_some() || l.starts_with("| "));
            if k + 1 < lines.len() && indent(lines[k].text) == 0 && plain_term(lines[k].text) && indent(lines[k + 1].text) > 0 && !blank(lines[k + 1].text) {
                i = k;
            } else {
                i = j;
                break;
            }
        }
        out.push(Block { line: lines[start].no, kind: BlockKind::Definitions(defs) });
        i
    }

    fn line_block(&mut self, lines: &[Line], i: usize, out: &mut Vec<Block>) -> usize {
        let mut j = i;
        let mut inl: Vec<Inline> = vec![];
        while j < lines.len() && (lines[j].text.starts_with("| ") || lines[j].text == "|" || (indent(lines[j].text) > 0 && !blank(lines[j].text))) {
            let t = lines[j].text;
            if let Some(rest) = t.strip_prefix('|') {
                if j > i {
                    inl.push(Inline::HardBreak);
                }
                inl.extend(self.inl(rest.trim()));
            } else {
                inl.push(Inline::SoftBreak);
                inl.extend(self.inl(t.trim()));
            }
            j += 1;
        }
        out.push(Block { line: lines[i].no, kind: BlockKind::Paragraph(crate::model::normalize(inl)) });
        j
    }

    // ------------------------------------------------------------------ tables

    fn cell(&self, text: &str) -> Vec<Inline> {
        self.inl(text.trim())
    }

    fn grid_table(&mut self, lines: &[Line], i: usize) -> (Option<Block>, usize) {
        let mut j = i;
        let mut rows: Vec<Vec<char>> = vec![];
        while j < lines.len() && !blank(lines[j].text) && (lines[j].text.starts_with('+') || lines[j].text.starts_with('|')) {
            rows.push(lines[j].text.chars().collect());
            j += 1;
        }
        let top = &rows[0];
        let stops: Vec<usize> = top.iter().enumerate().filter(|(_, c)| **c == '+').map(|(k, _)| k).collect();
        if stops.len() < 2 || rows.len() < 3 {
            return (None, i);
        }
        let mut regions: Vec<Vec<Vec<Inline>>> = vec![];
        let mut header_rows = 0;
        let mut cur: Vec<String> = vec![String::new(); stops.len() - 1];
        let mut has_content = false;
        for (k, r) in rows.iter().enumerate().skip(1) {
            let is_border = r.first() == Some(&'+') && r.iter().all(|c| matches!(c, '+' | '-' | '=' | ':'));
            if is_border {
                if has_content {
                    regions.push(cur.iter().map(|t| self.cell(t)).collect());
                    cur = vec![String::new(); stops.len() - 1];
                    has_content = false;
                    if r.contains(&'=') {
                        header_rows = regions.len();
                    }
                }
                continue;
            }
            let _ = k;
            for (c, w) in stops.windows(2).enumerate() {
                let (a, b) = (w[0] + 1, w[1].min(r.len()));
                if a < b {
                    let piece: String = r[a..b].iter().collect();
                    if !piece.trim().is_empty() {
                        if !cur[c].is_empty() {
                            cur[c].push('\n');
                        }
                        cur[c].push_str(piece.trim());
                    }
                }
            }
            has_content = true;
        }
        if has_content {
            regions.push(cur.iter().map(|t| self.cell(t)).collect());
        }
        if regions.is_empty() {
            return (None, i);
        }
        let ncols = stops.len() - 1;
        let (header, body) = if header_rows > 0 {
            let header = regions[0].clone();
            let body = regions[1..].to_vec();
            (header, body)
        } else {
            (vec![], regions)
        };
        (Some(Block { line: lines[i].no, kind: BlockKind::Table { align: vec![Align::Left; ncols], header, rows: body } }), j)
    }

    fn simple_table(&mut self, lines: &[Line], i: usize, cols: &[(usize, usize)]) -> (Option<Block>, usize) {
        let mut j = i + 1;
        let mut borders = vec![];
        let mut rows: Vec<(bool, Vec<String>)> = vec![];
        while j < lines.len() && !blank(lines[j].text) {
            let t = lines[j].text;
            if simple_border(t).is_some() || (t.starts_with('=') && t.chars().all(|c| c == '=' || c == ' ')) {
                borders.push(rows.len());
                j += 1;
                continue;
            }
            // underline rows (column spans) are ignored
            if t.chars().all(|c| c == '-' || c == ' ') {
                j += 1;
                continue;
            }
            let chars: Vec<char> = t.chars().collect();
            let mut cells = vec![];
            for (c, (st, _)) in cols.iter().enumerate() {
                let en = cols.get(c + 1).map_or(chars.len(), |n| n.0);
                let piece: String = chars.get(*st..en.min(chars.len())).map(|s| s.iter().collect()).unwrap_or_default();
                cells.push(piece.trim().to_string());
            }
            let continuation = cells[0].is_empty() && !rows.is_empty();
            rows.push((continuation, cells));
            j += 1;
        }
        if rows.is_empty() || borders.is_empty() {
            return (None, i);
        }
        // merge continuation lines into the row above
        let mut merged: Vec<Vec<String>> = vec![];
        let mut start_of: Vec<usize> = vec![];
        for (idx, (cont, cells)) in rows.iter().enumerate() {
            if *cont && let Some(last) = merged.last_mut() {
                for (c, t) in cells.iter().enumerate() {
                    if !t.is_empty() {
                        if !last[c].is_empty() {
                            last[c].push('\n');
                        }
                        last[c].push_str(t);
                    }
                }
            } else {
                merged.push(cells.clone());
                start_of.push(idx);
            }
        }
        // a border between rows (not the closing one) ends the header
        let mid = borders.iter().copied().find(|b| *b > 0 && *b < rows.len());
        let to_inl = |p: &Self, r: &Vec<String>| -> Vec<Vec<Inline>> { r.iter().map(|t| p.cell(t)).collect() };
        let (header, body): (Vec<Vec<Inline>>, Vec<Vec<Vec<Inline>>>) = match mid {
            Some(_) => (to_inl(self, &merged[0]), merged[1..].iter().map(|r| to_inl(self, r)).collect()),
            None => (vec![], merged.iter().map(|r| to_inl(self, r)).collect()),
        };
        let _ = start_of;
        (Some(Block { line: lines[i].no, kind: BlockKind::Table { align: vec![Align::Left; cols.len()], header, rows: body } }), j)
    }

    // ------------------------------------------------------------------ explicit markup

    fn explicit(&mut self, lines: &[Line], i: usize, out: &mut Vec<Block>) -> usize {
        let first = lines[i].text;
        let (body, j) = self.indented_body(lines, i);
        let rest = first[2..].trim_start();
        // targets and substitutions were read in advance
        if rest.starts_with('_') || rest.starts_with('|') {
            return j;
        }
        // a footnote or citation: [1] text
        if let Some(r) = rest.strip_prefix('[')
            && let Some(e) = r.find(']')
        {
            let label = &r[..e];
            let mut inner = vec![];
            let text = format!("**[{}]** {}", label.trim_start_matches('#'), r[e + 1..].trim());
            inner.push(Line { text: &text, no: lines[i].no });
            inner.extend(dedent_lines(&body));
            // the text borrows from a local: build the blocks before it goes away
            let blocks = self.blocks(&inner);
            out.extend(blocks);
            return j;
        }
        // a directive: name::
        if let Some(p) = rest.find("::")
            && p > 0
            && rest[..p].chars().all(|c| c.is_alphanumeric() || "-_.+:".contains(c))
            && rest[p + 2..].chars().next().is_none_or(|c| c == ' ')
            && rest.chars().next().is_some_and(|c| c.is_alphabetic())
        {
            let name = rest[..p].to_ascii_lowercase();
            let mut arg = rest[p + 2..].trim().to_string();
            let dedented = dedent_lines(&body);
            let mut k = 0;
            while k < dedented.len() && !blank(dedented[k].text) && !dedented[k].text.starts_with(':') {
                arg.push(' ');
                arg.push_str(dedented[k].text.trim());
                k += 1;
            }
            let mut options: Vec<(String, String)> = vec![];
            while k < dedented.len() && !blank(dedented[k].text) {
                if let Some((n, v)) = field_marker(dedented[k].text) {
                    options.push((n.to_string(), v.to_string()));
                }
                k += 1;
            }
            while k < dedented.len() && blank(dedented[k].text) {
                k += 1;
            }
            let content: Vec<Line> = dedented[k..].to_vec();
            let blocks = self.directive(&name, arg.trim(), &options, &content, lines[i].no);
            out.extend(blocks);
            return j;
        }
        // anything else is a comment
        j
    }

    fn content_blocks(&mut self, content: &[Line]) -> Vec<Block> {
        let mut c = content.to_vec();
        while c.last().is_some_and(|l| blank(l.text)) {
            c.pop();
        }
        self.blocks(&c)
    }

    fn directive(&mut self, name: &str, arg: &str, options: &[(String, String)], content: &[Line], line: usize) -> Vec<Block> {
        let opt = |n: &str| options.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
        let adm = |p: &mut Self, kind: &str, title: &str, with_arg: bool| -> Vec<Block> {
            let mut inner: Vec<Line> = vec![];
            let arg_owned = arg.to_string();
            if with_arg && !arg_owned.is_empty() {
                // the argument is the first paragraph of an admonition
                let blocks_arg = p.inl(&arg_owned);
                let mut blocks = vec![Block { line, kind: BlockKind::Paragraph(blocks_arg) }];
                blocks.extend(p.content_blocks(content));
                return vec![Block { line, kind: BlockKind::Admonition { kind: kind.into(), title: title.into(), blocks } }];
            }
            inner.extend_from_slice(content);
            let blocks = p.content_blocks(&inner);
            vec![Block { line, kind: BlockKind::Admonition { kind: kind.into(), title: title.into(), blocks } }]
        };
        match name {
            "attention" | "caution" | "danger" | "error" | "hint" | "important" | "note" | "tip" | "warning" | "todo" => adm(self, name, "", true),
            "seealso" => adm(self, "seealso", "See also", true),
            "admonition" => adm(self, "note", arg, false),
            "versionadded" => adm(self, "tip", &format!("New in version {arg}"), false),
            "versionchanged" => adm(self, "note", &format!("Changed in version {arg}"), false),
            "versionremoved" | "deprecated" => adm(self, "warning", &if name == "deprecated" { format!("Deprecated since version {arg}") } else { format!("Removed in version {arg}") }, false),
            "code-block" | "code" | "sourcecode" | "parsed-literal" | "math" | "literal-block" => {
                let text = content.iter().map(|l| l.text).collect::<Vec<_>>().join("\n");
                let lang = match name {
                    "math" => "math".to_string(),
                    "parsed-literal" => String::new(),
                    _ => arg.to_string(),
                };
                let text = if name == "math" && text.trim().is_empty() { arg.to_string() } else { text };
                vec![Block { line, kind: BlockKind::Code { lang, text: text.trim_matches('\n').to_string() } }]
            }
            "image" | "figure" => {
                let img = Inline::Image { src: arg.to_string(), alt: opt("alt").unwrap_or("").to_string(), title: String::new() };
                let node = match opt("target") {
                    Some(t) => Inline::Link { url: t.to_string(), title: String::new(), children: vec![img] },
                    None => img,
                };
                let mut v = vec![Block { line, kind: BlockKind::Paragraph(vec![node]) }];
                if name == "figure" {
                    v.extend(self.content_blocks(content));
                }
                v
            }
            "toctree" => {
                let mut items = vec![];
                for l in content {
                    let t = l.text.trim();
                    if t.is_empty() || t.starts_with(':') {
                        continue;
                    }
                    let shown = match t.rsplit_once(" <") {
                        Some((title, rest)) if rest.ends_with('>') => title.trim().to_string(),
                        _ => t.to_string(),
                    };
                    items.push(ListItem { checked: None, blocks: vec![Block { line: l.no, kind: BlockKind::Paragraph(vec![Inline::Text(shown)]) }] });
                }
                if items.is_empty() { vec![] } else { vec![Block { line, kind: BlockKind::List { ordered: false, start: 0, items } }] }
            }
            "list-table" => {
                let blocks = self.content_blocks(content);
                let hr: usize = opt("header-rows").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                let mut rows: Vec<Vec<Vec<Inline>>> = vec![];
                if let Some(Block { kind: BlockKind::List { items, .. }, .. }) = blocks.first() {
                    for it in items {
                        if let Some(Block { kind: BlockKind::List { items: cells, .. }, .. }) = it.blocks.first() {
                            rows.push(cells.iter().map(|c| join_paragraphs(&c.blocks)).collect());
                        }
                    }
                }
                if rows.is_empty() {
                    return vec![];
                }
                let ncols = rows.iter().map(Vec::len).max().unwrap_or(1);
                for r in rows.iter_mut() {
                    r.resize(ncols, vec![]);
                }
                let header = if hr > 0 { rows.remove(0) } else { vec![] };
                let rows = if hr > 1 { rows.split_off((hr - 1).min(rows.len())) } else { rows };
                vec![Block { line, kind: BlockKind::Table { align: vec![Align::Left; ncols], header, rows } }]
            }
            "csv-table" => {
                let mut rows: Vec<Vec<Vec<Inline>>> = content.iter().filter(|l| !blank(l.text)).map(|l| csv_line(l.text).iter().map(|c| self.cell(c)).collect()).collect();
                let hr: usize = opt("header-rows").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                let header: Vec<Vec<Inline>> = match opt("header") {
                    Some(h) => csv_line(h).iter().map(|c| self.cell(c)).collect(),
                    None if hr > 0 && !rows.is_empty() => rows.remove(0),
                    None => vec![],
                };
                if rows.is_empty() && header.is_empty() {
                    return vec![];
                }
                let ncols = rows.iter().map(Vec::len).chain([header.len()]).max().unwrap_or(1);
                for r in rows.iter_mut() {
                    r.resize(ncols, vec![]);
                }
                vec![Block { line, kind: BlockKind::Table { align: vec![Align::Left; ncols], header, rows } }]
            }
            "rubric" => vec![Block { line, kind: BlockKind::Paragraph(vec![Inline::Strong(self.inl(arg))]) }],
            "epigraph" | "pull-quote" | "highlights" | "blockquote" => {
                let blocks = self.content_blocks(content);
                vec![Block { line, kind: BlockKind::Quote(blocks) }]
            }
            "topic" | "sidebar" => {
                let mut blocks = vec![Block { line, kind: BlockKind::Paragraph(vec![Inline::Strong(self.inl(arg))]) }];
                blocks.extend(self.content_blocks(content));
                vec![Block { line, kind: BlockKind::Admonition { kind: "note".into(), title: String::new(), blocks } }]
            }
            "contents" | "sectnum" | "section-numbering" | "index" | "meta" | "highlight" | "default-role" | "role" | "include" | "literalinclude" | "only" | "raw" | "module" | "currentmodule" | "sectionauthor" | "codeauthor" | "tabularcolumns" | "cssclass" | "title" | "date" | "header" | "footer" | "ifconfig" | "automodule" | "autosummary" | "autoclass" | "autofunction" | "automethod" | "autodata" | "autoattribute" | "autoexception" | "toctree-hidden" | "productionlist" | "rst-class" | "class" | "target-notes" | "unicode" | "replace" | "hlist" => vec![],
            n if n.contains(':') || matches!(n, "function" | "method" | "classmethod" | "staticmethod" | "data" | "attribute" | "exception" | "decorator" | "describe" | "cmdoption" | "option" | "envvar" | "program") => {
                // Sphinx object descriptions: the signature, then what it does
                let blocks = self.content_blocks(content);
                vec![Block { line, kind: BlockKind::Definitions(vec![Definition { term: vec![Inline::Code(arg.to_string())], blocks }]) }]
            }
            _ => self.content_blocks(content),
        }
    }
}

fn join_paragraphs(blocks: &[Block]) -> Vec<Inline> {
    let mut out: Vec<Inline> = vec![];
    for b in blocks {
        if let BlockKind::Paragraph(p) = &b.kind {
            if !out.is_empty() {
                out.push(Inline::SoftBreak);
            }
            out.extend(p.iter().cloned());
        }
    }
    out
}

fn csv_line(s: &str) -> Vec<String> {
    let mut cells = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' if quoted && it.peek() == Some(&'"') => {
                cur.push('"');
                it.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => cells.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BlockKind as K;

    fn kinds(doc: &Document) -> Vec<&'static str> {
        doc.blocks
            .iter()
            .map(|b| match &b.kind {
                K::Heading { .. } => "h",
                K::Paragraph(_) => "p",
                K::Code { .. } => "code",
                K::List { .. } => "list",
                K::Quote(_) => "quote",
                K::Admonition { .. } => "admon",
                K::Rule => "hr",
                K::Table { .. } => "table",
                K::Definitions(_) => "defs",
            })
            .collect()
    }

    #[test]
    fn sections_take_their_level_from_the_underline_style() {
        let d = parse("Title\n=====\n\nIntro text.\n\nPart\n----\n\nSub\n~~~\n\n=====\nOver\n=====\n\nBack\n----\n");
        assert_eq!(kinds(&d), ["h", "p", "h", "h", "h", "h"]);
        let lv: Vec<u8> = d.blocks.iter().filter_map(|b| if let K::Heading { level, .. } = &b.kind { Some(*level) } else { None }).collect();
        assert_eq!(lv, [1, 2, 3, 4, 2]);
        assert_eq!(d.blocks[2].line, 5);
    }

    #[test]
    fn paragraphs_literals_and_transitions() {
        let d = parse("Some text with ``code`` and *emphasis*.\nSecond line.\n\nExample::\n\n    x = 1\n\n    y = 2\n\nafter\n\n----\n\nlast::\n\n    z\n");
        assert_eq!(kinds(&d), ["p", "p", "code", "p", "hr", "p", "code"]);
        let K::Code { text, .. } = &d.blocks[2].kind else { panic!() };
        assert_eq!(text, "x = 1\n\ny = 2");
        let K::Paragraph(p) = &d.blocks[1].kind else { panic!() };
        assert_eq!(p, &vec![Inline::Text("Example:".into())]);
    }

    #[test]
    fn lists() {
        let d = parse("- one\n- two\n\n  more of two\n\n  - nested\n- three\n\n1. a\n2. b\n\n#. x\n#. y\n");
        assert_eq!(kinds(&d), ["list", "list"]);
        let K::List { items, .. } = &d.blocks[0].kind else { panic!() };
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].blocks.len(), 3);
        // `#.` goes on with the numbers above it
        let K::List { ordered, items, .. } = &d.blocks[1].kind else { panic!() };
        assert_eq!((*ordered, items.len()), (true, 4));
    }

    #[test]
    fn definition_and_field_lists() {
        let d = parse("term\n    its definition\n\nother : type\n    more\n\n:param x: the x\n:returns: nothing\n    really\n");
        assert_eq!(kinds(&d), ["defs", "defs"]);
        let K::Definitions(v) = &d.blocks[0].kind else { panic!() };
        assert_eq!(v.len(), 2);
        let K::Definitions(f) = &d.blocks[1].kind else { panic!() };
        assert_eq!(f.len(), 2);
        assert_eq!(plain_text(&f[0].term), "param x");
    }

    #[test]
    fn block_quotes_and_line_blocks() {
        let d = parse("text\n\n    quoted\n\n| line one\n| line two\n");
        assert_eq!(kinds(&d), ["p", "quote", "p"]);
        let K::Paragraph(p) = &d.blocks[2].kind else { panic!() };
        assert!(p.contains(&Inline::HardBreak));
    }

    #[test]
    fn admonitions_and_code_blocks() {
        let d = parse(".. note:: Careful here\n   and here.\n\n.. warning::\n\n   Do not\n\n   do this.\n\n.. code-block:: python\n   :linenos:\n\n   def f():\n       pass\n\n.. versionadded:: 3.2\n\n   It exists.\n");
        assert_eq!(kinds(&d), ["admon", "admon", "code", "admon"]);
        let K::Admonition { kind, blocks, .. } = &d.blocks[0].kind else { panic!() };
        assert_eq!((kind.as_str(), blocks.len()), ("note", 1));
        let K::Admonition { blocks, .. } = &d.blocks[1].kind else { panic!() };
        assert_eq!(blocks.len(), 2);
        let K::Code { lang, text } = &d.blocks[2].kind else { panic!() };
        assert_eq!((lang.as_str(), text.as_str()), ("python", "def f():\n    pass"));
        let K::Admonition { title, .. } = &d.blocks[3].kind else { panic!() };
        assert_eq!(title, "New in version 3.2");
    }

    #[test]
    fn comments_targets_and_substitutions_are_not_shown() {
        let d = parse(".. a comment\n   over two lines\n\n.. _python: https://python.org\n\n.. |ver| replace:: 1.2\n\nSee Python_, `Python`_ and |ver|.\n");
        assert_eq!(kinds(&d), ["p"]);
        let K::Paragraph(p) = &d.blocks[0].kind else { panic!() };
        assert!(matches!(&p[1], Inline::Link { url, .. } if url == "https://python.org"));
        assert!(matches!(&p[3], Inline::Link { url, .. } if url == "https://python.org"));
        assert_eq!(p.last(), Some(&Inline::Text(" and 1.2.".into())));
    }

    #[test]
    fn section_titles_can_be_referenced() {
        let d = parse("Intro\n=====\n\nsee Intro_ and Missing_.\n");
        let K::Paragraph(p) = &d.blocks[1].kind else { panic!() };
        assert!(matches!(&p[1], Inline::Link { url, .. } if url == "#intro"));
    }

    #[test]
    fn grid_tables() {
        let d = parse("+-------+-------+\n| Name  | Value |\n+=======+=======+\n| a     | 1     |\n+-------+-------+\n| b     | 2     |\n+-------+-------+\n");
        let K::Table { header, rows, .. } = &d.blocks[0].kind else { panic!("{:?}", d.blocks) };
        assert_eq!(plain_text(&header[0]), "Name");
        assert_eq!(rows.len(), 2);
        assert_eq!(plain_text(&rows[1][1]), "2");
    }

    #[test]
    fn simple_tables() {
        let d = parse("=====  =====\nName   Value\n=====  =====\nx      1\ny      2\n=====  =====\n");
        let K::Table { header, rows, .. } = &d.blocks[0].kind else { panic!("{:?}", d.blocks) };
        assert_eq!((plain_text(&header[0]), plain_text(&header[1])), ("Name".into(), "Value".into()));
        assert_eq!(rows.len(), 2);
        assert_eq!(plain_text(&rows[0][0]), "x");
    }

    #[test]
    fn list_and_csv_tables() {
        let d = parse(".. list-table::\n   :header-rows: 1\n\n   * - a\n     - b\n   * - 1\n     - 2\n\n.. csv-table::\n   :header: \"x\", \"y\"\n\n   1, 2\n   3, 4\n");
        assert_eq!(kinds(&d), ["table", "table"]);
        let K::Table { header, rows, .. } = &d.blocks[0].kind else { panic!() };
        assert_eq!((header.len(), rows.len()), (2, 1));
        let K::Table { header, rows, .. } = &d.blocks[1].kind else { panic!() };
        assert_eq!((header.len(), rows.len()), (2, 2));
    }

    #[test]
    fn images_toctree_and_domains() {
        let d = parse(".. image:: logo.png\n   :alt: Logo\n\n.. toctree::\n   :maxdepth: 2\n\n   intro\n   Guide <guide>\n\n.. py:function:: f(a, b)\n\n   Does f.\n");
        assert_eq!(kinds(&d), ["p", "list", "defs"]);
        let K::Paragraph(p) = &d.blocks[0].kind else { panic!() };
        assert!(matches!(&p[0], Inline::Image { src, alt, .. } if src == "logo.png" && alt == "Logo"));
        let K::List { items, .. } = &d.blocks[1].kind else { panic!() };
        assert_eq!(items.len(), 2);
        let K::Definitions(v) = &d.blocks[2].kind else { panic!() };
        assert_eq!(v[0].term, vec![Inline::Code("f(a, b)".into())]);
    }

    #[test]
    fn footnotes() {
        let d = parse("text [1]_\n\n.. [1] The note.\n");
        assert_eq!(kinds(&d), ["p", "p"]);
        let K::Paragraph(p) = &d.blocks[1].kind else { panic!() };
        assert_eq!(p[0], Inline::Strong(vec![Inline::Text("[1]".into())]));
    }

    #[test]
    fn doctests_and_crlf() {
        let d = parse(">>> 1 + 1\n2\n\r\nafter\r\n");
        assert_eq!(kinds(&d), ["code", "p"]);
    }

    #[test]
    fn empty_documents() {
        assert!(parse("").blocks.is_empty());
        assert!(parse("\n  \n").blocks.is_empty());
    }
}
