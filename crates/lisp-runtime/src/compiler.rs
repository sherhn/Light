//! Compiling a file with the real compiler and turning its diagnostics into IDE problems.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lisp_syntax::outline::outline;
use lisp_syntax::{Dialect, parse};

use crate::discover::Implementation;
use crate::runner::{self, RunHandle, lisp_string};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    StyleWarning,
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub severity: Severity,
    pub message: String,
    /// `DEFUN FOO`, from the compiler's "in:" line.
    pub form: Option<String>,
    /// The offending sub-form as printed by the compiler.
    pub snippet: Option<String>,
    /// 1-based line/column when the compiler reports them (reader errors).
    pub line: Option<usize>,
    pub column: Option<usize>,
    /// Undefined function/variable name.
    pub symbol: Option<String>,
}

fn parse_position(s: &str) -> Option<(usize, usize)> {
    let i = s.find("line: ")?;
    let rest = &s[i + 6..];
    let line: usize = rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    let j = rest.find("column: ")?;
    let col: usize = rest[j + 8..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    Some((line, col))
}

/// Parse SBCL compiler output (`; caught WARNING:` blocks and the undefined-name summary).
pub fn parse_compiler_output(text: &str) -> Vec<Problem> {
    let mut out: Vec<Problem> = Vec::new();
    let mut form: Option<String> = None;
    let mut snippet: Vec<String> = Vec::new();
    let mut collecting_snippet = false;
    let mut cur: Option<Problem> = None;
    let mut undefined_kind: Option<bool> = None; // Some(true)=function names, Some(false)=variables

    fn flush(cur: &mut Option<Problem>, out: &mut Vec<Problem>) {
        if let Some(mut p) = cur.take() {
            p.message = p.message.trim().to_string();
            if p.severity == Severity::Error
                && let Some((l, c)) = parse_position(&p.message)
            {
                p.line = Some(l);
                p.column = Some(c);
            }
            out.push(p);
        }
    }

    for raw in text.lines() {
        // SBCL ends a message block with a bare "; "; blank lines inside a message are ";   ".
        let block_end = raw == ";" || raw == "; ";
        let line = raw.trim_end();
        if !line.starts_with(';') {
            if line.trim().is_empty() {
                flush(&mut cur, &mut out);
            }
            continue;
        }
        let body = line.trim_start_matches(';');
        let trimmed = body.trim();
        if let Some(rest) = line.strip_prefix("; in: ") {
            flush(&mut cur, &mut out);
            form = Some(rest.trim().to_string());
            snippet.clear();
            collecting_snippet = true;
            undefined_kind = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("; caught ") {
            flush(&mut cur, &mut out);
            collecting_snippet = false;
            let sev = match rest.trim_end_matches(':') {
                "ERROR" => Severity::Error,
                "WARNING" => Severity::Warning,
                "STYLE-WARNING" => Severity::StyleWarning,
                _ => Severity::Note,
            };
            cur = Some(Problem {
                severity: sev,
                message: String::new(),
                form: form.clone(),
                snippet: (!snippet.is_empty()).then(|| snippet.join(" ")),
                line: None,
                column: None,
                symbol: None,
            });
            continue;
        }
        if line.starts_with("; note:") {
            flush(&mut cur, &mut out);
            collecting_snippet = false;
            continue;
        }
        if line.starts_with("; compilation unit") || line.starts_with("; compilation finished") || line.starts_with("; compilation aborted") {
            flush(&mut cur, &mut out);
            collecting_snippet = false;
            continue;
        }
        // undefined summary: ";   Undefined functions:" then ";     A B C"
        if trimmed.starts_with("Undefined function") {
            flush(&mut cur, &mut out);
            undefined_kind = Some(true);
            continue;
        }
        if trimmed.starts_with("Undefined variable") {
            flush(&mut cur, &mut out);
            undefined_kind = Some(false);
            continue;
        }
        if let Some(is_fn) = undefined_kind {
            if trimmed.is_empty() || trimmed.starts_with("caught") || trimmed.starts_with("printed") {
                undefined_kind = None;
            } else {
                for name in trimmed.split_whitespace() {
                    out.push(Problem {
                        severity: Severity::Warning,
                        message: format!("Undefined {}: {}", if is_fn { "function" } else { "variable" }, name),
                        form: None,
                        snippet: None,
                        line: None,
                        column: None,
                        symbol: Some(name.to_string()),
                    });
                }
                continue;
            }
        }
        if let Some(p) = cur.as_mut() {
            if trimmed.is_empty() {
                if block_end && !p.message.trim().is_empty() {
                    flush(&mut cur, &mut out);
                } else {
                    p.message.push('\n');
                }
            } else if trimmed.starts_with("See also") {
                // skip documentation pointers
                flush(&mut cur, &mut out);
            } else {
                if !p.message.is_empty() {
                    p.message.push('\n');
                }
                p.message.push_str(trimmed);
            }
            continue;
        }
        if collecting_snippet {
            if trimmed.is_empty() {
                collecting_snippet = false;
            } else {
                snippet.push(trimmed.to_string());
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Location (byte range) of a problem in `src`, best effort.
pub fn locate(p: &Problem, src: &str, dialect: Dialect) -> Option<(usize, usize)> {
    if let Some(line) = p.line {
        let mut off = 0;
        for (i, l) in src.split_inclusive('\n').enumerate() {
            if i + 1 == line {
                let col = p.column.unwrap_or(0);
                let start = off + l.char_indices().nth(col).map_or(l.trim_end_matches('\n').len(), |(b, _)| b);
                let end = (start + 1).min(src.len());
                return Some((start, end.max(start)));
            }
            off += l.len();
        }
    }
    let defs = outline(src, dialect);
    // restrict to the named top-level form if known
    let scope: (usize, usize) = p
        .form
        .as_ref()
        .and_then(|f| f.split_whitespace().nth(1))
        .and_then(|name| defs.iter().find(|d| d.name.to_ascii_lowercase().split(' ').next() == Some(&name.to_ascii_lowercase())))
        .map_or((0, src.len()), |d| (d.start, d.end));
    let hay = &src[scope.0..scope.1];
    if let Some(sym) = &p.symbol {
        let r = lisp_syntax_find_symbol(hay, sym).or_else(|| lisp_syntax_find_symbol(src, sym).map(|(s, e)| (s.wrapping_sub(scope.0), e.wrapping_sub(scope.0))));
        if let Some((s, e)) = r {
            return Some((scope.0 + s, scope.0 + e));
        }
    }
    if let Some(snip) = &p.snippet
        && let Some((s, e)) = find_flexible(hay, snip)
    {
        return Some((scope.0 + s, scope.0 + e));
    }
    if p.form.is_some() && scope != (0, src.len()) {
        // fall back to the definition's name
        if let Some(d) = defs.iter().find(|d| d.start == scope.0) {
            return Some((d.name_start, d.name_end));
        }
    }
    let _ = parse(src, dialect);
    None
}

fn lisp_syntax_find_symbol(hay: &str, sym: &str) -> Option<(usize, usize)> {
    let bare = sym.rsplit(':').next().unwrap_or(sym);
    let lower = hay.to_ascii_lowercase();
    let needle = bare.to_ascii_lowercase();
    let is_sym = |c: char| !(c.is_whitespace() || "()[]{}\"';`,".contains(c));
    let mut from = 0;
    while let Some(i) = lower[from..].find(&needle) {
        let s = from + i;
        let e = s + needle.len();
        let before_ok = !hay[..s].chars().next_back().is_some_and(is_sym) || hay[..s].ends_with(':') || hay[..s].ends_with("#'");
        let after_ok = !hay[e..].chars().next().is_some_and(is_sym);
        if before_ok && after_ok {
            return Some((s, e));
        }
        from = s + 1;
    }
    None
}

/// Find `snippet` (as printed by the compiler: upper-cased, re-flowed whitespace) in `hay`.
fn find_flexible(hay: &str, snippet: &str) -> Option<(usize, usize)> {
    let toks: Vec<String> = snippet.split_whitespace().map(|t| t.to_ascii_lowercase()).collect();
    let first = toks.first()?;
    let lower = hay.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(first.as_str()) {
        let s = from + i;
        let mut pos = s + first.len();
        let mut ok = true;
        for t in &toks[1..] {
            let rest = &lower[pos..];
            let trimmed = rest.trim_start();
            if trimmed.starts_with(t.as_str()) {
                pos += rest.len() - trimmed.len() + t.len();
            } else {
                ok = false;
                break;
            }
        }
        if ok {
            return Some((s, pos));
        }
        from = s + 1;
    }
    None
}

#[derive(Clone, Debug)]
pub struct CompileResult {
    pub output: String,
    pub problems: Vec<Problem>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

impl CompileResult {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

static FASL_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn compile_file(imp: &Implementation, file: &Path) -> io::Result<(RunHandle, std::path::PathBuf)> {
    let dir = std::env::temp_dir().join("light");
    std::fs::create_dir_all(&dir)?;
    let fasl = dir.join(format!("compile-{}-{}.fasl", std::process::id(), FASL_COUNTER.fetch_add(1, Ordering::SeqCst)));
    let forms = format!(
        "(multiple-value-bind (out warn fail) (compile-file {} :output-file {}) (declare (ignore out warn)) (sb-ext:exit :code (if fail 1 0)))",
        lisp_string(&imp.lisp_path(file)),
        lisp_string(&imp.lisp_path(&fasl))
    );
    let dir_of_file = file.parent().filter(|p| !p.as_os_str().is_empty());
    let h = runner::run_eval(imp, &forms, dir_of_file)?;
    Ok((h, fasl))
}

pub fn compile_sync(imp: &Implementation, file: &Path, timeout: Duration) -> io::Result<CompileResult> {
    let (h, fasl) = compile_file(imp, file)?;
    let (output, exit_code, timed_out) = h.wait_output(timeout);
    let _ = std::fs::remove_file(&fasl);
    let problems = parse_compiler_output(&output);
    Ok(CompileResult { output, problems, exit_code, timed_out })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WARNINGS: &str = r#"; compiling file "C:/x/good2.lisp" (written 20 OCT 2026 05:56:21 PM):

; file: C:/x/good2.lisp
; in: DEFUN F
;     (+ X "a")
;
; note: deleting unreachable code
;
; caught WARNING:
;   Constant "a" conflicts with its asserted type NUMBER.
;   See also:
;     The SBCL Manual, Node "Handling of Types"

; in: DEFUN G
;     (CAR 1 2 3)
;
; caught WARNING:
;   The function CAR is called with three arguments, but wants exactly one.
;
; compilation unit finished
;   caught 2 WARNING conditions
;   printed 1 note
"#;

    const STYLE_AND_READ: &str = r#"; file: C:/x/bad.lisp
; in: DEFUN FOO
;     (UNUSED 1)
;
; caught STYLE-WARNING:
;   The variable UNUSED is defined but never used.

;
; caught ERROR:
;   READ error during COMPILE-FILE:
;·
;     end of file on #<SB-INT:FORM-TRACKING-STREAM for "file C:\\x\\bad.lisp" {1100D41243}>
;·
;     (in form starting at line: 9, column: 0, position: 121)
;
; compilation unit aborted
;   caught 1 fatal ERROR condition
"#;

    const UNDEFINED: &str = r#"; in: DEFUN BAZ
;     (UNDEFINED-FN 1 2)
;
; caught STYLE-WARNING:
;   undefined function: COMMON-LISP-USER::UNDEFINED-FN
;
; compilation unit finished
;   Undefined functions:
;     BAR UNDEFINED-FN
;   Undefined variable:
;     *UNDEFINED-VAR*
;   caught 1 STYLE-WARNING condition
"#;


    /// The editor strips trailing blanks from this source file, but SBCL's output has them:
    /// a bare `; ` ends a message block and `;   ` is a blank line inside a message (written `;·` here).
    fn sbcl_text(s: &str) -> String {
        let mut out: Vec<String> = s.lines().map(|l| if l == ";" { "; ".to_string() } else { l.replace(";·", ";   ") }).collect();
        out.push(String::new());
        out.join("
")
    }

    #[test]
    fn parses_warnings_with_form_and_snippet() {
        let ps = parse_compiler_output(&sbcl_text(WARNINGS));
        assert_eq!(ps.len(), 2, "{ps:#?}");
        assert_eq!(ps[0].severity, Severity::Warning);
        assert_eq!(ps[0].form.as_deref(), Some("DEFUN F"));
        assert_eq!(ps[0].snippet.as_deref(), Some("(+ X \"a\")"));
        assert_eq!(ps[0].message, "Constant \"a\" conflicts with its asserted type NUMBER.");
        assert_eq!(ps[1].form.as_deref(), Some("DEFUN G"));
        assert!(ps[1].message.starts_with("The function CAR is called"));
    }

    #[test]
    fn parses_style_warning_and_read_error_position() {
        let ps = parse_compiler_output(&sbcl_text(STYLE_AND_READ));
        assert_eq!(ps.len(), 2, "{ps:#?}");
        assert_eq!(ps[0].severity, Severity::StyleWarning);
        assert_eq!(ps[0].message, "The variable UNUSED is defined but never used.");
        assert_eq!(ps[1].severity, Severity::Error);
        assert_eq!((ps[1].line, ps[1].column), (Some(9), Some(0)));
        assert!(ps[1].message.starts_with("READ error"));
    }

    #[test]
    fn parses_undefined_summary() {
        let ps = parse_compiler_output(&sbcl_text(UNDEFINED));
        let syms: Vec<_> = ps.iter().filter_map(|p| p.symbol.as_deref()).collect();
        assert_eq!(syms, vec!["BAR", "UNDEFINED-FN", "*UNDEFINED-VAR*"]);
        assert!(ps.iter().any(|p| p.severity == Severity::StyleWarning && p.message.contains("undefined function")));
    }

    #[test]
    fn locates_by_line_symbol_and_snippet() {
        let src = "(defun foo (x)\n  (bar x)\n  (let ((unused 1)) x))\n\n(defun baz ()\n  (undefined-fn 1 2))\n";
        let ps = parse_compiler_output(&sbcl_text(UNDEFINED));
        let undefined = ps.iter().find(|p| p.symbol.as_deref() == Some("UNDEFINED-FN")).unwrap();
        let (s, e) = locate(undefined, src, Dialect::CommonLisp).unwrap();
        assert_eq!(&src[s..e], "undefined-fn");
        let bar = ps.iter().find(|p| p.symbol.as_deref() == Some("BAR")).unwrap();
        let (s, e) = locate(bar, src, Dialect::CommonLisp).unwrap();
        assert_eq!(&src[s..e], "bar");

        let style = &parse_compiler_output(&sbcl_text(STYLE_AND_READ))[0];
        let (s, e) = locate(style, src, Dialect::CommonLisp).unwrap();
        assert_eq!(&src[s..e].to_ascii_lowercase(), "(unused 1)");

        let read = &parse_compiler_output(&sbcl_text(STYLE_AND_READ))[1];
        let multi = "1\n2\n3\n4\n5\n6\n7\n8\n(defun broken (\n";
        let (s, _) = locate(read, multi, Dialect::CommonLisp).unwrap();
        assert_eq!(&multi[s..], "(defun broken (\n");
    }

    #[test]
    fn empty_and_garbage_output() {
        assert!(parse_compiler_output("").is_empty());
        assert!(parse_compiler_output("random text\n; weird\n;;;\n").is_empty());
    }

    fn sbcl() -> Option<Implementation> {
        crate::discover::find_sbcl("")
    }

    fn write(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("light-compile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn real_compile_clean_file_succeeds() {
        let Some(imp) = sbcl() else { return };
        let f = write("clean.lisp", "(defun add (a b) (+ a b))\n");
        let r = compile_sync(&imp, &f, Duration::from_secs(60)).unwrap();
        assert!(r.success(), "{}", r.output);
        assert!(r.problems.iter().all(|p| p.severity != Severity::Error));
    }

    #[test]
    fn real_compile_reports_and_locates_problems() {
        let Some(imp) = sbcl() else { return };
        let src = "(defun f (x)\n  (+ x \"a\"))\n\n(defun g ()\n  (missing-function 1))\n";
        let f = write("warn.lisp", src);
        let r = compile_sync(&imp, &f, Duration::from_secs(60)).unwrap();
        assert!(!r.success(), "type warning means failure-p");
        assert!(r.problems.iter().any(|p| p.severity == Severity::Warning && p.message.contains("conflicts")), "{:#?}", r.problems);
        let missing = r.problems.iter().find(|p| p.symbol.as_deref().is_some_and(|s| s.contains("MISSING-FUNCTION"))).expect("undefined function reported");
        let (s, e) = locate(missing, src, Dialect::CommonLisp).unwrap();
        assert_eq!(&src[s..e], "missing-function");
        let conflict = r.problems.iter().find(|p| p.message.contains("conflicts")).unwrap();
        let (s, e) = locate(conflict, src, Dialect::CommonLisp).unwrap();
        assert_eq!(&src[s..e], "(+ x \"a\")");
    }

    #[test]
    fn real_compile_reader_error_has_exact_position() {
        let Some(imp) = sbcl() else { return };
        let src = "(defun ok () 1)\n\n(defun broken (\n";
        let f = write("broken.lisp", src);
        let r = compile_sync(&imp, &f, Duration::from_secs(60)).unwrap();
        assert!(!r.success());
        let err = r.problems.iter().find(|p| p.severity == Severity::Error).expect("error problem");
        assert_eq!(err.line, Some(3), "{:#?}", r.problems);
        let (s, _) = locate(err, src, Dialect::CommonLisp).unwrap();
        assert_eq!(s, src.find("(defun broken").unwrap());
    }
}
