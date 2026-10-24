//! Documentation of a definition, taken from its source: the signature (the definition line, continued while its
//! parentheses are open, cut at the body's brace) and the documentation written for it - a docstring under it
//! (Python) or the comment block above it (`///`, `//`, `/** */`, `#`, `;`, `--`). It is what the hover popup shows
//! for a function of a language a module brings.

use light_lang::Language;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocInfo {
    pub signature: String,
    pub doc: String,
}

const MAX_SIGNATURE: usize = 400;
const MAX_DOC_LINES: usize = 60;

/// The signature and the documentation of the definition that starts at 0-based line `def_line` of `text`.
pub fn definition_doc(text: &str, def_line: usize, lang: &Language) -> DocInfo {
    let lines: Vec<&str> = text.lines().collect();
    if def_line >= lines.len() {
        return DocInfo::default();
    }
    let (signature, end_line) = signature_at(&lines, def_line);
    let doc = docstring_after(&lines, end_line + 1).unwrap_or_else(|| comment_above(&lines, def_line, lang));
    DocInfo { signature, doc }
}

/// How many arguments the call that starts right after the name (at byte `after` of `text`) passes: `None` when the
/// name is not followed by `(` or the call is not closed within a reasonable distance.
pub fn call_arity(text: &str, after: usize) -> Option<usize> {
    let rest = text.get(after..)?.trim_start_matches([' ', '\t']);
    let body = rest.strip_prefix('(')?;
    let (mut depth, mut commas, mut segment) = (0usize, 0usize, false);
    let mut quote: Option<char> = None;
    let mut chars = body.chars().take(4000);
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == '\\' {
                chars.next();
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                segment = true;
            }
            '(' | '[' | '{' => {
                depth += 1;
                segment = true;
            }
            ')' | ']' | '}' => {
                if depth == 0 {
                    return Some(commas + usize::from(segment));
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                commas += 1;
                segment = false;
            }
            c if !c.is_whitespace() => segment = true,
            _ => {}
        }
    }
    None
}

/// The fewest and the most arguments a definition with this `signature` takes (`None`: any number), or `None` when
/// the signature has no parameter list to read (a variable, a label, a shell function called without parentheses).
pub fn signature_arity(signature: &str, name: &str) -> Option<(usize, Option<usize>)> {
    // the name itself, not a letter of `def` or of a type: whole word, then the parameter list
    let at = signature.match_indices(name).find_map(|(i, _)| {
        let before = signature[..i].chars().next_back();
        let after = &signature[i + name.len()..];
        (before.is_none_or(|c| !(c.is_alphanumeric() || c == '_')) && after.trim_start().starts_with('(')).then_some(i + name.len())
    })?;
    let body = signature[at..].trim_start().strip_prefix('(')?;
    let mut params: Vec<String> = vec![];
    let (mut depth, mut cur) = (0usize, String::new());
    let mut closed = false;
    for c in body.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' if depth == 0 => {
                closed = true;
                break;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => params.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if !closed {
        return None;
    }
    params.push(cur);
    let params: Vec<String> = params.iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    if params.len() == 1 && params[0] == "void" {
        return Some((0, Some(0)));
    }
    let (mut min, mut max) = (0usize, Some(0usize));
    for (k, p) in params.iter().enumerate() {
        let receiver = k == 0 && (p == "self" || p == "cls" || p.starts_with("self:") || p.starts_with("self =") || p == "&self" || p == "&mut self");
        if receiver || p == "/" || p == "*" {
            continue;
        }
        // *args, **kwargs, and C's `...`
        if p.starts_with('*') || p.ends_with("...") {
            max = None;
            continue;
        }
        if !p.contains('=') {
            min += 1;
        }
        max = max.map(|m| m + 1);
    }
    Some((min, max))
}

/// Can a call with `call` arguments (if known) be a call of a definition with these `limits` (if known)?
pub fn arity_fits(call: Option<usize>, limits: Option<(usize, Option<usize>)>) -> bool {
    match (call, limits) {
        (Some(n), Some((min, max))) => n >= min && max.is_none_or(|m| n <= m),
        _ => true,
    }
}

/// The definition's header and the line it ends on.
fn signature_at(lines: &[&str], def_line: usize) -> (String, usize) {
    let mut sig = String::new();
    let mut depth: i32 = 0;
    let mut end = def_line;
    for (k, l) in lines.iter().enumerate().skip(def_line).take(8) {
        let t = l.trim();
        end = k;
        let mut cut = t.len();
        for (i, c) in t.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                // the body starts here: the signature is what came before
                '{' if depth <= 0 => {
                    cut = i;
                    depth = 0;
                    break;
                }
                _ => {}
            }
        }
        let part = t[..cut].trim_end();
        if !part.is_empty() {
            if !sig.is_empty() {
                sig.push(' ');
            }
            sig.push_str(part);
        }
        if cut < t.len() || (depth <= 0 && !t.ends_with(',') && !t.ends_with('\\')) {
            break;
        }
    }
    let sig = sig.trim_end_matches([';', '{', ' ']).to_string();
    let sig = if sig.chars().count() > MAX_SIGNATURE { sig.chars().take(MAX_SIGNATURE).collect::<String>() + "..." } else { sig };
    (sig, end)
}

