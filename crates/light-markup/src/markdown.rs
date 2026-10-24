//! Markdown (CommonMark with the GitHub extensions people use: tables, task lists, strikethrough, autolinks, alerts)
//! parsed into a [`Document`]. Containers (quotes, list items) are parsed by taking their prefix off the lines and
//! parsing what is left again, so nesting needs no special cases.

use crate::html;
use crate::md_inline::{Refs, normalize_label, parse_inlines};
use crate::model::{Align, Anchors, Block, BlockKind, Document, Inline, ListItem, plain_text};

#[derive(Clone, Copy)]
struct Line<'a> {
    text: &'a str,
    /// Source line (0-based).
    no: usize,
}

pub fn parse(text: &str) -> Document {
    let owned: Vec<String> = text.split('\n').map(|l| expand_leading_tabs(l.strip_suffix('\r').unwrap_or(l))).collect();
    let lines: Vec<Line> = owned.iter().enumerate().map(|(no, t)| Line { text: t.as_str(), no }).collect();
    let refs = collect_refs(&lines);
    let mut p = Parser { refs, anchors: Anchors::default() };
    Document { blocks: p.blocks(&lines) }
}

fn expand_leading_tabs(l: &str) -> String {
    if !l.contains('\t') {
        return l.to_string();
    }
    // a tab after a list marker is the spaces up to the next tab stop
    let ws = l.len() - l.trim_start_matches([' ', '\t']).len();
    if !l[..ws].contains('\t') && list_marker_with_tab(&l[ws..]) {
        let at = ws + l[ws..].find('\t').unwrap_or(0);
        let col = l[..at].chars().count();
        let n = 4 - col % 4;
        return format!("{}{}{}", &l[..at], " ".repeat(n), &l[at + 1..]);
    }
    if !l.starts_with(['\t', ' ']) {
        return l.to_string();
    }
    let ws = l.len() - l.trim_start_matches([' ', '\t']).len();
    let mut col = 0;
    let mut out = String::new();
    for c in l[..ws].chars() {
        if c == '\t' {
            let n = 4 - col % 4;
            out.push_str(&" ".repeat(n));
            col += n;
        } else {
            out.push(' ');
            col += 1;
        }
    }
    out.push_str(&l[ws..]);
    out
}

/// `-\t` or `1.\t` at the start of `s`.
fn list_marker_with_tab(s: &str) -> bool {
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    match digits {
        0 => s.starts_with(['-', '*', '+']) && s[1..].starts_with('\t'),
        1..=9 => s[digits..].starts_with(['.', ')']) && s[digits + 1..].starts_with('\t'),
        _ => false,
    }
}

fn indent(s: &str) -> usize {
    s.len() - s.trim_start_matches(' ').len()
}

fn blank(s: &str) -> bool {
    s.trim().is_empty()
}

fn strip_indent(s: &str, n: usize) -> &str {
    let k = indent(s).min(n);
    &s[k..]
}

fn fence_open(s: &str) -> Option<(char, usize, String)> {
    let c = s.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = s.chars().take_while(|x| *x == c).count();
    if n < 3 {
        return None;
    }
    let info = s[n..].trim();
    (c != '`' || !info.contains('`')).then(|| (c, n, info.to_string()))
}

fn is_hr(s: &str) -> bool {
    let t = s.trim();
    let Some(c) = t.chars().next().filter(|c| matches!(c, '-' | '*' | '_')) else { return false };
    t.chars().filter(|x| !x.is_whitespace()).all(|x| x == c) && t.chars().filter(|x| *x == c).count() >= 3
}

fn atx(s: &str) -> Option<(u8, &str)> {
    let n = s.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&n) {
        return None;
    }
    let rest = &s[n..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let t = rest.trim();
    // a closing run of #
    let open = t.trim_end_matches('#');
    let t = if open.len() == t.len() {
        t
    } else if open.is_empty() {
        ""
    } else if open.ends_with(' ') {
        open.trim_end()
    } else {
        t
    };
    Some((n as u8, t))
}

/// `-`, `*`, `+` or `1.` / `1)` at the start of `s` (already without indentation): the marker and what follows.
struct Marker {
    ordered: bool,
    start: u64,
    /// The bullet, or `.` / `)`.
    kind: char,
    /// Width of the marker and the spaces after it, counted so that the content lines up.
    width: usize,
    /// Byte offset of the content in `s`.
    content_at: usize,
}

