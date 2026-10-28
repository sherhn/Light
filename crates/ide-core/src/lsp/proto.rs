//! Reading what the server answers: places, edits, completions, hover text and problems. Each takes the JSON of the
//! answer and returns what the IDE uses; whatever a server leaves out or gets wrong is skipped, never a panic.

use serde_json::Value;

use super::pos::{Encoding, LineMap, Position, Range};

fn position(v: &Value) -> Option<Position> {
    Some(Position { line: u32::try_from(v["line"].as_u64()?).ok()?, character: u32::try_from(v["character"].as_u64()?).ok()? })
}

fn range(v: &Value) -> Option<Range> {
    Some(Range { start: position(&v["start"])?, end: position(&v["end"])? })
}

/// A place in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

/// The places of an answer to `definition`, `declaration` or `references`: a `Location`, a list of them, a list of
/// `LocationLink`s (whose target is the place), or nothing.
pub fn locations(v: &Value) -> Vec<Location> {
    let one = |v: &Value| -> Option<Location> {
        match v.get("targetUri") {
            Some(uri) => Some(Location { uri: uri.as_str()?.to_string(), range: range(&v["targetSelectionRange"]).or_else(|| range(&v["targetRange"]))? }),
            None => Some(Location { uri: v["uri"].as_str()?.to_string(), range: range(&v["range"])? }),
        }
    };
    match v {
        Value::Array(a) => a.iter().filter_map(one).collect(),
        Value::Object(_) => one(v).into_iter().collect(),
        _ => vec![],
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/// A list of `TextEdit`s (the answer to `formatting`).
pub fn text_edits(v: &Value) -> Vec<TextEdit> {
    v.as_array().map_or_else(Vec::new, |a| a.iter().filter_map(|e| Some(TextEdit { range: range(&e["range"])?, new_text: e["newText"].as_str()?.to_string() })).collect())
}

/// The edits of a `WorkspaceEdit` (the answer to `rename`), file by file. Creating, renaming or deleting files is
/// not something the IDE does for a server: such an edit is refused as a whole.
pub fn workspace_edit(v: &Value) -> Result<Vec<(String, Vec<TextEdit>)>, String> {
    let mut out: Vec<(String, Vec<TextEdit>)> = vec![];
    if let Some(changes) = v.get("documentChanges").and_then(Value::as_array) {
        for c in changes {
            if c.get("kind").is_some() {
                return Err(format!("the change would {} a file", c["kind"].as_str().unwrap_or("change")));
            }
            let Some(uri) = c["textDocument"]["uri"].as_str() else { continue };
            out.push((uri.to_string(), text_edits(&c["edits"])));
        }
    } else if let Some(changes) = v.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            out.push((uri.clone(), text_edits(edits)));
        }
    }
    out.retain(|(_, e)| !e.is_empty());
    Ok(out)
}

/// The byte ranges of `edits` in `text` with their new text, in order. Edits that overlap make no sense together.
pub fn edit_ranges(text: &str, edits: &[TextEdit], enc: Encoding) -> Result<Vec<(usize, usize, String)>, String> {
    let map = LineMap::new(text);
    let mut out: Vec<(usize, usize, String)> = edits.iter().map(|e| {
        let (s, end) = map.range(e.range, enc);
        (s, end, e.new_text.clone())
    }).collect();
    // stable: inserts at one place stay in the order the server gave them
    out.sort_by_key(|(s, e, _)| (*s, *e));
    if out.windows(2).any(|w| w[0].1 > w[1].0) {
        return Err("the edits overlap".into());
    }
    Ok(out)
}

/// `text` with `edits` made.
pub fn apply(text: &str, edits: &[TextEdit], enc: Encoding) -> Result<String, String> {
    let mut out = text.to_string();
    for (s, e, new) in edit_ranges(text, edits, enc)?.into_iter().rev() {
        out.replace_range(s..e, &new);
    }
    Ok(out)
}

