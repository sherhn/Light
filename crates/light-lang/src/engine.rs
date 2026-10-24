//! A language ready to use: its grammar compiled, and everything the editor asks of a language - highlighting spans,
//! matching brackets, the entries of the outline, foldable regions, the indentation of a new line and the
//! continuation of lists.

use regex::{Regex, RegexBuilder};

use lisp_syntax::highlight::{Class, Span};
use lisp_syntax::outline::{Definition, SymbolKind};

use crate::def::{LangDef, RuleDef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentMode {
    Keep,
    Braces,
    Python,
    Markup,
    Tags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    None,
    Braces,
    Indent,
    Headings,
    Tags,
}

/// The class a grammar names (`"keyword"`, `"string"`, ...).
pub fn class_named(name: &str) -> Option<Class> {
    Some(match name {
        "plain" => Class::Plain,
        "comment" => Class::Comment,
        "string" => Class::String,
        "char" => Class::Char,
        "number" => Class::Number,
        "keyword" => Class::SpecialForm,
        "argument" => Class::Keyword,
        "builtin" => Class::Builtin,
        "constant" => Class::Constant,
        "variable" | "register" => Class::SpecialVar,
        "type" => Class::Type,
        "function" | "definition" | "label" => Class::Definition,
        "decorator" | "annotation" | "prefix" | "meta" => Class::Prefix,
        "preproc" | "directive" => Class::Preproc,
        "heading" => Class::Heading,
        "strong" => Class::Strong,
        "emphasis" => Class::Emphasis,
        "code" => Class::Code,
        "link" => Class::Link,
        "quote" => Class::Quote,
        "error" => Class::Error,
        _ => return None,
    })
}

fn symbol_kind(name: &str) -> Option<SymbolKind> {
    Some(match name {
        "function" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "class" => SymbolKind::Class,
        "struct" => SymbolKind::Struct,
        "type" => SymbolKind::Type,
        "variable" => SymbolKind::Variable,
        "constant" => SymbolKind::Constant,
        "macro" => SymbolKind::Macro,
        "module" | "heading" => SymbolKind::Package,
        "other" => SymbolKind::Other,
        _ => return None,
    })
}

fn build(pattern: &str) -> Result<Regex, String> {
    // `^` and `$` are the ends of lines: a grammar is written line by line
    let re = RegexBuilder::new(pattern).multi_line(true).size_limit(8 << 20).build().map_err(|e| format!("pattern {pattern:?}: {e}"))?;
    if re.is_match("") {
        return Err(format!("pattern {pattern:?} matches the empty string"));
    }
    Ok(re)
}

enum Kind {
    Scoped { re: Regex, language: Box<Language> },
    Match { re: Regex, captures: Vec<(usize, Class)>, class: Option<Class>, consume: usize },
    Region { begin: Regex, end: Option<Regex>, class: Class },
    Brackets { re: Regex },
}

/// Where a rule matched.
#[derive(Clone, Debug)]
struct Hit {
    start: usize,
    end: usize,
    /// (group, start, end) of the groups that took part, for rules with `captures`.
    groups: Vec<(usize, usize, usize)>,
}

impl Kind {
    fn find(&self, text: &str, from: usize) -> Option<Hit> {
        match self {
            Kind::Match { re, captures, consume, .. } if !captures.is_empty() || *consume > 0 => {
                let c = re.captures_at(text, from)?;
                let m = c.get(0)?;
                let groups = captures.iter().filter_map(|(g, _)| c.get(*g).map(|x| (*g, x.start(), x.end()))).collect();
                // scanning resumes after the group asked for, never before the match has begun
                let end = if *consume > 0 { c.get(*consume).map_or(m.end(), |g| g.end()).max(m.start() + 1) } else { m.end() };
                Some(Hit { start: m.start(), end, groups })
            }
            Kind::Scoped { re, .. } | Kind::Match { re, .. } | Kind::Brackets { re } => re.find_at(text, from).map(|m| Hit { start: m.start(), end: m.end(), groups: vec![] }),
            Kind::Region { begin, end, .. } => {
                let b = begin.find_at(text, from)?;
                let stop = end.as_ref().and_then(|e| e.find_at(text, b.end())).map_or(text.len(), |m| m.end());
                Some(Hit { start: b.start(), end: stop, groups: vec![] })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LevelMode {
    Length,
    FirstSeen,
    Setext,
}

struct OutlineRule {
    re: Regex,
    kind: SymbolKind,
    operator: String,
    name: usize,
    level: usize,
    mode: LevelMode,
}

/// What Enter does inside a list or a quotation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Continuation {
    /// Text for the start of the new line (indentation and the marker).
    pub insert: String,
    /// The line holds only a marker: Enter ends the list, the marker goes away and no new marker is made.
    pub end_list: bool,
}

pub struct Language {
    pub def: LangDef,
    rules: Vec<Rule>,
    pairs: Vec<(char, char)>,
    outline: Vec<OutlineRule>,
    first_line: Option<Regex>,
    indent: IndentMode,
    fold: FoldKind,
    list_re: Regex,
    quote_re: Regex,
    /// The words the grammar names (keywords, built-ins, types), with how they are coloured: the vocabulary of the language.
    words: Vec<(String, Class)>,
}

struct Rule {
    kind: Kind,
}

fn parse_mode(m: &str) -> Result<IndentMode, String> {
    Ok(match m {
        "" | "keep" => IndentMode::Keep,
        "braces" => IndentMode::Braces,
        "python" => IndentMode::Python,
        "markup" => IndentMode::Markup,
        "tags" => IndentMode::Tags,
        other => return Err(format!("unknown indent mode {other:?}")),
    })
}

fn parse_fold(m: &str) -> Result<FoldKind, String> {
    Ok(match m {
        "" | "none" => FoldKind::None,
        "braces" => FoldKind::Braces,
        "indent" => FoldKind::Indent,
        "headings" => FoldKind::Headings,
        "tags" => FoldKind::Tags,
        other => return Err(format!("unknown fold kind {other:?}")),
    })
}

fn class_of(name: &str, what: &str) -> Result<Class, String> {
    class_named(name).ok_or_else(|| format!("{what}: unknown class {name:?}"))
}

fn compile_rule(r: &RuleDef, i: usize, brackets: &[String]) -> Result<Rule, String> {
    let at = |e: String| format!("rule {}: {e}", i + 1);
    let kind = if !r.grammar.is_empty() {
        if r.pattern.is_empty() || !r.begin.is_empty() || !r.words.is_empty() || !r.captures.is_empty() || r.consume != 0 {
            return Err(at("a scoped grammar needs a `match` without begin, words, captures or consume".into()));
        }
        if !r.class.is_empty() {
            class_of(&r.class, &format!("rule {}", i + 1))?;
        }
        let language = Language::new(LangDef { id: format!("rule {}", i + 1), grammar: r.grammar.clone(), brackets: brackets.to_vec(), ..LangDef::default() }).map_err(at)?;
        Kind::Scoped { re: build(&r.pattern).map_err(at)?, language: Box::new(language) }
    } else if !r.begin.is_empty() {
        let begin = build(&r.begin).map_err(at)?;
        let end = if r.end.is_empty() { None } else { Some(RegexBuilder::new(&r.end).multi_line(true).build().map_err(|e| at(format!("end {:?}: {e}", r.end)))?) };
        Kind::Region { begin, end, class: class_of(&r.class, &format!("rule {}", i + 1))? }
    } else if !r.words.is_empty() {
        for w in &r.words {
            if w.is_empty() || !w.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') || w.starts_with('-') || w.ends_with('-') {
                return Err(at(format!("word {w:?} must be letters, digits, _ and internal hyphens (use a `match` rule for anything else)")));
            }
        }
        let mut words: Vec<&String> = r.words.iter().collect();
        // the longest first: `int` must not hide `int64_t`
        words.sort_by_key(|w| std::cmp::Reverse(w.len()));
        let alt = words.iter().map(|w| regex::escape(w)).collect::<Vec<_>>().join("|");
        let pattern = format!("{}\\b(?:{alt})\\b", if r.ignore_case { "(?i)" } else { "" });
        Kind::Match { re: build(&pattern).map_err(at)?, captures: vec![], class: Some(class_of(&r.class, &format!("rule {}", i + 1))?), consume: 0 }
    } else if !r.pattern.is_empty() {
        let re = build(&r.pattern).map_err(at)?;
        let mut captures = Vec::new();
        for (g, name) in &r.captures {
            let group: usize = g.parse().map_err(|_| at(format!("capture {g:?} is not a group number")))?;
            if group == 0 || group >= re.captures_len() {
                return Err(at(format!("the pattern has no group {group}")));
            }
            captures.push((group, class_of(name, &format!("rule {} capture {group}", i + 1))?));
        }
        captures.sort_by_key(|(g, _)| *g);
        let class = if r.class.is_empty() { None } else { Some(class_of(&r.class, &format!("rule {}", i + 1))?) };
        if captures.is_empty() && class.is_none() {
            return Err(at("a `match` rule needs a class or captures".into()));
        }
        if r.consume >= re.captures_len() {
            return Err(at(format!("the pattern has no group {} to consume up to", r.consume)));
        }
        Kind::Match { re, captures, class, consume: r.consume }
    } else {
        return Err(at("the rule has none of `match`, `words` or `begin`".into()));
    };
    Ok(Rule { kind })
}

impl Language {
    pub fn new(def: LangDef) -> Result<Language, String> {
        let mut rules = Vec::with_capacity(def.grammar.len() + 1);
        for (i, r) in def.grammar.iter().enumerate() {
            rules.push(compile_rule(r, i, &def.brackets).map_err(|e| format!("{}: {e}", def.id))?);
        }
        let mut pairs = Vec::new();
        for b in &def.brackets {
            let cs: Vec<char> = b.chars().collect();
            if cs.len() != 2 {
                return Err(format!("{}: bracket pair {b:?} must be two characters", def.id));
            }
            pairs.push((cs[0], cs[1]));
        }
        if !pairs.is_empty() {
            // brackets come last: whatever else claims them (a string, a comment) wins
            let chars: String = pairs.iter().flat_map(|(o, c)| [*o, *c]).map(|c| regex::escape(&c.to_string())).collect();
            rules.push(Rule { kind: Kind::Brackets { re: build(&format!("[{chars}]")).map_err(|e| format!("{}: {e}", def.id))? } });
        }
        let mut outline = Vec::new();
        for o in &def.outline {
            let re = build(&o.pattern).map_err(|e| format!("{}: outline {e}", def.id))?;
            let kind = symbol_kind(&o.kind).ok_or_else(|| format!("{}: unknown outline kind {:?}", def.id, o.kind))?;
            if o.name == 0 || o.name >= re.captures_len() || o.level >= re.captures_len() {
                return Err(format!("{}: outline pattern {:?} has no such group", def.id, o.pattern));
            }
            let mode = match o.level_mode.as_str() {
                "" | "length" => LevelMode::Length,
                "first_seen" => LevelMode::FirstSeen,
                "setext" => LevelMode::Setext,
                other => return Err(format!("{}: unknown level mode {other:?}", def.id)),
            };
            outline.push(OutlineRule { re, kind, operator: o.kind.clone(), name: o.name, level: o.level, mode });
        }
        let first_line = if def.first_line.is_empty() { None } else { Some(Regex::new(&def.first_line).map_err(|e| format!("{}: first_line: {e}", def.id))?) };
        let indent = parse_mode(&def.indent.mode).map_err(|e| format!("{}: {e}", def.id))?;
        let fold = parse_fold(&def.fold).map_err(|e| format!("{}: {e}", def.id))?;
        let list_re = Regex::new(r"^(\s*)((?:[-*+]|\d+[.)]|#\.))(\s+)(\[[ xX]\]\s+)?(.*)$").expect("list pattern");
        let quote_re = Regex::new(r"^(\s*>+\s?)(.*)$").expect("quote pattern");
        let mut words: Vec<(String, Class)> = Vec::new();
        for r in &def.grammar {
            if let Some(class) = class_named(&r.class) {
                for w in &r.words {
                    if !words.iter().any(|(x, _)| x == w) {
                        words.push((w.clone(), class));
                    }
                }
            }
        }
        for rule in &rules {
            if let Kind::Scoped { language, .. } = &rule.kind {
                for (word, class) in language.words() {
                    if !words.iter().any(|(w, _)| w == word) {
                        words.push((word.clone(), *class));
                    }
                }
            }
        }
        Ok(Language { def, rules, pairs, outline, first_line, indent, fold, list_re, quote_re, words })
    }

    /// Text without any support: no colours, no brackets, the indentation of the line above.
    pub fn plain() -> Language {
        Language::new(LangDef { id: "plain".into(), name: "Plain text".into(), ..LangDef::default() }).expect("plain text")
    }

    pub fn id(&self) -> &str {
        &self.def.id
    }

    pub fn name(&self) -> &str {
        &self.def.name
    }

    /// The words the grammar lists - keywords, built-in functions, types, constants - and how each is coloured.
    pub fn words(&self) -> &[(String, Class)] {
        &self.words
    }

    pub fn is_identifier_char(&self, c: char) -> bool {
        c.is_alphanumeric() || c == '_' || self.def.identifier_chars.contains(c)
    }

    /// The markup a preview of this language's files renders (`markdown`, `rst`), if it has one.
    pub fn preview(&self) -> Option<&str> {
        Some(self.def.preview.as_str()).filter(|p| !p.is_empty())
    }

    pub fn is_plain(&self) -> bool {
        self.rules.is_empty() && self.outline.is_empty()
    }

    /// Does `first` (the first line of a file) say the file is of this language?
    pub fn matches_first_line(&self, first: &str) -> bool {
        self.first_line.as_ref().is_some_and(|r| r.is_match(first))
    }

    pub fn line_comment(&self) -> Option<&str> {
        Some(self.def.line_comment.as_str()).filter(|c| !c.is_empty())
    }

    /// The markers of block comments, when the language has no line comment to toggle with.
    pub fn block_comment(&self) -> Option<(&str, &str)> {
        match self.def.block_comment.as_slice() {
            [a, b] if !a.is_empty() && !b.is_empty() => Some((a, b)),
            _ => None,
        }
    }

    /// Enter after `{` (or `(`, `[`) and a closing bracket typed first on a line change the indentation.
    pub fn braces(&self) -> bool {
        self.indent == IndentMode::Braces
    }

    pub fn indent_mode(&self) -> IndentMode {
        self.indent
    }

    pub fn indent_size(&self) -> usize {
        self.def.indent.size.clamp(1, 16)
    }

    /// Enter between matching markup tags gives the closing tag its own line.
    pub fn split_tag_pair(&self, before: &str, after: &str) -> bool {
        if self.indent != IndentMode::Tags || !before.ends_with('>') || !after.starts_with("</") {
            return false;
        }
        let opens = crate::tags::scan(before, &self.highlight(before), &self.def.void_tags);
        let closes = crate::tags::scan(after, &self.highlight(after), &self.def.void_tags);
        matches!((opens.last(), closes.first()), (Some(a), Some(b)) if !a.closing && !a.empty && a.end == before.len() && b.closing && b.start == 0 && a.name == b.name)
    }

    pub fn fold_kind(&self) -> FoldKind {
        self.fold
    }

    pub fn bracket_pairs(&self) -> &[(char, char)] {
        &self.pairs
    }

    // ------------------------------------------------------------------ highlighting

    /// Spans of `text` (sorted, not overlapping; plain text is left out).
    pub fn highlight(&self, text: &str) -> Vec<Span> {
        let mut out = Vec::new();
        let n = self.rules.len();
        if n == 0 {
            return out;
        }
        // the next place each rule matches at or after `pos`, found lazily and only again once `pos` has passed it
        let mut hit: Vec<Option<Hit>> = vec![None; n];
        let mut exhausted = vec![false; n];
        let (mut pos, mut depth) = (0usize, 0u32);
        loop {
            let mut best: Option<(usize, usize)> = None;
            for i in 0..n {
                if !exhausted[i] && hit[i].as_ref().is_none_or(|h| h.start < pos) {
                    hit[i] = self.rules[i].kind.find(text, pos);
                    exhausted[i] = hit[i].is_none();
                }
                if let Some(h) = &hit[i]
                    && best.is_none_or(|(s, _)| h.start < s)
                {
                    best = Some((h.start, i));
                }
            }
            let Some((_, i)) = best else { break };
            let h = hit[i].take().expect("best was a hit");
            let mut push = |start: usize, end: usize, class: Class, depth: u32| {
                if end > start && class != Class::Plain {
                    out.push(Span { start, end, class, depth });
                }
            };
            match &self.rules[i].kind {
                Kind::Scoped { language, .. } => {
                    for s in language.highlight(&text[h.start..h.end]) {
                        let d = if s.class == Class::Bracket {
                            let c = text[h.start + s.start..].chars().next().unwrap_or(' ');
                            if self.pairs.iter().any(|(o, _)| *o == c) {
                                let d = depth;
                                depth += 1;
                                d
                            } else {
                                depth = depth.saturating_sub(1);
                                depth
                            }
                        } else {
                            s.depth
                        };
                        push(h.start + s.start, h.start + s.end, s.class, d);
                    }
                }
                Kind::Match { captures, class, .. } => {
                    if captures.is_empty() {
                        if let Some(c) = class {
                            push(h.start, h.end, *c, 0);
                        }
                    } else {
                        let mut groups: Vec<(usize, usize, Class)> = h.groups.iter().filter_map(|(g, s, e)| captures.iter().find(|(cg, _)| cg == g).map(|(_, c)| (*s, *e, *c))).collect();
                        groups.sort_by_key(|(s, _, _)| *s);
                        let mut last = h.start;
                        for (s, e, c) in groups {
                            if s >= last {
                                push(s, e, c, 0);
                                last = e;
                            }
                        }
                    }
                }
                Kind::Region { class, .. } => push(h.start, h.end, *class, 0),
                Kind::Brackets { .. } => {
                    let c = text[h.start..].chars().next().unwrap_or(' ');
                    if self.pairs.iter().any(|(o, _)| *o == c) {
                        push(h.start, h.end, Class::Bracket, depth);
                        depth += 1;
                    } else {
                        depth = depth.saturating_sub(1);
                        push(h.start, h.end, Class::Bracket, depth);
                    }
                }
            }
            pos = h.end;
        }
        out
    }

    /// The bracket matching the one at or just before `off` (the pair as offsets), skipping strings and comments
    /// (`spans` are those of [`Language::highlight`]).
    pub fn matching_bracket(&self, text: &str, spans: &[Span], off: usize) -> Option<(usize, usize)> {
        let brackets: Vec<usize> = spans.iter().filter(|s| s.class == Class::Bracket).map(|s| s.start).collect();
        let at = |o: usize| brackets.binary_search(&o).is_ok();
        let here = if off < text.len() && at(off) {
            off
        } else if off > 0 && at(off - 1) {
            off - 1
        } else {
            return None;
        };
        let idx = brackets.binary_search(&here).ok()?;
        let ch = |p: usize| text[p..].chars().next().unwrap_or(' ');
        let c = ch(here);
        let (opens, partner) = self.pairs.iter().find_map(|(o, cl)| if *o == c { Some((true, *cl)) } else if *cl == c { Some((false, *o)) } else { None })?;
        let mut level = 0i32;
        if opens {
            for &p in &brackets[idx + 1..] {
                let x = ch(p);
                if x == c {
                    level += 1;
                } else if x == partner {
                    if level == 0 {
                        return Some((here, p));
                    }
                    level -= 1;
                }
            }
        } else {
            for &p in brackets[..idx].iter().rev() {
                let x = ch(p);
                if x == c {
                    level += 1;
                } else if x == partner {
                    if level == 0 {
                        return Some((p, here));
                    }
                    level -= 1;
                }
            }
        }
        None
    }

    // ------------------------------------------------------------------ outline and folding

    /// The entries of the Structure window: functions, classes, headings, ... in the order of the text. For headings
    /// `operator` is `h1`, `h2`, ...
    pub fn outline(&self, text: &str) -> Vec<Definition> {
        let starts = line_starts(text);
        let mut out: Vec<Definition> = Vec::new();
        if self.outline.is_empty() {
            return out;
        }
        // a `def` in a docstring, a `# heading` in a code block or a comment are not entries
        let dead: Vec<(usize, usize)> = self.highlight(text).iter().filter(|s| matches!(s.class, Class::Comment | Class::String | Class::Code)).map(|s| (s.start, s.end)).collect();
        let is_dead = |at: usize| {
            let i = dead.partition_point(|&(_, e)| e <= at);
            dead.get(i).is_some_and(|&(s, _)| s <= at)
        };
        for rule in &self.outline {
            let mut seen: Vec<String> = Vec::new();
            for c in rule.re.captures_iter(text) {
                let (Some(whole), Some(name)) = (c.get(0), c.get(rule.name)) else { continue };
                if is_dead(name.start()) {
                    continue;
                }
                let mut operator = rule.operator.clone();
                if rule.level > 0
                    && let Some(l) = c.get(rule.level)
                {
                    let level = match rule.mode {
                        LevelMode::Length => l.as_str().chars().count(),
                        LevelMode::Setext => if l.as_str().starts_with('=') { 1 } else { 2 },
                        LevelMode::FirstSeen => {
                            let key: String = l.as_str().chars().take(1).collect();
                            match seen.iter().position(|s| *s == key) {
                                Some(i) => i + 1,
                                None => {
                                    seen.push(key);
                                    seen.len()
                                }
                            }
                        }
                    };
                    operator = format!("h{level}");
                }
                // by the name, not by the start of the match: a pattern that begins with `\s*` may begin on a blank line
                let line = starts.partition_point(|&s| s <= name.start()) - 1;
                out.push(Definition { kind: rule.kind, name: name.as_str().trim().to_string(), operator, start: whole.start(), end: whole.end(), name_start: name.start(), name_end: name.end(), line });
            }
        }
        out.sort_by_key(|d| d.start);
        out.dedup_by_key(|d| d.start);
        out
    }

    /// Regions that can be folded, as (first line, last line) with the first line staying visible.
    pub fn fold_regions(&self, text: &str, spans: &[Span]) -> Vec<(usize, usize)> {
        let starts = line_starts(text);
        let line_of = |off: usize| starts.partition_point(|&s| s <= off) - 1;
        let mut regions: Vec<(usize, usize)> = Vec::new();
        match self.fold {
            FoldKind::None => {}
            FoldKind::Tags => {
                let mut stack: Vec<crate::tags::Tag> = Vec::new();
                for tag in crate::tags::scan(text, spans, &self.def.void_tags) {
                    if tag.closing {
                        if let Some(i) = stack.iter().rposition(|open| open.name == tag.name) {
                            let open = stack[i].start;
                            stack.truncate(i);
                            let (a, b) = (line_of(open), line_of(tag.end - 1));
                            if b > a {
                                regions.push((a, b));
                            }
                        }
                    } else if !tag.empty {
                        stack.push(tag);
                    }
                }
            }
            FoldKind::Braces => {
                let mut stack = Vec::new();
                for s in spans.iter().filter(|s| s.class == Class::Bracket) {
                    match text.as_bytes()[s.start] {
                        b'{' => stack.push(s.start),
                        b'}' => {
                            if let Some(o) = stack.pop() {
                                let (a, b) = (line_of(o), line_of(s.start));
                                if b > a {
                                    regions.push((a, b));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            FoldKind::Indent => {
                let width = |l: &str| l.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>();
                let mut stack: Vec<(usize, usize)> = Vec::new();
                let mut last = None::<usize>;
                for (i, l) in text.lines().enumerate() {
                    if l.trim().is_empty() {
                        continue;
                    }
                    let w = width(l);
                    while let Some(&(sw, sl)) = stack.last() {
                        if w > sw {
                            break;
                        }
                        stack.pop();
                        if let Some(e) = last.filter(|e| *e > sl) {
                            regions.push((sl, e));
                        }
                    }
                    stack.push((w, i));
                    last = Some(i);
                }
                while let Some((_, sl)) = stack.pop() {
                    if let Some(e) = last.filter(|e| *e > sl) {
                        regions.push((sl, e));
                    }
                }
            }
            FoldKind::Headings => {
                let heads: Vec<(usize, usize)> = self.outline(text).iter().filter_map(|d| d.operator.strip_prefix('h').and_then(|n| n.parse::<usize>().ok()).map(|lv| (d.line, lv))).collect();
                let last_line = text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()).map(|(i, _)| i).last().unwrap_or(0);
                for (k, &(line, level)) in heads.iter().enumerate() {
                    let next = heads[k + 1..].iter().find(|(_, lv)| *lv <= level).map(|(l, _)| *l);
                    let mut end = next.map_or(last_line, |n| n.saturating_sub(1));
                    // blank lines between sections stay outside
                    let lines: Vec<&str> = text.lines().collect();
                    while end > line && lines.get(end).is_some_and(|l| l.trim().is_empty()) {
                        end -= 1;
                    }
                    if end > line {
                        regions.push((line, end));
                    }
                }
            }
        }
        regions.sort_unstable();
        regions.dedup();
        regions
    }

    // ------------------------------------------------------------------ indentation

    /// The indentation for a new line whose text so far is `cur`, after the line `prev` (the last one that is not
    /// blank): spaces or tabs.
    pub fn indent_for(&self, prev: &str, cur: &str) -> String {
        let base: String = prev.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let unit = if base.contains('\t') { "\t".to_string() } else { " ".repeat(self.indent_size()) };
        let less = |w: &str| -> String {
            let n = w.len().saturating_sub(unit.len());
            w[..n].to_string()
        };
        let code = match self.line_comment() {
            Some(m) => prev.split(m).next().unwrap_or(prev),
            None => prev,
        }
        .trim_end();
        match self.indent {
            IndentMode::Keep | IndentMode::Markup => base,
            IndentMode::Tags => {
                let spans = self.highlight(prev);
                let mut stack: Vec<String> = Vec::new();
                for tag in crate::tags::scan(prev, &spans, &self.def.void_tags) {
                    if tag.closing {
                        if let Some(i) = stack.iter().rposition(|name| *name == tag.name) {
                            stack.truncate(i);
                        }
                    } else if !tag.empty {
                        stack.push(tag.name);
                    }
                }
                let mut w = base + &unit.repeat(stack.len());
                if cur.trim_start().starts_with("</") {
                    w = less(&w);
                }
                w
            }
            IndentMode::Braces => {
                let mut w = base;
                if code.ends_with(['{', '(', '[']) {
                    w.push_str(&unit);
                }
                if cur.trim_start().starts_with(['}', ')', ']']) {
                    w = less(&w);
                }
                w
            }
            IndentMode::Python => {
                let mut w = base;
                if code.ends_with(':') || code.ends_with(['{', '(', '[']) {
                    w.push_str(&unit);
                } else if ["return", "pass", "break", "continue", "raise"].iter().any(|k| {
                    let t = code.trim_start();
                    t == *k || t.starts_with(&format!("{k} ")) || t.starts_with(&format!("{k}("))
                }) {
                    w = less(&w);
                }
                w
            }
        }
    }

    /// Markdown and reStructuredText: what Enter at the end of `line` does in a list or a quotation.
    pub fn continue_line(&self, line: &str) -> Option<Continuation> {
        if self.indent != IndentMode::Markup {
            return None;
        }
        if let Some(c) = self.list_re.captures(line) {
            let (indent, marker, gap, task, rest) = (&c[1], &c[2], &c[3], c.get(4).map_or("", |m| m.as_str()), &c[5]);
            if rest.trim().is_empty() && task.is_empty() {
                return Some(Continuation { insert: String::new(), end_list: true });
            }
            let next = match marker.trim_end_matches(['.', ')']).parse::<u64>() {
                Ok(n) => format!("{}{}", n + 1, &marker[marker.len() - 1..]),
                Err(_) => marker.to_string(),
            };
            let task = if task.is_empty() { String::new() } else { "[ ] ".to_string() };
            return Some(Continuation { insert: format!("{indent}{next}{gap}{task}"), end_list: false });
        }
        if let Some(c) = self.quote_re.captures(line) {
            if c[2].trim().is_empty() {
                return Some(Continuation { insert: String::new(), end_list: true });
            }
            return Some(Continuation { insert: c[1].to_string(), end_list: false });
        }
        None
    }
}

/// Byte offsets where the lines of `text` start.
pub fn line_starts(text: &str) -> Vec<usize> {
    let mut v = vec![0];
    v.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    v
}