/// A Python-style docstring starting at the first non-blank line from `from`.
fn docstring_after(lines: &[&str], from: usize) -> Option<String> {
    let mut i = from;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    let first = lines.get(i)?.trim_start();
    let body = first.trim_start_matches(['r', 'R', 'u', 'U', 'b', 'B']);
    let quote = ["\"\"\"", "'''"].into_iter().find(|q| body.starts_with(q))?;
    let after_open = &body[3..];
    let mut collected: Vec<String> = vec![];
    if let Some(p) = after_open.find(quote) {
        collected.push(after_open[..p].to_string());
    } else {
        collected.push(after_open.to_string());
        for l in lines.iter().skip(i + 1).take(MAX_DOC_LINES) {
            if let Some(p) = l.find(quote) {
                collected.push(l[..p].to_string());
                break;
            }
            collected.push((*l).to_string());
        }
    }
    Some(dedent(&collected))
}

/// The comment block directly above line `def_line` (decorators and attributes in between are skipped).
fn comment_above(lines: &[&str], def_line: usize, lang: &Language) -> String {
    let marker = lang.line_comment().unwrap_or("");
    let block = lang.block_comment();
    let mut i = def_line;
    // decorators and attributes belong to the definition
    while i > 0 && {
        let t = lines[i - 1].trim_start();
        t.starts_with('@') || t.starts_with("#[") || t.starts_with("[[")
    } {
        i -= 1;
    }
    if i == 0 {
        return String::new();
    }
    let mut out: Vec<String> = vec![];
    let above = lines[i - 1].trim();
    // a block comment that ends right above
    if let Some((open, close)) = block
        && above.ends_with(close)
        && !close.is_empty()
    {
        let mut start = i - 1;
        while start > 0 && !lines[start].contains(open) && i - start < MAX_DOC_LINES {
            start -= 1;
        }
        if lines[start].contains(open) {
            for (k, l) in lines[start..i].iter().enumerate() {
                let mut t = l.trim();
                if k == 0 {
                    t = t.split_once(open).map_or(t, |(_, r)| r).trim_start_matches(['*', '!']);
                }
                t = t.strip_suffix(close).unwrap_or(t);
                let t = t.trim_start().trim_start_matches('*');
                out.push(t.strip_prefix(' ').unwrap_or(t).trim_end().to_string());
            }
            return clean(&out);
        }
    }
    if marker.is_empty() {
        return String::new();
    }
    let mut j = i;
    while j > 0 && out.len() < MAX_DOC_LINES {
        let t = lines[j - 1].trim_start();
        let Some(rest) = t.strip_prefix(marker) else { break };
        // doc comments: `///`, `//!`, `;;`, `##`
        let rest = rest.trim_start_matches(['/', '!', ';', '#', '-']);
        out.push(rest.strip_prefix(' ').unwrap_or(rest).trim_end().to_string());
        j -= 1;
    }
    out.reverse();
    clean(&out)
}

fn clean(lines: &[String]) -> String {
    let mut v: Vec<&str> = lines.iter().map(String::as_str).collect();
    while v.first().is_some_and(|l| l.trim().is_empty()) {
        v.remove(0);
    }
    while v.last().is_some_and(|l| l.trim().is_empty()) {
        v.pop();
    }
    v.join("\n")
}