/// A completion the server offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    /// What the list shows.
    pub label: String,
    /// What goes into the text.
    pub insert: String,
    /// What the typed prefix is matched against.
    pub filter: String,
    /// What the server sorts by.
    pub sort: String,
    /// `CompletionItemKind`: 2 method, 3 function, 6 variable, 7 class, 14 keyword...
    pub kind: u64,
    pub detail: String,
}

/// A snippet as plain text: `${1:name}` is `name`, `$1` and `$0` are nothing, `\$` is `$`.
fn snippet_text(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '$' if chars.peek().is_some_and(char::is_ascii_digit) => {
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
            }
            '$' if chars.peek() == Some(&'{') => {
                chars.next();
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
                if chars.peek() == Some(&':') {
                    chars.next();
                }
                // the placeholder's text, which may hold placeholders of its own
                let mut inner = String::new();
                let mut depth = 1;
                for c in chars.by_ref() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    inner.push(c);
                }
                out.push_str(&snippet_text(&inner));
            }
            _ => out.push(c),
        }
    }
    out
}

/// The items of an answer to `completion` (a `CompletionList` or a plain list) and whether the list is incomplete
/// (typing more must ask again rather than narrow it down).
pub fn completion(v: &Value) -> (Vec<CompletionItem>, bool) {
    let (items, incomplete) = match v {
        Value::Array(a) => (a.as_slice(), false),
        Value::Object(_) => (v["items"].as_array().map_or(&[][..], Vec::as_slice), v["isIncomplete"].as_bool().unwrap_or(false)),
        _ => (&[][..], false),
    };
    let out = items
        .iter()
        .filter_map(|i| {
            let label = i["label"].as_str()?.trim().to_string();
            let edit = &i["textEdit"];
            let raw = edit["newText"].as_str().or_else(|| i["insertText"].as_str()).unwrap_or(&label);
            let insert = if i["insertTextFormat"].as_u64() == Some(2) { snippet_text(raw) } else { raw.to_string() };
            let insert = insert.trim().to_string();
            if insert.is_empty() {
                return None;
            }
            Some(CompletionItem {
                filter: i["filterText"].as_str().map_or_else(|| insert.clone(), str::to_string),
                sort: i["sortText"].as_str().map_or_else(|| label.clone(), str::to_string),
                kind: i["kind"].as_u64().unwrap_or(1),
                detail: i["detail"].as_str().unwrap_or("").trim().to_string(),
                label,
                insert,
            })
        })
        .collect();
    (out, incomplete)
}