fn list_marker(s: &str) -> Option<Marker> {
    let mut it = s.char_indices();
    let (_, c) = it.next()?;
    let (ordered, start, kind, mlen) = if matches!(c, '-' | '*' | '+') {
        (false, 0, c, 1)
    } else if c.is_ascii_digit() {
        let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 9 {
            return None;
        }
        let d = s[digits..].chars().next().filter(|d| *d == '.' || *d == ')')?;
        (true, s[..digits].parse().ok()?, d, digits + 1)
    } else {
        return None;
    };
    let rest = &s[mlen..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let spaces = indent(rest);
    let blank_rest = rest.trim().is_empty();
    // five or more spaces: the content is an indented code block, the marker takes one space
    let used = if blank_rest || spaces > 4 { 1.min(spaces) } else { spaces };
    Some(Marker { ordered, start, kind, width: mlen + used.max(if blank_rest { 1 } else { 0 }), content_at: mlen + used })
}

fn table_delim(s: &str) -> Option<Vec<Align>> {
    let t = s.trim();
    if !t.contains('-') || !t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
        return None;
    }
    let cells = split_row(t);
    if cells.is_empty() {
        return None;
    }
    cells
        .iter()
        .map(|c| {
            let c = c.trim();
            let (l, r) = (c.starts_with(':'), c.ends_with(':'));
            let core = c.trim_matches(':');
            (!core.is_empty() && core.chars().all(|x| x == '-')).then_some(match (l, r) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}

fn split_row(s: &str) -> Vec<String> {
    let t = s.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let mut cells = vec![];
    let mut cur = String::new();
    let mut it = t.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\\' if it.peek() == Some(&'|') => {
                cur.push('|');
                it.next();
            }
            '|' => cells.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() || cells.is_empty() {
        cells.push(cur);
    }
    cells
}

fn collect_refs(lines: &[Line]) -> Refs {
    let mut refs = Refs::default();
    let mut fence: Option<(char, usize)> = None;
    for l in lines {
        let s = l.text.trim_start();
        if let Some((c, n)) = fence {
            if s.starts_with(&c.to_string().repeat(n)) && s.trim_end_matches(c).is_empty() {
                fence = None;
            }
            continue;
        }
        if let Some((c, n, _)) = fence_open(s) {
            fence = Some((c, n));
            continue;
        }
        if indent(l.text) > 3 {
            continue;
        }
        // [label]: url "title"
        let Some(rest) = s.strip_prefix('[') else { continue };
        let Some(close) = rest.find("]:") else { continue };
        let label = &rest[..close];
        if label.is_empty() || label.contains('[') || label.starts_with('^') {
            continue;
        }
        let after = rest[close + 2..].trim();
        let (url, tail) = match after.strip_prefix('<') {
            Some(r) => match r.find('>') {
                Some(e) => (&r[..e], r[e + 1..].trim()),
                None => continue,
            },
            None => match after.split_once(char::is_whitespace) {
                Some((u, t)) => (u, t.trim()),
                None => (after, ""),
            },
        };
        if url.is_empty() {
            continue;
        }
        let title = tail.trim_matches(|c| c == '"' || c == '\'' || c == '(' || c == ')');
        refs.0.entry(normalize_label(label)).or_insert((url.to_string(), title.to_string()));
    }
    refs
}

fn is_ref_def(s: &str) -> bool {
    let Some(rest) = s.trim_start().strip_prefix('[') else { return false };
    rest.find("]:").is_some_and(|c| c > 0 && !rest[..c].contains('[') && !rest.starts_with('^') && !rest[c + 2..].trim().is_empty())
}

fn html_block_start(s: &str) -> bool {
    if s.starts_with("<!--") {
        return true;
    }
    let chars: Vec<char> = s.chars().take(200).collect();
    html::parse_tag(&chars).is_some_and(|t| {
        matches!(
            t.name.as_str(),
            "p" | "div" | "details" | "summary" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "hr" | "br" | "img" | "a" | "table" | "ul" | "ol" | "li" | "center" | "pre" | "picture" | "source" | "section" | "br/" | "blockquote" | "kbd" | "b" | "i" | "em" | "strong" | "span"
        ) && !(matches!(t.name.as_str(), "a" | "b" | "i" | "em" | "strong" | "span" | "kbd" | "img") && {
            // an inline tag in running text is a paragraph; it is a block only when it stands alone on its line
            let rest: String = chars[t.len..].iter().collect();
            !rest.trim().is_empty() && !rest.trim_start().starts_with('<')
        })
    })
}

struct Parser {
    refs: Refs,
    anchors: Anchors,
}

impl Parser {
    fn inl(&self, s: &str) -> Vec<Inline> {
        parse_inlines(s, &self.refs)
    }

    /// Does `l` end a paragraph that is being read?
    fn interrupts(&self, l: &str) -> bool {
        if blank(l) {
            return true;
        }
        let ind = indent(l);
        if ind > 3 {
            return false;
        }
        let s = &l[ind..];
        fence_open(s).is_some()
            || atx(s).is_some()
            || is_hr(s)
            || s.starts_with('>')
            || list_marker(s).is_some_and(|m| !m.ordered || m.start == 1) && !s[m_len(s)..].trim().is_empty()
            || (s.starts_with('<') && html_block_start(s) && !s.starts_with("<a ") && !s.starts_with("<b>") && !s.starts_with("<i>") && !s.starts_with("<em>") && !s.starts_with("<span"))
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
            let ind = indent(l.text);
            // indented code
            if ind >= 4 {
                let mut j = i;
                let mut body: Vec<&str> = vec![];
                while j < lines.len() && (blank(lines[j].text) || indent(lines[j].text) >= 4) {
                    body.push(strip_indent(lines[j].text, 4));
                    j += 1;
                }
                while body.last().is_some_and(|b| blank(b)) {
                    body.pop();
                }
                out.push(Block { line: l.no, kind: BlockKind::Code { lang: String::new(), text: body.join("\n") } });
                i = j;
                continue;
            }
            let s = &l.text[ind..];
            // fenced code
            if let Some((c, n, info)) = fence_open(s) {
                let mut j = i + 1;
                let mut body: Vec<&str> = vec![];
                while j < lines.len() {
                    let t = lines[j].text;
                    let ts = t.trim_start();
                    if indent(t) <= 3 && ts.starts_with(&c.to_string().repeat(n)) && ts.trim_end().chars().all(|x| x == c) {
                        j += 1;
                        break;
                    }
                    body.push(strip_indent(t, ind));
                    j += 1;
                }
                let lang = info.split_whitespace().next().unwrap_or("").trim_matches(|c| c == '{' || c == '}').trim_start_matches('.').to_string();
                out.push(Block { line: l.no, kind: BlockKind::Code { lang, text: body.join("\n") } });
                i = j;
                continue;
            }
            if let Some((level, t)) = atx(s) {
                let inlines = self.inl(t);
                let anchor = self.anchors.make(&plain_text(&inlines));
                out.push(Block { line: l.no, kind: BlockKind::Heading { level, inlines, anchor } });
                i += 1;
                continue;
            }
            if is_hr(s) {
                out.push(Block { line: l.no, kind: BlockKind::Rule });
                i += 1;
                continue;
            }
            if s.starts_with('>') {
                i = self.quote(lines, i, &mut out);
                continue;
            }
            if let Some(m) = list_marker(s) {
                i = self.list(lines, i, m, &mut out);
                continue;
            }
            if s.starts_with('<') && html_block_start(s) {
                i = self.html(lines, i, &mut out);
                continue;
            }
            if is_ref_def(s) {
                i += 1;
                continue;
            }
            // a table: a header row and the delimiter row under it
            if s.contains('|')
                && i + 1 < lines.len()
                && let Some(align) = table_delim(lines[i + 1].text)
                && split_row(s).len() == align.len()
            {
                let header: Vec<Vec<Inline>> = split_row(s).iter().map(|c| self.inl(c.trim())).collect();
                let mut rows = vec![];
                let mut j = i + 2;
                while j < lines.len() && !blank(lines[j].text) && !self.interrupts_table(lines[j].text) {
                    let mut cells: Vec<Vec<Inline>> = split_row(lines[j].text).iter().map(|c| self.inl(c.trim())).collect();
                    cells.resize(align.len(), vec![]);
                    rows.push(cells);
                    j += 1;
                }
                out.push(Block { line: l.no, kind: BlockKind::Table { align, header, rows } });
                i = j;
                continue;
            }
            i = self.paragraph(lines, i, &mut out);
        }
        out
    }

    fn interrupts_table(&self, l: &str) -> bool {
        let ind = indent(l);
        ind <= 3 && {
            let s = &l[ind..];
            fence_open(s).is_some() || atx(s).is_some() || s.starts_with('>')
        }
    }

    fn paragraph(&mut self, lines: &[Line], i: usize, out: &mut Vec<Block>) -> usize {
        let first = lines[i];
        let mut j = i;
        let mut text = String::new();
        let mut setext = None;
        while j < lines.len() {
            let t = lines[j].text;
            if j > i {
                // a setext underline turns the lines so far into a heading
                let ts = t.trim();
                if indent(t) <= 3 && !ts.is_empty() && (ts.chars().all(|c| c == '=') || ts.chars().all(|c| c == '-')) {
                    setext = Some(if ts.starts_with('=') { 1 } else { 2 });
                    j += 1;
                    break;
                }
                if self.interrupts(t) {
                    break;
                }
            }
            if j > i {
                text.push('\n');
            }
            text.push_str(t.trim_start());
            j += 1;
        }
        let text = text.trim_end_matches([' ', '\n']).to_string();
        match setext {
            Some(level) => {
                let inlines = self.inl(&text.replace('\n', " "));
                let anchor = self.anchors.make(&plain_text(&inlines));
                out.push(Block { line: first.no, kind: BlockKind::Heading { level, inlines, anchor } });
            }
            None => {
                let inlines = self.inl(&text);
                if !inlines.is_empty() {
                    out.push(Block { line: first.no, kind: BlockKind::Paragraph(inlines) });
                }
            }
        }
        j
    }

    fn quote(&mut self, lines: &[Line], i: usize, out: &mut Vec<Block>) -> usize {
        let mut inner: Vec<Line> = vec![];
        let mut j = i;
        while j < lines.len() {
            let t = lines[j].text;
            let ind = indent(t);
            if ind <= 3 && t[ind..].starts_with('>') {
                let rest = &t[ind + 1..];
                inner.push(Line { text: rest.strip_prefix(' ').unwrap_or(rest), no: lines[j].no });
            } else if !blank(t) && inner.last().is_some_and(|p| !blank(p.text) && !is_hr(p.text) && fence_open(p.text.trim_start()).is_none()) && !self.interrupts(t) {
                // lazy continuation of a paragraph
                inner.push(Line { text: t.trim_start(), no: lines[j].no });
            } else {
                break;
            }
            j += 1;
        }
        // GitHub alerts: > [!NOTE]
        let mut kind = None;
        if let Some(first) = inner.first() {
            let f = first.text.trim();
            if let Some(k) = f.strip_prefix("[!").and_then(|r| r.strip_suffix(']')).map(str::to_ascii_lowercase)
                && matches!(k.as_str(), "note" | "tip" | "important" | "warning" | "caution")
            {
                kind = Some(k);
                inner.remove(0);
            }
        }
        let blocks = self.blocks(&inner);
        out.push(Block { line: lines[i].no, kind: match kind {
            Some(kind) => BlockKind::Admonition { kind, title: String::new(), blocks },
            None => BlockKind::Quote(blocks),
        } });
        j
    }

    fn list(&mut self, lines: &[Line], start: usize, first: Marker, out: &mut Vec<Block>) -> usize {
        let mut items: Vec<ListItem> = vec![];
        let mut i = start;
        let mut marker = first;
        loop {
            let l = lines[i];
            let base = indent(l.text);
            let content_indent = base + marker.width;
            let mut inner: Vec<Line> = vec![Line { text: &l.text[base + marker.content_at..], no: l.no }];
            // an item that starts with a blank has its content on the next lines
            let mut j = i + 1;
            while j < lines.len() {
                let t = lines[j];
                if blank(t.text) {
                    // blank lines belong to the item when more of it follows
                    let mut k = j;
                    while k < lines.len() && blank(lines[k].text) {
                        k += 1;
                    }
                    if k < lines.len() && indent(lines[k].text) >= content_indent && inner.iter().any(|x| !blank(x.text)) {
                        for b in &lines[j..k] {
                            inner.push(Line { text: "", no: b.no });
                        }
                        j = k;
                        continue;
                    }
                    break;
                }
                if indent(t.text) >= content_indent {
                    inner.push(Line { text: strip_indent(t.text, content_indent), no: t.no });
                } else if inner.last().is_some_and(|p| !blank(p.text)) && list_marker(t.text.trim_start()).is_none() && !self.interrupts(t.text) && indent(t.text) < content_indent {
                    // lazy continuation
                    inner.push(Line { text: t.text.trim_start(), no: t.no });
                } else {
                    break;
                }
                j += 1;
            }
            // a task list item
            let mut checked = None;
            if let Some(f) = inner.first_mut() {
                for (pre, v) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
                    if let Some(rest) = f.text.strip_prefix(pre) {
                        checked = Some(v);
                        f.text = rest;
                        break;
                    }
                }
            }
            let blocks = self.blocks(&inner);
            items.push(ListItem { checked, blocks });
            // the next item of the same list?
            let mut k = j;
            while k < lines.len() && blank(lines[k].text) {
                k += 1;
            }
            let next = (k < lines.len() && indent(lines[k].text) <= 3 && indent(lines[k].text) < content_indent + 4)
                .then(|| list_marker(&lines[k].text[indent(lines[k].text)..]))
                .flatten()
                .filter(|m| m.ordered == marker.ordered && m.kind == marker.kind && !is_hr(&lines[k].text[indent(lines[k].text)..]));
            match next {
                Some(m) => {
                    marker = m;
                    i = k;
                }
                None => {
                    i = j;
                    break;
                }
            }
        }
        out.push(Block { line: lines[start].no, kind: BlockKind::List { ordered: marker.ordered, start: marker_start(lines[start].text), items } });
        i
    }

    fn html(&mut self, lines: &[Line], i: usize, out: &mut Vec<Block>) -> usize {
        let mut j = i;
        let mut raw = String::new();
        let comment = lines[i].text.trim_start().starts_with("<!--");
        while j < lines.len() {
            let t = lines[j].text;
            if blank(t) && !comment {
                break;
            }
            raw.push_str(t);
            raw.push('\n');
            j += 1;
            if comment && t.contains("-->") {
                break;
            }
        }
        for b in self.html_blocks(&raw, lines[i].no) {
            out.push(b);
        }
        j
    }

    /// The HTML of a README (centred logos, headings, rules, lists of links) as blocks.
    fn html_blocks(&mut self, raw: &str, line: usize) -> Vec<Block> {
        let chars: Vec<char> = raw.chars().collect();
        let mut blocks = vec![];
        let mut cur: Vec<Inline> = vec![];
        let mut heading: Option<u8> = None;
        let mut i = 0;
        let mut text = String::new();
        let mut in_pre = false;
        let flush_text = |text: &mut String, cur: &mut Vec<Inline>| {
            if !text.is_empty() {
                let t = html::decode_entities(&std::mem::take(text));
                let collapsed: String = {
                    let mut s = String::new();
                    let mut prev_ws = false;
                    for c in t.chars() {
                        if c.is_whitespace() {
                            if !prev_ws {
                                s.push(' ');
                            }
                            prev_ws = true;
                        } else {
                            s.push(c);
                            prev_ws = false;
                        }
                    }
                    s
                };
                cur.push(Inline::Text(collapsed));
            }
        };
        macro_rules! end_para {
            () => {
                flush_text(&mut text, &mut cur);
                let mut inl = crate::model::normalize(std::mem::take(&mut cur));
                if let Some(Inline::Text(t)) = inl.first_mut() {
                    *t = t.trim_start().to_string();
                }
                if let Some(Inline::Text(t)) = inl.last_mut() {
                    *t = t.trim_end().to_string();
                }
                let inl = crate::model::normalize(inl);
                if !inl.is_empty() {
                    match heading.take() {
                        Some(level) => {
                            let anchor = self.anchors.make(&plain_text(&inl));
                            blocks.push(Block { line, kind: BlockKind::Heading { level, inlines: inl, anchor } });
                        }
                        None => blocks.push(Block { line, kind: BlockKind::Paragraph(inl) }),
                    }
                }
            };
        }
        while i < chars.len() {
            if chars[i] == '<'
                && let Some(tag) = html::parse_tag(&chars[i..])
            {
                let name = tag.name.as_str();
                if in_pre {
                    if name == "pre" && tag.closing {
                        in_pre = false;
                        blocks.push(Block { line, kind: BlockKind::Code { lang: String::new(), text: html::decode_entities(text.trim_matches('\n')) } });
                        text.clear();
                    } else if name != "code" {
                        text.push_str(&chars[i..i + tag.len].iter().collect::<String>());
                    }
                    i += tag.len;
                    continue;
                }
                match name {
                    "" => {}
                    "pre" if !tag.closing => {
                        end_para!();
                        in_pre = true;
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        end_para!();
                        if !tag.closing {
                            heading = Some(name[1..].parse().unwrap_or(1));
                        }
                    }
                    "p" | "div" | "details" | "summary" | "table" | "tr" | "ul" | "ol" | "blockquote" | "center" | "picture" | "section" => {
                        end_para!();
                    }
                    "li" => {
                        end_para!();
                        if !tag.closing {
                            text.push_str("\u{2022} ");
                        }
                    }
                    "td" | "th" => {
                        if !tag.closing {
                            flush_text(&mut text, &mut cur);
                            if !cur.is_empty() {
                                cur.push(Inline::Text("  ".into()));
                            }
                        }
                    }
                    "hr" => {
                        end_para!();
                        blocks.push(Block { line, kind: BlockKind::Rule });
                    }
                    "br" => {
                        flush_text(&mut text, &mut cur);
                        cur.push(Inline::HardBreak);
                    }
                    "img" | "a" | "b" | "strong" | "i" | "em" | "code" | "kbd" | "del" | "s" | "strike" | "u" | "mark" | "small" | "sub" | "sup" | "span" | "font" => {
                        if tag.closing {
                            i += tag.len;
                            continue;
                        }
                        flush_text(&mut text, &mut cur);
                        let mut consumed = tag.len;
                        match name {
                            "img" => {
                                if let Some(src) = tag.attr("src") {
                                    cur.push(Inline::Image { src: src.to_string(), alt: tag.attr("alt").unwrap_or("").to_string(), title: String::new() });
                                }
                            }
                            "a" | "b" | "strong" | "i" | "em" | "code" | "kbd" | "del" | "s" | "strike" => {
                                // find the closing tag and take everything between as inlines (tags inside included)
                                let close = format!("</{name}");
                                let hay: String = chars[i + tag.len..].iter().collect();
                                let lower = hay.to_ascii_lowercase();
                                if let Some(e) = lower.find(&close) {
                                    let inner = &hay[..e];
                                    let gt = hay[e..].find('>').map(|g| e + g + 1).unwrap_or(hay.len());
                                    let kids = self.html_inlines(inner);
                                    consumed = tag.len + hay[..gt].chars().count();
                                    cur.push(match name {
                                        "a" => match tag.attr("href") {
                                            Some(h) if !h.is_empty() => Inline::Link { url: h.to_string(), title: String::new(), children: kids },
                                            _ => Inline::Emph(kids),
                                        },
                                        "b" | "strong" => Inline::Strong(kids),
                                        "i" | "em" => Inline::Emph(kids),
                                        "del" | "s" | "strike" => Inline::Strike(kids),
                                        _ => Inline::Code(crate::model::plain_text(&kids)),
                                    });
                                }
                            }
                            _ => {}
                        }
                        i += consumed;
                        continue;
                    }
                    _ => {}
                }
                i += tag.len;
                continue;
            }
            text.push(chars[i]);
            i += 1;
        }
        end_para!();
        blocks
    }

    fn html_inlines(&self, s: &str) -> Vec<Inline> {
        // inside an anchor or bold tag: text, images and nested tags
        let mut out = vec![];
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;
        let mut text = String::new();
        while i < chars.len() {
            if chars[i] == '<'
                && let Some(tag) = html::parse_tag(&chars[i..])
            {
                if !text.is_empty() {
                    out.push(Inline::Text(html::decode_entities(&std::mem::take(&mut text))));
                }
                if tag.name == "img" && let Some(src) = tag.attr("src") {
                    out.push(Inline::Image { src: src.to_string(), alt: tag.attr("alt").unwrap_or("").to_string(), title: String::new() });
                } else if tag.name == "br" {
                    out.push(Inline::HardBreak);
                }
                i += tag.len;
                continue;
            }
            text.push(chars[i]);
            i += 1;
        }
        if !text.is_empty() {
            out.push(Inline::Text(html::decode_entities(&text)));
        }
        let mut v = crate::model::normalize(out);
        if let Some(Inline::Text(t)) = v.first_mut() {
            *t = t.trim_start().to_string();
        }
        if let Some(Inline::Text(t)) = v.last_mut() {
            *t = t.trim_end().to_string();
        }
        crate::model::normalize(v)
    }
}

