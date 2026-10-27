//! Reading the output of a language's check command: where the problems are and what they say.

use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckProblem {
    /// 1-based.
    pub line: usize,
    /// 1-based column, when the output has one.
    pub col: Option<usize>,
    pub severity: Severity,
    pub message: String,
}

/// Compile a check's pattern: it must have the groups `line` and `msg`.
pub fn compile(pattern: &str) -> Result<Regex, String> {
    let re = regex::RegexBuilder::new(pattern).multi_line(true).build().map_err(|e| format!("pattern: {e}"))?;
    let names: Vec<&str> = re.capture_names().flatten().collect();
    for need in ["line", "msg"] {
        if !names.contains(&need) {
            return Err(format!("the pattern has no group named {need:?}"));
        }
    }
    Ok(re)
}

/// The problems `output` reports, in the order of the output, at most `limit` of them.
pub fn problems(re: &Regex, output: &str, limit: usize) -> Vec<CheckProblem> {
    let mut out = vec![];
    // Windows tools end their lines with CR LF
    let output = output.replace("\r\n", "\n").replace('\r', "\n");
    for c in re.captures_iter(&output) {
        // An optional, unmatched location is a file-wide diagnostic (Compose
        // validation and similar tools); anchor it to the first line.
        let Some(line) = c.name("line").map_or(Some(1), |m| m.as_str().trim().parse::<usize>().ok().filter(|l| *l > 0)) else { continue };
        let message = c.name("msg").map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if message.is_empty() {
            continue;
        }
        let col = c.name("col").and_then(|m| m.as_str().trim().parse::<usize>().ok()).filter(|c| *c > 0);
        let severity = match c.name("sev") {
            Some(s) if s.as_str().to_ascii_lowercase().contains("warn") => Severity::Warning,
            _ => Severity::Error,
        };
        out.push(CheckProblem { line, col, severity, message });
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_needs_a_line_and_a_message() {
        assert!(compile(r"^(?P<line>\d+): (?P<msg>.*)$").is_ok());
        assert!(compile(r"^(\d+): (.*)$").is_err());
        assert!(compile(r"(?P<line>\d+)").is_err());
        assert!(compile("(").is_err());
    }

    #[test]
    fn compiler_style_lines_are_read() {
        let re = compile(r"^[^:\n]+:(?P<line>\d+):(?:(?P<col>\d+):)? (?P<sev>\w+): (?P<msg>.*)$").unwrap();
        let out = "a.c:3:5: error: expected ';'\na.c:9: warning: unused variable 'x'\nnoise\n";
        let p = problems(&re, out, 10);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].line, p[0].col, p[0].severity, p[0].message.as_str()), (3, Some(5), Severity::Error, "expected ';'"));
        assert_eq!((p[1].line, p[1].col, p[1].severity), (9, None, Severity::Warning));
        assert_eq!(problems(&re, out, 1).len(), 1);
    }

    #[test]
    fn a_python_traceback_is_read() {
        let re = compile(r#"(?s)File "(?P<file>[^<"][^"]*)", line (?P<line>\d+)\n.*?(?P<sev>SyntaxError|IndentationError|TabError)[^:\n]*: (?P<msg>[^\n]*)"#).unwrap();
        let out = "Traceback (most recent call last):\n  File \"<string>\", line 1, in <module>\n    import sys;compile(...)\n  File \"x.py\", line 2\n    def f(:\n          ^\nSyntaxError: invalid syntax\n";
        let p = problems(&re, out, 5);
        assert_eq!(p, vec![CheckProblem { line: 2, col: None, severity: Severity::Error, message: "invalid syntax".into() }]);
        assert!(problems(&re, "all good\n", 5).is_empty());
        // the same with the line ends of Windows
        assert_eq!(problems(&re, &out.replace('\n', "\r\n"), 5).len(), 1);
    }
}