fn dedent(lines: &[String]) -> String {
    // the first line follows the quotes; the others are indented like the code
    let min = lines.iter().skip(1).filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    let v: Vec<String> = lines.iter().enumerate().map(|(i, l)| if i == 0 { l.trim().to_string() } else { l.get(min.min(l.len())..).unwrap_or("").trim_end().to_string() }).collect();
    clean(&v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use light_lang::LangDef;

    fn lang(line_comment: &str, block: &[&str]) -> Language {
        Language::new(LangDef { id: "x".into(), name: "X".into(), line_comment: line_comment.into(), block_comment: block.iter().map(|s| s.to_string()).collect(), ..LangDef::default() }).unwrap()
    }

    #[test]
    fn python_docstrings_and_signatures() {
        let src = "import os\n\n@decorator\ndef greet(name,\n          greeting=\"hi\"):\n    \"\"\"Say hello.\n\n    Longer text.\n    \"\"\"\n    return 1\n";
        let d = definition_doc(src, 3, &lang("#", &[]));
        assert_eq!(d.signature, "def greet(name, greeting=\"hi\"):");
        assert_eq!(d.doc, "Say hello.\n\nLonger text.");
        let one = definition_doc("def f(): \n    '''One line.'''\n", 0, &lang("#", &[]));
        assert_eq!(one.doc, "One line.");
    }

    #[test]
    fn comments_above_c_functions() {
        let src = "#include <stdio.h>\n\n/**\n * Adds two numbers.\n * @param a first\n */\nint add(int a,\n        int b)\n{\n    return a + b;\n}\n";
        let d = definition_doc(src, 6, &lang("//", &["/*", "*/"]));
        assert_eq!(d.signature, "int add(int a, int b)");
        assert_eq!(d.doc, "Adds two numbers.\n@param a first");
        let line = definition_doc("// Mixes colours.\n// Slowly.\nstatic void mix(void) {\n}\n", 2, &lang("//", &["/*", "*/"]));
        assert_eq!(line.signature, "static void mix(void)");
        assert_eq!(line.doc, "Mixes colours.\nSlowly.");
    }

    #[test]
    fn hash_and_semicolon_comments() {
        let sh = definition_doc("# Build it.\nbuild() {\n  make\n}\n", 1, &lang("#", &[]));
        assert_eq!((sh.signature.as_str(), sh.doc.as_str()), ("build()", "Build it."));
        let asm = definition_doc("section .text\n; Entry point.\n; Called by the loader.\nstart:\n  mov eax, 1\n", 3, &lang(";", &[]));
        assert_eq!((asm.signature.as_str(), asm.doc.as_str()), ("start:", "Entry point.\nCalled by the loader."));
    }

    #[test]
    fn arguments_of_a_call_are_counted() {
        let t = "f(a, g(b, c), \"x,y\", [1, 2])";
        assert_eq!(call_arity(t, 1), Some(4));
        assert_eq!(call_arity("f()", 1), Some(0));
        assert_eq!(call_arity("f( )", 1), Some(0));
        assert_eq!(call_arity("f(a,)", 1), Some(1));
        assert_eq!(call_arity("f (a)", 1), Some(1));
        assert_eq!(call_arity("f(a, ')', b)", 1), Some(3));
        // not a call, or never closed
        assert_eq!(call_arity("f = 1", 1), None);
        assert_eq!(call_arity("f(a, b", 1), None);
    }

    #[test]
    fn parameters_of_a_signature_are_counted() {
        assert_eq!(signature_arity("int open(char *name)", "open"), Some((1, Some(1))));
        assert_eq!(signature_arity("int run(void)", "run"), Some((0, Some(0))));
        assert_eq!(signature_arity("int printf(const char *fmt, ...)", "printf"), Some((1, None)));
        assert_eq!(signature_arity("def area(width, height=1):", "area"), Some((1, Some(2))));
        assert_eq!(signature_arity("def method(self, a, *args, **kw):", "method"), Some((1, None)));
        assert_eq!(signature_arity("def f(a, /, b, *, c):", "f"), Some((3, Some(3))));
        assert_eq!(signature_arity("void cb(int (*fn)(int, int), int x)", "cb"), Some((2, Some(2))));
        assert_eq!(signature_arity("start:", "start"), None);
        assert_eq!(signature_arity("build()", "build"), Some((0, Some(0))));
    }

    #[test]
    fn calls_are_matched_to_definitions_by_their_arguments() {
        assert!(arity_fits(Some(1), signature_arity("int open(char *name)", "open")));
        assert!(!arity_fits(Some(2), signature_arity("int open(char *name)", "open")));
        assert!(!arity_fits(Some(0), signature_arity("def area(width, height=1):", "area")));
        assert!(arity_fits(Some(7), signature_arity("int printf(const char *fmt, ...)", "printf")));
        // when either side is unknown, nothing is ruled out
        assert!(arity_fits(None, Some((1, Some(1)))));
        assert!(arity_fits(Some(3), None));
    }

    #[test]
    fn nothing_documented() {
        let d = definition_doc("a = 1\n\ndef f():\n    return 2\n", 2, &lang("#", &[]));
        assert_eq!((d.signature.as_str(), d.doc.as_str()), ("def f():", ""));
        assert_eq!(definition_doc("x", 9, &lang("#", &[])), DocInfo::default());
    }

    #[test]
    fn rust_doc_comments_lose_their_slashes() {
        let d = definition_doc("/// Does it.\n///\n/// More.\n#[inline]\npub fn run() {}\n", 4, &lang("//", &["/*", "*/"]));
        assert_eq!((d.signature.as_str(), d.doc.as_str()), ("pub fn run()", "Does it.\n\nMore."));
    }
}
