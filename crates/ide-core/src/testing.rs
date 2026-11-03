//! The tests of a file as a test framework of a module sees them ([`TestDef`]): where they are, the full names the
//! framework knows them by, the command that runs one, and which passed or failed according to the output of a run.

use regex::Regex;

use crate::modules::TestDef;

/// A test or a suite of tests declared in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestItem {
    /// 0-based line of the declaration.
    pub line: usize,
    pub name: String,
    /// The names of the enclosing suites and its own, joined by the framework's separator.
    pub full: String,
    pub suite: bool,
}

impl TestItem {
    /// Can a command line carry the name? Quotes, `$`, `%`, `!` and backslashes would mean something to a shell.
    pub fn runnable(&self) -> bool {
        !self.full.contains(['"', '$', '%', '!', '`', '\\', '\n'])
    }
}

/// Which tests passed and which failed, by the names the output gives them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub passed: Vec<String>,
    pub failed: Vec<String>,
}

fn compile(pattern: &str) -> Result<Option<Regex>, String> {
    if pattern.is_empty() { Ok(None) } else { Regex::new(pattern).map(Some).map_err(|e| e.to_string()) }
}

/// The reasons a definition cannot work: a regular expression that does not compile, a test pattern without a name,
/// no command.
pub fn check(def: &TestDef) -> Result<(), String> {
    for (what, pattern) in [("files", &def.files), ("content", &def.content), ("test", &def.test), ("suite", &def.suite), ("passed", &def.passed), ("failed", &def.failed)] {
        let re = compile(pattern).map_err(|e| format!("the {what} pattern: {e}"))?;
        if let Some(re) = re
            && matches!(what, "test" | "suite" | "passed" | "failed")
            && !re.capture_names().flatten().any(|n| n.starts_with("name"))
        {
            return Err(format!("the {what} pattern has no group `name`"));
        }
    }
    if def.test.is_empty() || def.command.trim().is_empty() {
        return Err("a test pattern and a command are needed".into());
    }
    Ok(())
}

/// Does the framework want the file at `rel` (relative to the project, `/` separated) with this text? The project
/// file condition is the caller's: it knows where the files are.
pub fn wants(def: &TestDef, rel: &str, text: &str) -> bool {
    let ok = |pattern: &str, s: &str| compile(pattern).ok().flatten().is_none_or(|re| re.is_match(s));
    ok(&def.files, rel) && ok(&def.content, text)
}

/// The name in the first group called `name...` that took part in the match.
fn name_of(re: &Regex, c: &regex::Captures) -> Option<String> {
    re.capture_names().flatten().filter(|n| n.starts_with("name")).find_map(|n| c.name(n)).map(|m| m.as_str().to_string())
}

fn indent(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

/// The tests and suites of `text`, in the order they are written. A suite holds the lines after it that are indented
/// deeper than its own line.
pub fn find(def: &TestDef, text: &str) -> Vec<TestItem> {
    let (Ok(Some(test)), Ok(suite)) = (compile(&def.test), compile(&def.suite)) else { return vec![] };
    let sep = if def.separator.is_empty() { " " } else { &def.separator };
    let mut suites: Vec<(usize, String)> = Vec::new();
    let mut out = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let depth = indent(line);
        while suites.last().is_some_and(|(d, _)| *d >= depth) {
            suites.pop();
        }
        let found = suite.as_ref().and_then(|re| re.captures(line).and_then(|c| name_of(re, &c))).map(|n| (n, true)).or_else(|| test.captures(line).and_then(|c| name_of(&test, &c)).map(|n| (n, false)));
        if let Some((name, is_suite)) = found {
            let full = suites.iter().map(|(_, n)| n.as_str()).chain([name.as_str()]).collect::<Vec<_>>().join(sep);
            out.push(TestItem { line: line_no, name: name.clone(), full, suite: is_suite });
            if is_suite {
                suites.push((depth, name));
            }
        }
    }
    out
}

/// The regular expression that picks `item` out by its full name: the test itself, or every test of the suite.
pub fn selector(def: &TestDef, item: &TestItem) -> String {
    let sep = if def.separator.is_empty() { " " } else { &def.separator };
    if item.suite { format!("^{}(?:{}|$)", regex::escape(&item.full), regex::escape(sep)) } else { format!("^{}$", regex::escape(&item.full)) }
}

/// The command line that runs `item` (or the whole file when there is none). `file`, `dir` and `root` are paths as the
/// shell sees them.
pub fn command_line(def: &TestDef, item: Option<&TestItem>, file: &str, dir: &str, root: &str, windows: bool) -> String {
    let pick = |own: &String, win: &String| if windows && !win.is_empty() { win.clone() } else { own.clone() };
    let template = match item {
        Some(_) => pick(&def.command, &def.windows),
        None if !def.file_command.is_empty() => pick(&def.file_command, &def.windows_file_command),
        None => pick(&def.command, &def.windows),
    };
    let test = item.map(|i| selector(def, i)).unwrap_or_else(|| ".*".into());
    template.replace("{file}", file).replace("{dir}", dir).replace("{root}", root).replace("{test}", &test)
}

/// `text` without the colour codes of a terminal.
pub fn strip_ansi(text: &str) -> String {
    static ANSI: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("valid")).replace_all(text, "").into_owned()
}

/// What the output of a run says about the tests.
pub fn outcome(def: &TestDef, output: &str) -> Outcome {
    let output = strip_ansi(output);
    let names = |pattern: &str| -> Vec<String> {
        let Ok(Some(re)) = compile(pattern) else { return vec![] };
        re.captures_iter(&output).filter_map(|c| name_of(&re, &c)).map(|n| n.trim().to_string()).collect()
    };
    Outcome { passed: names(&def.passed), failed: names(&def.failed) }
}