fn m_len(s: &str) -> usize {
    list_marker(s).map_or(0, |m| m.content_at)
}

fn marker_start(first_line: &str) -> u64 {
    list_marker(first_line.trim_start()).map_or(1, |m| m.start)
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
    fn headings_paragraphs_and_lines() {
        let d = parse("# Title\n\nSome *text*\nmore.\n\nSetext\n======\n\n## Sub ##\n");
        assert_eq!(kinds(&d), ["h", "p", "h", "h"]);
        assert_eq!(d.blocks[0].line, 0);
        assert_eq!(d.blocks[1].line, 2);
        assert_eq!(d.blocks[2].line, 5);
        let BlockKind::Heading { level, anchor, .. } = &d.blocks[3].kind else { panic!() };
        assert_eq!((*level, anchor.as_str()), (2, "sub"));
        let BlockKind::Paragraph(p) = &d.blocks[1].kind else { panic!() };
        assert_eq!(p[1], Inline::Emph(vec![Inline::Text("text".into())]));
        assert_eq!(p[2], Inline::SoftBreak);
    }

    #[test]
    fn fenced_and_indented_code() {
        let d = parse("```python title=x\ndef f():\n    pass\n```\n\n    indented\n    code\n\n~~~\nunclosed\n");
        assert_eq!(kinds(&d), ["code", "code", "code"]);
        let BlockKind::Code { lang, text } = &d.blocks[0].kind else { panic!() };
        assert_eq!((lang.as_str(), text.as_str()), ("python", "def f():\n    pass"));
        let BlockKind::Code { lang, text } = &d.blocks[1].kind else { panic!() };
        assert_eq!((lang.as_str(), text.as_str()), ("", "indented\ncode"));
        let BlockKind::Code { text, .. } = &d.blocks[2].kind else { panic!() };
        assert_eq!(text, "unclosed\n");
    }

    #[test]
    fn lists_nest_and_number() {
        let d = parse("- one\n- two\n  - nested a\n  - nested b\n- three\n\n3. x\n4. y\n");
        assert_eq!(kinds(&d), ["list", "list"]);
        let BlockKind::List { ordered, items, .. } = &d.blocks[0].kind else { panic!() };
        assert!(!ordered);
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[1].blocks[1].kind, BlockKind::List { items, .. } if items.len() == 2));
        let BlockKind::List { ordered, start, items } = &d.blocks[1].kind else { panic!() };
        assert_eq!((*ordered, *start, items.len()), (true, 3, 2));
    }

    #[test]
    fn list_items_hold_paragraphs_and_code() {
        let d = parse("1. First\n\n   second paragraph\n\n   ```sh\n   run\n   ```\n2. Next\n");
        let BlockKind::List { items, .. } = &d.blocks[0].kind else { panic!() };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].blocks.len(), 3);
        assert!(matches!(&items[0].blocks[2].kind, BlockKind::Code { lang, text } if lang == "sh" && text == "run"));
    }

    #[test]
    fn task_lists() {
        let d = parse("- [ ] todo\n- [x] done\n");
        let BlockKind::List { items, .. } = &d.blocks[0].kind else { panic!() };
        assert_eq!((items[0].checked, items[1].checked), (Some(false), Some(true)));
        assert!(matches!(&items[1].blocks[0].kind, BlockKind::Paragraph(p) if p == &vec![Inline::Text("done".into())]));
    }

    #[test]
    fn quotes_and_alerts() {
        let d = parse("> quoted\n> text\n\n> [!WARNING]\n> careful\n");
        assert_eq!(kinds(&d), ["quote", "admon"]);
        let BlockKind::Admonition { kind, blocks, .. } = &d.blocks[1].kind else { panic!() };
        assert_eq!((kind.as_str(), blocks.len()), ("warning", 1));
    }

    #[test]
    fn tables_with_alignment() {
        let d = parse("| a | b |\n|:--|--:|\n| 1 | `x|y` |\n| 2 | **z** |\n");
        let BlockKind::Table { align, header, rows } = &d.blocks[0].kind else { panic!("{:?}", d.blocks) };
        assert_eq!(align, &[Align::Left, Align::Right]);
        assert_eq!(header.len(), 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][1], vec![Inline::Strong(vec![Inline::Text("z".into())])]);
    }

    #[test]
    fn rules_and_setext_dashes() {
        let d = parse("a\n---\n\n***\n\n- - -\n");
        assert_eq!(kinds(&d), ["h", "hr", "hr"]);
    }

    #[test]
    fn reference_definitions_are_used_and_hidden() {
        let d = parse("See [docs] and [the site][site].\n\n[docs]: https://d.org \"Docs\"\n[site]: <https://s.org>\n");
        assert_eq!(kinds(&d), ["p"]);
        let BlockKind::Paragraph(p) = &d.blocks[0].kind else { panic!() };
        assert!(matches!(&p[1], Inline::Link { url, title, .. } if url == "https://d.org" && title == "Docs"));
        assert!(matches!(&p[3], Inline::Link { url, .. } if url == "https://s.org"));
    }

    #[test]
    fn html_in_readmes() {
        let d = parse("<p align=\"center\">\n  <img src=\"logo.png\" alt=\"Light\" width=\"96\">\n  <br>\n  <b>Light</b> is an <a href=\"https://x\">IDE</a>\n</p>\n\n<h2>Install</h2>\n\n<!-- hidden -->\n\ntext\n");
        assert_eq!(kinds(&d), ["p", "h", "p"]);
        let BlockKind::Paragraph(p) = &d.blocks[0].kind else { panic!() };
        assert!(matches!(&p[0], Inline::Image { src, alt, .. } if src == "logo.png" && alt == "Light"));
        assert!(p.contains(&Inline::HardBreak));
        assert!(p.iter().any(|i| matches!(i, Inline::Strong(_))));
        assert!(p.iter().any(|i| matches!(i, Inline::Link { url, .. } if url == "https://x")));
        let BlockKind::Heading { level, .. } = &d.blocks[1].kind else { panic!() };
        assert_eq!(*level, 2);
    }

    #[test]
    fn crlf_and_tabs() {
        let d = parse("# T\r\n\r\n-\tone\r\n-\ttwo\r\n");
        assert_eq!(kinds(&d), ["h", "list"]);
    }

    #[test]
    fn empty_and_blank_documents() {
        assert!(parse("").blocks.is_empty());
        assert!(parse("\n\n   \n").blocks.is_empty());
    }

    #[test]
    fn the_readme_of_this_repository_parses() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md")).unwrap();
        let d = parse(&text);
        let h = crate::model::headings(&d);
        assert!(h.iter().any(|(l, t, _, _)| *l == 1 && t == "Light"), "{h:?}");
        assert!(h.iter().any(|(_, t, a, _)| t == "Modules" && a == "modules"));
        assert!(d.blocks.iter().any(|b| matches!(&b.kind, BlockKind::Table { .. })));
        assert!(d.blocks.iter().any(|b| matches!(&b.kind, BlockKind::Code { .. })));
        // nothing of the markup syntax is left in the text
        fn no_markup(inl: &[Inline]) {
            for i in inl {
                match i {
                    Inline::Text(t) => assert!(!t.contains("**") && !t.contains("]("), "markup left in {t:?}"),
                    Inline::Emph(c) | Inline::Strong(c) | Inline::Strike(c) | Inline::Link { children: c, .. } => no_markup(c),
                    _ => {}
                }
            }
        }
        fn walk(blocks: &[Block]) {
            for b in blocks {
                match &b.kind {
                    BlockKind::Paragraph(i) | BlockKind::Heading { inlines: i, .. } => no_markup(i),
                    BlockKind::List { items, .. } => items.iter().for_each(|i| walk(&i.blocks)),
                    BlockKind::Quote(c) => walk(c),
                    _ => {}
                }
            }
        }
        walk(&d.blocks);
    }
}