/// The text of an answer to `hover`, as Markdown: `MarkupContent`, a `MarkedString` or a list of them. `None` when
/// the server has nothing to say.
pub fn hover_text(v: &Value) -> Option<String> {
    fn marked(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => {
                let value = o.get("value")?.as_str()?;
                match o.get("language").and_then(Value::as_str) {
                    Some(lang) => Some(format!("```{lang}\n{value}\n```")),
                    None => Some(value.to_string()),
                }
            }
            _ => None,
        }
    }
    let c = &v["contents"];
    let text = match c {
        Value::Array(a) => a.iter().filter_map(marked).collect::<Vec<_>>().join("\n\n"),
        _ => marked(c)?,
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Hover text taken apart for the documentation popup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HoverParts {
    /// `function`, `variable`... when the text says (`### function \`add\``).
    pub kind: String,
    /// The first block of code: the declaration.
    pub signature: String,
    /// The rest, as plain text.
    pub doc: String,
}

/// Markdown as the popup shows it: no heading marks, rules, code ticks or hard-break spaces.
fn plain_markdown(line: &str) -> String {
    let l = line.trim_end();
    let l = match l.trim_start_matches('#') {
        rest if rest.len() < l.len() && rest.starts_with(' ') => rest.trim_start(),
        _ => l,
    };
    l.replace("\\_", "_").replace("\\*", "*").replace('`', "")
}

/// Take hover Markdown apart: clangd writes `### function \`add\``, then what it returns, its parameters and its
/// documentation, and the declaration in a code block; other servers put the code block first.
pub fn hover_parts(markdown: &str) -> HoverParts {
    let mut parts = HoverParts::default();
    let mut doc: Vec<String> = vec![];
    let mut code: Option<Vec<&str>> = None;
    let mut first = true;
    for line in markdown.lines() {
        if let Some(block) = code.as_mut() {
            if line.trim_start().starts_with("```") {
                if parts.signature.is_empty() {
                    // clangd says where the name lives in a comment: `// In main`
                    parts.signature = block.iter().filter(|l| !l.trim_start().starts_with("// In ")).copied().collect::<Vec<_>>().join("\n").trim().to_string();
                } else {
                    doc.extend(block.iter().map(|l| l.to_string()));
                }
                code = None;
            } else {
                block.push(line);
            }
            continue;
        }
        if line.trim_start().starts_with("```") {
            code = Some(vec![]);
            continue;
        }
        let t = line.trim();
        if first && t.starts_with("### ") {
            parts.kind = t[4..].split('`').next().unwrap_or("").trim().to_string();
            first = false;
            continue;
        }
        if !t.is_empty() {
            first = false;
        }
        if t.chars().all(|c| c == '-' || c == '*' || c == '_') && t.len() >= 3 {
            doc.push(String::new());
            continue;
        }
        doc.push(plain_markdown(line));
    }
    // blank lines: none at the ends, never two in a row
    let mut out: Vec<String> = vec![];
    for l in doc {
        if l.is_empty() && out.last().is_none_or(String::is_empty) {
            continue;
        }
        out.push(l);
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    parts.doc = out.join("\n");
    parts
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range,
    /// 1 error, 2 warning, 3 information, 4 hint.
    pub severity: u64,
    pub message: String,
}

/// The parameters of `textDocument/publishDiagnostics`: the file, the version of its text the problems are about
/// (if the server says) and the problems.
pub fn diagnostics(params: &Value) -> Option<(String, Option<i64>, Vec<Diagnostic>)> {
    let uri = params["uri"].as_str()?.to_string();
    let list = params["diagnostics"].as_array().map_or_else(Vec::new, |a| {
        a.iter().filter_map(|d| Some(Diagnostic { range: range(&d["range"])?, severity: d["severity"].as_u64().unwrap_or(1), message: d["message"].as_str()?.trim().to_string() })).collect()
    });
    Some((uri, params["version"].as_i64(), list))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn r(l1: u32, c1: u32, l2: u32, c2: u32) -> Range {
        Range { start: Position { line: l1, character: c1 }, end: Position { line: l2, character: c2 } }
    }

    #[test]
    fn locations_come_in_three_shapes() {
        let loc = json!({"uri": "file:///a.c", "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 7}}});
        assert_eq!(locations(&loc), vec![Location { uri: "file:///a.c".into(), range: r(1, 4, 1, 7) }]);
        assert_eq!(locations(&json!([loc, loc])).len(), 2);
        let link = json!([{"targetUri": "file:///b.c", "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 3, "character": 1}}, "targetSelectionRange": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 7}}}]);
        assert_eq!(locations(&link), vec![Location { uri: "file:///b.c".into(), range: r(0, 4, 0, 7) }]);
        assert!(locations(&Value::Null).is_empty());
        assert!(locations(&json!([{"uri": 5}])).is_empty());
    }

    #[test]
    fn workspace_edits_by_changes_and_by_document_changes() {
        let e = |l: u32| json!({"range": {"start": {"line": l, "character": 0}, "end": {"line": l, "character": 3}}, "newText": "sum"});
        let by_map = workspace_edit(&json!({"changes": {"file:///a.c": [e(0), e(4)], "file:///b.h": [e(2)]}})).unwrap();
        assert_eq!(by_map.len(), 2);
        assert_eq!(by_map.iter().map(|(_, e)| e.len()).sum::<usize>(), 3);
        let by_docs = workspace_edit(&json!({"documentChanges": [{"textDocument": {"uri": "file:///a.c", "version": 3}, "edits": [e(1)]}]})).unwrap();
        assert_eq!(by_docs[0].0, "file:///a.c");
        assert_eq!(by_docs[0].1[0].new_text, "sum");
        assert!(workspace_edit(&json!({"documentChanges": [{"kind": "rename", "oldUri": "file:///a.c", "newUri": "file:///b.c"}]})).is_err());
    }

    #[test]
    fn edits_apply_from_the_end() {
        let text = "int add(int a);\nint x = add(1);\n";
        let edit = |range, t: &str| TextEdit { range, new_text: t.into() };
        let edits = [edit(r(1, 8, 1, 11), "sum"), edit(r(0, 4, 0, 7), "sum")];
        assert_eq!(apply(text, &edits, Encoding::Utf16).unwrap(), "int sum(int a);\nint x = sum(1);\n");
        let overlapping = [edit(r(0, 0, 0, 5), ""), edit(r(0, 3, 0, 8), "")];
        assert!(apply(text, &overlapping, Encoding::Utf16).is_err());
        // two inserts at one place keep their order
        let inserts = [edit(r(0, 0, 0, 0), "a"), edit(r(0, 0, 0, 0), "b")];
        assert!(apply(text, &inserts, Encoding::Utf8).unwrap().starts_with("abint"));
    }

    #[test]
    fn completion_items() {
        let v = json!({"isIncomplete": true, "items": [
            {"label": " add(int a, int b)", "kind": 3, "insertText": "add", "filterText": "add", "sortText": "1", "detail": "int"},
            {"label": "while", "kind": 14},
            {"label": "printf", "kind": 3, "insertTextFormat": 2, "textEdit": {"newText": "printf(${1:const char *format})$0", "range": {}}},
            {"label": "  "}
        ]});
        let (items, incomplete) = completion(&v);
        assert!(incomplete);
        assert_eq!(items.len(), 3);
        assert_eq!((items[0].label.as_str(), items[0].insert.as_str(), items[0].detail.as_str()), ("add(int a, int b)", "add", "int"));
        assert_eq!((items[1].insert.as_str(), items[1].filter.as_str(), items[1].kind), ("while", "while", 14));
        assert_eq!(items[2].insert, "printf(const char *format)");
        assert_eq!(completion(&json!([{"label": "x"}])).0.len(), 1);
        assert_eq!(snippet_text(r"a\$b$1${2:${3:x}y}$0"), "a$bxy");
    }

    #[test]
    fn hover_from_clangd() {
        let md = "### function `add`  \n\n---\n→ `int`  \nParameters:  \n- `int a`\n- `int b`\n\nAdds two numbers.\n\n---\n```c\n// In demo\nint add(int a, int b)\n```";
        let v = json!({"contents": {"kind": "markdown", "value": md}});
        let p = hover_parts(&hover_text(&v).unwrap());
        assert_eq!(p.kind, "function");
        assert_eq!(p.signature, "int add(int a, int b)");
        assert_eq!(p.doc, "→ int\nParameters:\n- int a\n- int b\n\nAdds two numbers.");
    }

    #[test]
    fn hover_from_other_servers() {
        let v = json!({"contents": [{"language": "python", "value": "def f(x: int) -> str"}, "Turns *x* into a string."]});
        let p = hover_parts(&hover_text(&v).unwrap());
        assert_eq!(p.signature, "def f(x: int) -> str");
        assert_eq!(p.doc, "Turns *x* into a string.");
        assert!(hover_text(&json!({"contents": ""})).is_none());
        assert!(hover_text(&Value::Null).is_none());
    }

    #[test]
    fn published_problems() {
        let p = json!({"uri": "file:///a.c", "version": 4, "diagnostics": [
            {"range": {"start": {"line": 2, "character": 2}, "end": {"line": 2, "character": 5}}, "severity": 1, "message": "Use of undeclared identifier 'foo'"},
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": "no severity"},
            {"message": "no range"}
        ]});
        let (uri, version, list) = diagnostics(&p).unwrap();
        assert_eq!((uri.as_str(), version), ("file:///a.c", Some(4)));
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].severity, 1);
    }
}