/// Is `reported` (a name from the output) the name of `item`: its own name, or its full name written with the
/// output's separator?
pub fn is_named(def: &TestDef, item: &TestItem, reported: &str) -> bool {
    let sep = if def.separator.is_empty() { " " } else { &def.separator };
    let full = if def.result_separator.is_empty() { reported.to_string() } else { reported.split(def.result_separator.as_str()).map(str::trim).collect::<Vec<_>>().join(sep) };
    full == item.full || reported == item.name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vitest() -> TestDef {
        TestDef {
            name: "Vitest".into(),
            languages: vec!["javascript".into()],
            files: r"(?:^|/)__tests__/|\.(?:test|spec)\.[cm]?[jt]sx?$".into(),
            test: r#"^\s*(?:it|test)(?:\.(?:only|skip|todo|concurrent|fails))*\(\s*(?:'(?P<name>[^'\n]*)'|"(?P<name2>[^"\n]*)"|`(?P<name3>[^`\n]*)`)"#.into(),
            suite: r#"^\s*describe(?:\.(?:only|skip|concurrent))*\(\s*(?:'(?P<name>[^'\n]*)'|"(?P<name2>[^"\n]*)"|`(?P<name3>[^`\n]*)`)"#.into(),
            separator: " ".into(),
            command: r#"npx vitest run "{file}" -t "{test}""#.into(),
            file_command: r#"npx vitest run "{file}""#.into(),
            passed: r"(?m)^\s*✓ (?P<name>.+?)(?: \d+ms)?$".into(),
            failed: r"(?m)^\s*× (?P<name>.+?)(?: \d+ms)?$".into(),
            result_separator: " > ".into(),
            ..Default::default()
        }
    }

    const SPEC: &str = "import { describe, it, test } from 'vitest';\ndescribe('math', () => {\n  it('adds', () => {});\n  it.skip(\"fails here\", () => {});\n  describe(`nested (deep)`, () => {\n    test('works', () => {});\n  });\n  it('after nested', () => {});\n});\ntest('top level', () => {});\n";

    #[test]
    fn tests_and_suites_are_found_with_their_full_names() {
        let def = vitest();
        assert!(check(&def).is_ok());
        let items = find(&def, SPEC);
        let got: Vec<(usize, &str, bool)> = items.iter().map(|i| (i.line, i.full.as_str(), i.suite)).collect();
        assert_eq!(got, [(1, "math", true), (2, "math adds", false), (3, "math fails here", false), (4, "math nested (deep)", true), (5, "math nested (deep) works", false), (7, "math after nested", false), (9, "top level", false)]);
        assert!(wants(&def, "src/math.test.js", SPEC) && wants(&def, "src/__tests__/a.js", "") && !wants(&def, "src/math.js", SPEC));
    }

    #[test]
    fn selectors_pick_a_test_or_every_test_of_a_suite() {
        let def = vitest();
        let items = find(&def, SPEC);
        assert_eq!(selector(&def, &items[2]), "^math fails here$");
        assert_eq!(selector(&def, &items[3]), r"^math nested \(deep\)(?: |$)");
        let suite = Regex::new(&selector(&def, &items[3])).unwrap();
        assert!(suite.is_match("math nested (deep) works") && suite.is_match("math nested (deep)") && !suite.is_match("math nested (deep)er"));
        assert_eq!(command_line(&def, Some(&items[1]), "/p/math.test.js", "/p", "/p", false), r#"npx vitest run "/p/math.test.js" -t "^math adds$""#);
        assert_eq!(command_line(&def, None, "C:/p/math.test.js", "C:/p", "C:/p", true), r#"npx vitest run "C:/p/math.test.js""#);
        let odd = TestItem { line: 0, name: "costs $5".into(), full: "costs $5".into(), suite: false };
        assert!(!odd.runnable() && items[1].runnable());
    }

    #[test]
    fn the_output_says_what_passed_and_failed() {
        let def = vitest();
        let output = " \x1b[31m❯\x1b[39m v/math.test.js (4 tests | 1 failed) 11ms\n   \x1b[32m✓\x1b[39m math\x1b[2m > \x1b[22madds\x1b[32m 2\x1b[2mms\x1b[22m\x1b[39m\n   × math > fails here 7ms\n     → expected 2 to be 3\n   ✓ math > nested (deep) > works 0ms\n   ✓ top level 0ms\n";
        let o = outcome(&def, output);
        assert_eq!(o.passed, ["math > adds", "math > nested (deep) > works", "top level"]);
        assert_eq!(o.failed, ["math > fails here"]);
        let items = find(&def, SPEC);
        assert!(is_named(&def, &items[5 - 1], "math > nested (deep) > works") && !is_named(&def, &items[1], "math > fails here"));
        // TAP of node:test gives the own names
        let tap = TestDef { result_separator: String::new(), ..def };
        assert!(is_named(&tap, &items[1], "adds"));
    }

    #[test]
    fn broken_definitions_say_why() {
        let base = vitest();
        assert!(check(&TestDef { test: "(".into(), ..base.clone() }).unwrap_err().contains("test pattern"));
        assert!(check(&TestDef { failed: r"^x (\w+)$".into(), ..base.clone() }).unwrap_err().contains("group `name`"));
        assert!(check(&TestDef { command: " ".into(), ..base }).unwrap_err().contains("command"));
    }
}
