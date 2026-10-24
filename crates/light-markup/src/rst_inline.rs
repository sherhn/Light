//! reStructuredText inlines: emphasis, strong, literals, interpreted text with roles, hyperlink references (named,
//! embedded `<url>`, standalone URLs), footnote references and substitutions.

use std::collections::HashMap;

use crate::model::{Inline, normalize};

/// What the whole document defines for the inline text: named hyperlink targets and substitutions.
#[derive(Default, Debug)]
pub struct Defs {
    /// Normalized name -> URL (or `#anchor`).
    pub targets: HashMap<String, String>,
    /// Substitution name -> its replacement text (an image definition keeps its path as `image:path`).
    pub subs: HashMap<String, String>,
}

pub fn normalize_name(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

pub fn parse_inlines(src: &str, defs: &Defs) -> Vec<Inline> {
    let chars: Vec<char> = src.chars().collect();
    normalize(scan(&chars, defs))
}

fn start_ok(chars: &[char], i: usize) -> bool {
    i == 0 || chars[i - 1].is_whitespace() || "'\"([{<-/:".contains(chars[i - 1])
}

fn end_ok(chars: &[char], j: usize) -> bool {
    // `j` is the index just after the end string
    j >= chars.len() || chars[j].is_whitespace() || "'\")]}>-/:.,;!?\\".contains(chars[j])
}

fn find_str(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    let p: Vec<char> = pat.chars().collect();
    (from..chars.len().saturating_sub(p.len() - 1)).find(|&k| chars[k..k + p.len()] == p[..])
}

/// The closing run of `mark` for an opener at `i` (after `mark.len()` chars): not preceded by whitespace, followed by
/// something that may end markup, with at least one character between.
fn find_end(chars: &[char], from: usize, mark: &str) -> Option<usize> {
    let mut k = from;
    while let Some(p) = find_str(chars, k, mark) {
        let after = p + mark.chars().count();
        // a reference may follow the closing backquote with `_`
        let ends = end_ok(chars, after) || (mark == "`" && chars.get(after) == Some(&'_'));
        if p > from && !chars[p - 1].is_whitespace() && chars[p - 1] != '\\' && ends {
            return Some(p);
        }
        k = p + 1;
    }
    None
}

/// Role names that show their text as code.
fn code_role(role: &str) -> bool {
    let r = role.rsplit(':').next().unwrap_or(role);
    matches!(
        r,
        "code" | "literal" | "samp" | "kbd" | "file" | "command" | "program" | "envvar" | "option" | "func" | "function" | "class" | "meth" | "method" | "mod" | "module" | "attr" | "attribute" | "data" | "exc" | "exception" | "const" | "constant" | "obj" | "object" | "type" | "macro" | "member" | "var" | "variable" | "py" | "c" | "cpp" | "math" | "regexp" | "mimetype" | "makevar" | "manpage" | "guilabel" | "menuselection" | "token" | "keyword" | "term_code"
    )
}

fn explicit_title(text: &str) -> (String, Option<String>) {
    let t = text.trim_end();
    // Sphinx lets the space before `<` go
    if t.ends_with('>')
        && let Some(lt) = t.rfind('<')
    {
        let title = t[..lt].trim().to_string();
        let target = t[lt + 1..t.len() - 1].trim().to_string();
        if !title.is_empty() {
            return (title, Some(target));
        }
    }
    (t.to_string(), None)
}

fn role_node(role: &str, text: &str, defs: &Defs) -> Inline {
    let r = role.rsplit(':').next().unwrap_or(role);
    match r {
        "strong" => return Inline::Strong(vec![Inline::Text(text.into())]),
        "emphasis" | "title-reference" | "t" | "title" | "abbr" | "dfn" | "term" => return Inline::Emph(vec![Inline::Text(explicit_title(text).0)]),
        "ref" | "doc" | "numref" | "download" | "any" => {
            let (title, target) = explicit_title(text);
            let label = if target.is_some() { title } else { text.trim().to_string() };
            return match target.as_deref().and_then(|t| defs.targets.get(&normalize_name(t))) {
                Some(url) if url.contains("://") => Inline::Link { url: url.clone(), title: String::new(), children: vec![Inline::Emph(vec![Inline::Text(label)])] },
                _ => Inline::Emph(vec![Inline::Text(label)]),
            };
        }
        "pep" => {
            let n: String = text.trim().chars().filter(char::is_ascii_digit).collect();
            return Inline::Link { url: format!("https://peps.python.org/pep-{n:0>4}/"), title: String::new(), children: vec![Inline::Text(format!("PEP {n}"))] };
        }
        "rfc" => {
            let n: String = text.trim().chars().filter(char::is_ascii_digit).collect();
            return Inline::Link { url: format!("https://datatracker.ietf.org/doc/html/rfc{n}"), title: String::new(), children: vec![Inline::Text(format!("RFC {n}"))] };
        }
        "sub" | "subscript" | "sup" | "superscript" => return Inline::Text(text.to_string()),
        _ => {}
    }
    if code_role(role) {
        let (title, target) = explicit_title(text);
        let shown = if target.is_some() { title } else { text.trim().to_string() };
        // `~pkg.mod.func` shows `func`; `.name` and `!name` are plain
        let shown = match shown.strip_prefix('~') {
            Some(rest) => rest.rsplit('.').next().unwrap_or(rest).to_string(),
            None => shown.trim_start_matches(['.', '!']).to_string(),
        };
        return Inline::Code(shown);
    }
    Inline::Emph(vec![Inline::Text(explicit_title(text).0)])
}

fn scan(chars: &[char], defs: &Defs) -> Vec<Inline> {
    let n = chars.len();
    let mut out: Vec<Inline> = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    macro_rules! flush {
        () => {
            if !text.is_empty() {
                out.push(Inline::Text(std::mem::take(&mut text)));
            }
        };
    }
    while i < n {
        let c = chars[i];
        match c {
            '\\' if i + 1 < n => {
                // a backslash and a space vanish together
                if chars[i + 1] != ' ' && chars[i + 1] != '\n' {
                    text.push(chars[i + 1]);
                }
                i += 2;
            }
            '`' if chars.get(i + 1) == Some(&'`') && start_ok(chars, i) => {
                if let Some(e) = find_end(chars, i + 2, "``") {
                    flush!();
                    out.push(Inline::Code(chars[i + 2..e].iter().collect()));
                    i = e + 2;
                } else {
                    text.push(c);
                    i += 1;
                }
            }
            '*' if chars.get(i + 1) == Some(&'*') && start_ok(chars, i) && chars.get(i + 2).is_some_and(|c| !c.is_whitespace()) => {
                if let Some(e) = find_end(chars, i + 2, "**") {
                    flush!();
                    out.push(Inline::Strong(scan(&chars[i + 2..e], defs)));
                    i = e + 2;
                } else {
                    text.push_str("**");
                    i += 2;
                }
            }
            '*' if start_ok(chars, i) && chars.get(i + 1).is_some_and(|c| !c.is_whitespace() && *c != '*') => {
                if let Some(e) = find_end(chars, i + 1, "*") {
                    flush!();
                    out.push(Inline::Emph(scan(&chars[i + 1..e], defs)));
                    i = e + 1;
                } else {
                    text.push(c);
                    i += 1;
                }
            }
            '`' if start_ok(chars, i) && chars.get(i + 1).is_some_and(|c| !c.is_whitespace()) => {
                let Some(e) = find_end(chars, i + 1, "`") else {
                    text.push(c);
                    i += 1;
                    continue;
                };
                let body: String = chars[i + 1..e].iter().collect();
                let mut next = e + 1;
                flush!();
                // `text`:role:
                if chars.get(next) == Some(&':')
                    && let Some(close) = chars[next + 1..].iter().position(|c| *c == ':')
                    && close > 0
                    && chars[next + 1..next + 1 + close].iter().all(|c| c.is_alphanumeric() || "-_.+:".contains(*c))
                {
                    let role: String = chars[next + 1..next + 1 + close].iter().collect();
                    out.push(role_node(&role, &body, defs));
                    i = next + close + 2;
                    continue;
                }
                // `text`_ and `text`__ are references
                if chars.get(next) == Some(&'_') {
                    next += 1;
                    let anonymous = chars.get(next) == Some(&'_');
                    if anonymous {
                        next += 1;
                    }
                    let (title, target) = explicit_title(&body);
                    let url = match &target {
                        Some(t) if t.contains(':') || t.contains('/') || t.contains('.') || t.contains('@') => Some(if t.contains("://") || t.starts_with("mailto:") || t.contains('/') || t.contains('.') { t.clone() } else { format!("mailto:{t}") }),
                        Some(t) => defs.targets.get(&normalize_name(t)).cloned().or_else(|| Some(format!("#{}", crate::model::slug(t)))),
                        None => defs.targets.get(&normalize_name(&body)).cloned(),
                    };
                    let label = if target.is_some() { title } else { body.clone() };
                    match url {
                        Some(url) => out.push(Inline::Link { url, title: String::new(), children: vec![Inline::Text(label)] }),
                        None => out.push(Inline::Text(label)),
                    }
                    i = next;
                    continue;
                }
                // the default role: a title reference, shown in italics
                out.push(Inline::Emph(vec![Inline::Text(body)]));
                i = e + 1;
            }
            ':' if start_ok(chars, i) && chars.get(i + 1).is_some_and(|c| c.is_alphabetic()) => {
                // :role:`text`
                let mut k = i + 1;
                while k < n && (chars[k].is_alphanumeric() || "-_.+".contains(chars[k]) || (chars[k] == ':' && chars.get(k + 1) != Some(&'`'))) {
                    k += 1;
                }
                if k < n && chars[k] == ':' && chars.get(k + 1) == Some(&'`')
                    && let Some(e) = find_end(chars, k + 2, "`")
                {
                    flush!();
                    let role: String = chars[i + 1..k].iter().collect();
                    let body: String = chars[k + 2..e].iter().collect();
                    out.push(role_node(&role, &body, defs));
                    i = e + 1;
                    continue;
                }
                text.push(c);
                i += 1;
            }
            '|' if start_ok(chars, i) && chars.get(i + 1).is_some_and(|c| !c.is_whitespace() && *c != '|') => {
                if let Some(e) = find_end(chars, i + 1, "|") {
                    let name: String = chars[i + 1..e].iter().collect();
                    if let Some(rep) = defs.subs.get(&normalize_name(&name)) {
                        flush!();
                        match rep.strip_prefix("image:") {
                            Some(src) => out.push(Inline::Image { src: src.to_string(), alt: name, title: String::new() }),
                            None => out.push(Inline::Text(rep.clone())),
                        }
                        let mut next = e + 1;
                        while chars.get(next) == Some(&'_') {
                            next += 1;
                        }
                        i = next;
                        continue;
                    }
                }
                text.push(c);
                i += 1;
            }
            '[' if start_ok(chars, i) => {
                // footnote and citation references: [1]_ [#]_ [#name]_ [CIT2002]_
                if let Some(close) = chars[i..].iter().position(|c| *c == ']')
                    && chars.get(i + close + 1) == Some(&'_')
                    && close > 1
                    && chars[i + 1..i + close].iter().all(|c| c.is_alphanumeric() || "#*-_.".contains(*c))
                {
                    flush!();
                    let label: String = chars[i + 1..i + close].iter().collect();
                    out.push(Inline::Text(format!("[{}]", label.trim_start_matches('#'))));
                    i += close + 2;
                } else {
                    text.push(c);
                    i += 1;
                }
            }
            'h' if (matches_at(chars, i, "http://") || matches_at(chars, i, "https://") || matches_at(chars, i, "ftp://")) && start_ok(chars, i) => {
                let mut j = i;
                while j < n && !chars[j].is_whitespace() && !"<>".contains(chars[j]) {
                    j += 1;
                }
                while j > i + 8 && ".,:;!?)'\"]".contains(chars[j - 1]) {
                    j -= 1;
                }
                flush!();
                let url: String = chars[i..j].iter().collect();
                out.push(Inline::Link { url: url.clone(), title: String::new(), children: vec![Inline::Text(url)] });
                i = j;
            }
            '\n' => {
                let hard = false;
                flush!();
                out.push(if hard { Inline::HardBreak } else { Inline::SoftBreak });
                i += 1;
            }
            c if (c.is_alphanumeric() || c == '_') && start_ok(chars, i) => {
                // word_ : a reference to a named target
                let mut j = i;
                while j < n && (chars[j].is_alphanumeric() || (j > i && "-_.:+".contains(chars[j]) && chars.get(j + 1).is_some_and(|x| x.is_alphanumeric() || *x == '_'))) {
                    j += 1;
                }
                let mut handled = false;
                if j > i && chars.get(j) == Some(&'_') && chars.get(j + 1).is_none_or(|c| !c.is_alphanumeric() && *c != '_') {
                    let name: String = chars[i..j].iter().collect();
                    if let Some(url) = defs.targets.get(&normalize_name(&name)) {
                        flush!();
                        out.push(Inline::Link { url: url.clone(), title: String::new(), children: vec![Inline::Text(name)] });
                        i = j + 1;
                        handled = true;
                    }
                }
                if !handled {
                    let end = j.max(i + 1);
                    text.extend(&chars[i..end]);
                    i = end;
                }
            }
            _ => {
                text.push(c);
                i += 1;
            }
        }
    }
    flush!();
    out
}

fn matches_at(chars: &[char], i: usize, s: &str) -> bool {
    s.chars().enumerate().all(|(k, c)| chars.get(i + k) == Some(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<Inline> {
        parse_inlines(s, &Defs::default())
    }

    fn t(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    #[test]
    fn emphasis_strong_literal() {
        assert_eq!(p("a *b* **c** ``d*e`` f"), vec![t("a "), Inline::Emph(vec![t("b")]), t(" "), Inline::Strong(vec![t("c")]), t(" "), Inline::Code("d*e".into()), t(" f")]);
        assert_eq!(p("2 * 3 and a*b"), vec![t("2 * 3 and a*b")]);
        assert_eq!(p("snake_case and *"), vec![t("snake_case and *")]);
    }

    #[test]
    fn roles() {
        assert_eq!(p(":func:`print` and :py:class:`~os.path.Path`"), vec![Inline::Code("print".into()), t(" and "), Inline::Code("Path".into())]);
        assert_eq!(p(":ref:`Title <label>`"), vec![Inline::Emph(vec![t("Title")])]);
        assert_eq!(p(":strong:`x` :pep:`8`"), vec![Inline::Strong(vec![t("x")]), t(" "), Inline::Link { url: "https://peps.python.org/pep-0008/".into(), title: String::new(), children: vec![t("PEP 8")] }]);
        assert_eq!(p("`text`:code:"), vec![Inline::Code("text".into())]);
        assert_eq!(p("`title ref`"), vec![Inline::Emph(vec![t("title ref")])]);
    }

    #[test]
    fn hyperlinks() {
        assert_eq!(p("`Light <https://x.org/a>`_"), vec![Inline::Link { url: "https://x.org/a".into(), title: String::new(), children: vec![t("Light")] }]);
        let mut d = Defs::default();
        d.targets.insert("python".into(), "https://python.org".into());
        let v = parse_inlines("see Python_ and `Python`_.", &d);
        assert!(matches!(&v[1], Inline::Link { url, .. } if url == "https://python.org"));
        assert!(matches!(&v[3], Inline::Link { url, .. } if url == "https://python.org"));
        assert_eq!(v[4], t("."));
        assert_eq!(p("go to https://x.org/a, ok"), vec![t("go to "), Inline::Link { url: "https://x.org/a".into(), title: String::new(), children: vec![t("https://x.org/a")] }, t(", ok")]);
    }

    #[test]
    fn substitutions_footnotes_escapes() {
        let mut d = Defs::default();
        d.subs.insert("version".into(), "1.2".into());
        assert_eq!(parse_inlines("v |version| [1]_ \\*x\\*", &d), vec![t("v 1.2 [1] *x*")]);
    }
}
