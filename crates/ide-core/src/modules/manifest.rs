//! What a module is: a folder with `module.json` (this file's [`Manifest`]), a cover picture and whatever else the
//! manifest names. A module is data - languages, how to run their files, the Linux tools they want, how to tell a
//! project needs them and templates for new projects - and the IDE does the rest. The Lisp module is the exception:
//! its engine (reader, REPL, structural editing) is part of the IDE and its manifest only switches that on.

use std::collections::BTreeMap;

use light_lang::LangDef;
use serde::{Deserialize, Serialize};

/// A command that runs a file of a language.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct RunDef {
    /// The language id this applies to.
    pub language: String,
    pub name: String,
    /// A shell command line with `{file}` (the path as the shell sees it), `{dir}` (its folder), `{name}` (file name)
    /// and `{stem}` (name without the extension). `{image}` is a Docker-safe,
    /// stable image/container name derived from the file's directory.
    pub command: String,
    /// A different command line for a project that runs on Windows (empty: the same one).
    pub windows: String,
    /// The run measures code coverage: the format of the report (see [`crate::coverage::FORMATS`]) it writes to
    /// `{report}` (in the folder `{report_dir}`, which the IDE makes for the run).
    pub coverage: String,
}

impl RunDef {
    /// The command line that runs `file`, with the placeholders filled in. `file` and `dir` are the paths as the shell
    /// that runs the line sees them (slashes, not backslashes); `windows`: the project runs on Windows.
    pub fn command_line(&self, file: &str, dir: &str, windows: bool) -> String {
        let template = if windows && !self.windows.is_empty() { &self.windows } else { &self.command };
        let name = file.rsplit('/').next().unwrap_or(file);
        let stem = name.rsplit_once('.').filter(|(s, _)| !s.is_empty()).map_or(name, |(s, _)| s);
        let folder: String = dir.rsplit('/').find(|s| !s.is_empty()).unwrap_or("project").to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
        let image = format!("light-{}-{}", if folder.is_empty() { "project" } else { &folder }, &super::sha256::sha256_hex(dir.as_bytes())[..8]);
        template.replace("{file}", file).replace("{dir}", dir).replace("{name}", name).replace("{stem}", stem).replace("{image}", &image)
    }
}

/// A command that checks a file of a language for errors without running it (a syntax check) and the pattern that
/// finds the problems in what it prints. Run on the saved file, in the project's Linux or on Windows like a run command.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct CheckDef {
    /// The language id this applies to.
    pub language: String,
    pub name: String,
    /// A shell command line, with the same placeholders as a run command's.
    pub command: String,
    /// A different command line for a project that runs on Windows (empty: the same one).
    pub windows: String,
    /// A regular expression over the whole output (`^` and `$` match at lines) with the groups `line` and `msg`, and
    /// optionally `col` and `sev` (a word with `warn` in it makes a warning; anything else an error). See
    /// [`crate::modules::check::problems`].
    pub pattern: String,
}

impl CheckDef {
    /// The command line that checks `file` (see [`RunDef::command_line`]).
    pub fn command_line(&self, file: &str, dir: &str, windows: bool) -> String {
        RunDef { language: self.language.clone(), name: self.name.clone(), command: self.command.clone(), windows: self.windows.clone(), coverage: String::new() }.command_line(file, dir, windows)
    }
}

/// Where the tasks of a project are written down - the `scripts` of a package.json, the targets of a makefile - and
/// the command that runs one. The Tasks window lists them for every file of the project with one of `files` names.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct TaskDef {
    /// What the tasks are called in the Tasks window (`npm`, `make`).
    pub name: String,
    /// The file names that hold tasks (`package.json`), compared without regard to case.
    pub files: Vec<String>,
    /// The tasks are the members of this object of a JSON file (`scripts`), or the `name`s of the objects of this
    /// array (`configurePresets` of CMakePresets.json; those marked `hidden` are not tasks)...
    pub json: String,
    /// ...or the matches of this regular expression (with a group `name`) over the file's text.
    pub pattern: String,
    /// A shell command line with `{task}` (the task's name), `{file}` and `{dir}` (the file holding the tasks and its
    /// folder, as the shell sees them) and `{root}` (the project); it runs in `{dir}`.
    pub command: String,
    /// A different command line for a project that runs on Windows (empty: the same one).
    pub windows: String,
    /// Folders whose files never hold the project's own tasks (`node_modules`): they are not looked in.
    pub skip: Vec<String>,
    /// Tasks every such file has, each with a command of its own (CMake's configure and build), listed first.
    pub fixed: Vec<FixedTask>,
}

/// A task of [`TaskDef::fixed`].
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct FixedTask {
    pub name: String,
    pub command: String,
    pub windows: String,
}

impl TaskDef {
    /// The names of the tasks that `text` (the content of one of the `files`) defines, in the order they are written.
    /// Names a command line could not carry safely (quotes, line breaks) are left out.
    pub fn tasks_in(&self, text: &str) -> Vec<String> {
        let fixed = self.fixed.iter().map(|f| f.name.clone());
        let mut names: Vec<String> = if !self.json.is_empty() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return fixed.collect() };
            if let Some(items) = value.get(&self.json).and_then(|v| v.as_array()) {
                let named = items.iter().filter(|i| i["hidden"] != true).filter_map(|i| i["name"].as_str().map(String::from));
                return dedup(fixed.chain(named).collect());
            }
            let Some(object) = value.get(&self.json).and_then(|v| v.as_object()) else { return fixed.collect() };
            // serde_json keeps the members sorted: the file says in which order they come
            let from = text.find(&format!("\"{}\"", self.json)).unwrap_or(0);
            let mut keyed: Vec<(usize, String)> = object.keys().map(|k| {
                let quoted = serde_json::to_string(k).unwrap_or_default();
                (text[from..].find(&quoted).map_or(usize::MAX, |i| from + i), k.clone())
            }).collect();
            keyed.sort();
            fixed.chain(keyed.into_iter().map(|(_, k)| k)).collect()
        } else if !self.pattern.is_empty() {
            let Ok(re) = regex::Regex::new(&self.pattern) else { return fixed.collect() };
            fixed.chain(re.captures_iter(text).filter_map(|c| c.name("name").map(|m| m.as_str().to_string()))).collect()
        } else {
            fixed.collect()
        };
        names = dedup(names);
        names
    }

    /// Is `file_name` one of the files that hold these tasks?
    pub fn holds(&self, file_name: &str) -> bool {
        self.files.iter().any(|f| f.eq_ignore_ascii_case(file_name))
    }

    /// The command line that runs `task` of the file `file` in `dir` of the project `root` (see
    /// [`RunDef::command_line`]).
    pub fn command_line(&self, task: &str, file: &str, dir: &str, root: &str, windows: bool) -> String {
        let (own, win) = match self.fixed.iter().find(|f| f.name == task) {
            Some(f) => (&f.command, &f.windows),
            None => (&self.command, &self.windows),
        };
        let template = if windows && !win.is_empty() { win } else { own };
        template.replace("{task}", task).replace("{file}", file).replace("{dir}", dir).replace("{root}", root)
    }
}

/// Names a command line can carry safely (no quotes, line breaks or shell variables), each once.
fn dedup(mut names: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| !n.trim().is_empty() && !n.contains(['"', '\n', '\r', '`', '$', '%']) && seen.insert(n.clone()));
    names
}

/// A test framework of a language: how to find its tests in a file (the editor puts a run button next to each), how to
/// run one of them, a suite or the whole file, and how to tell from the output which passed and which failed. A module
/// lists its frameworks in order; a file belongs to the first one that wants it (see [`crate::testing`]).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct TestDef {
    /// `Vitest`, `Jest`...
    pub name: String,
    /// The language ids whose files can hold these tests.
    pub languages: Vec<String>,
    /// A regular expression over the file's path relative to the project (`/` separated); empty: every file.
    pub files: String,
    /// A regular expression the file's text must match (`node:test` imported); empty: no condition.
    pub content: String,
    /// The nearest file of this name in the file's folder or above it (inside the project) is where the tests run;
    /// with `project_needs`, the framework is used only when that file contains this text (`"vitest"`).
    pub project_file: String,
    pub project_needs: String,
    /// A regular expression for the line that declares a test, with the name in a group `name` (or `name2`, `name3`:
    /// one for each kind of quotes). A group `suite` names the test's suite on the same line (`TEST(Suite, Name)`).
    pub test: String,
    /// The same for a group of tests (a suite); a suite holds the lines below it that are indented deeper.
    pub suite: String,
    /// What joins the names of the suites and the test into the full name the framework knows (` `).
    pub separator: String,
    /// Shell command lines: `command` runs one test or suite, `file_command` the whole file. Placeholders: `{file}`
    /// and `{dir}` (as a run command's), `{root}` (the folder of `project_file`, where it runs) and `{test}` (the
    /// regular expression that selects the test or suite by its full name).
    pub command: String,
    pub file_command: String,
    pub windows: String,
    pub windows_file_command: String,
    /// Environment variables for the runs (`NO_COLOR`).
    pub env: BTreeMap<String, String>,
    /// Regular expressions over the output with a group `name` for each test that passed or failed. The names are the
    /// tests' own names or their full names with `result_separator` between the parts.
    pub passed: String,
    pub failed: String,
    pub result_separator: String,
    /// Running the tests with code coverage: the report's format and the command lines (with `{report}` and
    /// `{report_dir}` as a run command's that measures coverage).
    pub coverage: String,
    pub coverage_command: String,
    pub coverage_file_command: String,
}

/// A program the IDE downloads when it is first needed (after asking), checked against its SHA-256 and unpacked.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct DownloadDef {
    /// What it is called when the IDE asks (`vscode-js-debug`).
    pub name: String,
    pub version: String,
    /// A `.tar.gz` archive.
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// Its licence and where it comes from, to say so before downloading.
    pub license: String,
    pub homepage: String,
}

/// A debugger of a language: an adapter that speaks the Debug Adapter Protocol, how to start it and the launch
/// configuration that debugs a file. The adapter listens on a TCP port; it runs where the project runs (in its Linux
/// for a project in WSL).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct DebugDef {
    /// `Node.js`.
    pub name: String,
    pub languages: Vec<String>,
    pub download: Option<DownloadDef>,
    /// The adapter's command line: `{adapter}` is where the download was unpacked, `{port}` the port to listen on.
    pub command: Vec<String>,
    /// The `adapterID` of the `initialize` request.
    pub adapter_id: String,
    /// `tcp`: the adapter listens on `{port}` (the default); `stdio`: it speaks on its standard input and output.
    pub transport: String,
    /// A shell command line that builds the program with what the debugger needs (`cc -g`) before it starts, with the
    /// placeholders of a run command; nothing is debugged when it fails.
    pub prepare: String,
    pub windows_prepare: String,
    /// The arguments of the `launch` request; `{file}`, `{dir}`, `{stem}` (the file's name without its extension),
    /// `{exe}` (`.exe` on Windows, nothing elsewhere) and `{root}` (the project) in its strings are filled in.
    pub launch: serde_json::Value,
}

/// A group of tools a module wants in the project's Linux (see the developer tools on the Linux page).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ToolDef {
    pub name: String,
    /// Programs whose presence says the group is installed.
    pub programs: Vec<String>,
    /// Debian / Ubuntu packages that bring them, separated by spaces.
    pub apt: String,
    /// The programs a project in this language cannot be built or run without; when one is missing in the project's
    /// Linux the IDE offers to install the tools. Empty: the first of `programs`.
    pub required: Vec<String>,
}

impl ToolDef {
    /// The programs a project of this language needs before anything else.
    pub fn required_programs(&self) -> Vec<String> {
        if self.required.is_empty() { self.programs.iter().take(1).cloned().collect() } else { self.required.clone() }
    }
}

/// When a project looks like it needs the module.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Detect {
    pub extensions: Vec<String>,
    pub files: Vec<String>,
}

/// A starting point for a new project: files to create. `{{name}}` in a path or in a file becomes the project's
/// name (as typed, made safe for a folder) and `{{ident}}` the same in lower case with `-` for everything that is not
/// a letter or a digit.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Template {
    pub name: String,
    pub description: String,
    /// The file to open first (a relative path; empty: the first one).
    pub open: String,
    /// Relative path -> content.
    pub files: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// `Languages`, `Documents`, ... - what the Modules tab groups by.
    pub category: String,
    pub summary: String,
    pub description: String,
    /// The cover picture's file name inside the module.
    pub cover: String,
    /// `lisp`: the engine is compiled into the IDE and the module switches it on.
    pub builtin: String,
    pub languages: Vec<LangDef>,
    pub run: Vec<RunDef>,
    /// Syntax checks of the language's files, run when a file is saved.
    pub check: Vec<CheckDef>,
    pub tools: Vec<ToolDef>,
    pub detect: Detect,
    pub templates: Vec<Template>,
    /// Tasks the project's files define (see [`TaskDef`]).
    pub tasks: Vec<TaskDef>,
    /// Test frameworks, the first that wants a file runs its tests (see [`TestDef`]).
    pub tests: Vec<TestDef>,
    /// Debuggers of the languages (see [`DebugDef`]).
    pub debug: Vec<DebugDef>,
}

impl Manifest {
    pub fn parse(json: &str) -> Result<Manifest, String> {
        let m: Manifest = serde_json::from_str(json).map_err(|e| format!("module.json: {e}"))?;
        m.check()?;
        Ok(m)
    }

    /// Reasons the module cannot be used: an id that is not a plain name, a language that does not compile.
    pub fn check(&self) -> Result<(), String> {
        if self.id.is_empty() || !self.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!("the module id {:?} must be lower-case letters, digits and -", self.id));
        }
        if self.name.trim().is_empty() {
            return Err(format!("{}: the module has no name", self.id));
        }
        if parse_version(&self.version).is_none() {
            return Err(format!("{}: the version {:?} is not like 1.2.3", self.id, self.version));
        }
        for l in &self.languages {
            light_lang::Language::new(l.clone())?;
        }
        for r in &self.run {
            if !self.languages.iter().any(|l| l.id == r.language) && self.builtin.is_empty() {
                return Err(format!("{}: the run command {:?} is for the unknown language {:?}", self.id, r.name, r.language));
            }
        }
        for c in &self.check {
            if !self.languages.iter().any(|l| l.id == c.language) {
                return Err(format!("{}: the check {:?} is for the unknown language {:?}", self.id, c.name, c.language));
            }
            crate::modules::check::compile(&c.pattern).map_err(|e| format!("{}: the check {:?}: {e}", self.id, c.name))?;
        }
        for r in self.run.iter().filter(|r| !r.coverage.is_empty()) {
            if !crate::coverage::FORMATS.contains(&r.coverage.as_str()) {
                return Err(format!("{}: the run command {:?}: unknown coverage format {:?}", self.id, r.name, r.coverage));
            }
        }
        for t in &self.tests {
            crate::testing::check(t).map_err(|e| format!("{}: the tests {:?}: {e}", self.id, t.name))?;
            if !t.coverage.is_empty() && (!crate::coverage::FORMATS.contains(&t.coverage.as_str()) || t.coverage_command.trim().is_empty()) {
                return Err(format!("{}: the tests {:?}: coverage needs a known format and a command", self.id, t.name));
            }
            if let Some(l) = t.languages.iter().find(|l| !self.languages.iter().any(|x| x.id == **l)) {
                return Err(format!("{}: the tests {:?} are for the unknown language {l:?}", self.id, t.name));
            }
        }
        for d in &self.debug {
            if d.command.is_empty() || !d.launch.is_object() || d.languages.is_empty() {
                return Err(format!("{}: the debugger {:?} needs languages, a command and a launch configuration", self.id, d.name));
            }
            if let Some(dl) = &d.download
                && (dl.url.is_empty() || dl.sha256.len() != 64)
            {
                return Err(format!("{}: the debugger {:?}: a download needs a URL and its SHA-256", self.id, d.name));
            }
        }
        for t in &self.tasks {
            let listed = !t.json.is_empty() || !t.pattern.is_empty();
            if t.files.is_empty() || (t.json.is_empty() == t.pattern.is_empty() && t.fixed.is_empty()) || (!t.json.is_empty() && !t.pattern.is_empty()) || (listed && t.command.trim().is_empty()) || t.fixed.iter().any(|f| f.name.is_empty() || f.command.trim().is_empty()) {
                return Err(format!("{}: the tasks {:?} need files, a command and either json or a pattern", self.id, t.name));
            }
            if !t.pattern.is_empty() {
                let re = regex::Regex::new(&t.pattern).map_err(|e| format!("{}: the tasks {:?}: {e}", self.id, t.name))?;
                if !re.capture_names().any(|n| n == Some("name")) {
                    return Err(format!("{}: the tasks {:?}: the pattern has no group `name`", self.id, t.name));
                }
            }
        }
        Ok(())
    }
}

/// `1.2.3` -> (1, 2, 3).
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().split('.').map(|p| p.parse::<u32>().ok());
    let r = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(r)
}

/// Is version `a` newer than `b`?
pub fn is_newer(a: &str, b: &str) -> bool {
    matches!((parse_version(a), parse_version(b)), (Some(x), Some(y)) if x > y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r##"{"id":"python","name":"Python","version":"1.0.0","category":"Languages","summary":"s",
        "languages":[{"id":"python","name":"Python","extensions":["py"],"grammar":[{"match":"#.*$","class":"comment"}]}],
        "run":[{"language":"python","name":"Run","command":"python3 {file}"}],
        "tools":[{"name":"Python","programs":["python3"],"apt":"python3"}],
        "detect":{"extensions":["py"],"files":["pyproject.toml"]},
        "templates":[{"name":"Hello","files":{"main.py":"print(1)\n"}}]}"##;

    #[test]
    fn a_run_command_gets_the_file_filled_in() {
        let r = RunDef { language: "c".into(), name: "Run".into(), command: "cc \"{file}\" -o \"{dir}/{stem}\" && ./{name}".into(), windows: "gcc \"{file}\" -o \"{dir}/{stem}.exe\"".into(), ..Default::default() };
        assert_eq!(r.command_line("/home/u/p/main.c", "/home/u/p", false), "cc \"/home/u/p/main.c\" -o \"/home/u/p/main\" && ./main.c");
        assert_eq!(r.command_line("C:/p/main.c", "C:/p", true), "gcc \"C:/p/main.c\" -o \"C:/p/main.exe\"");
        // no Windows variant: the same line
        let same = RunDef { windows: String::new(), ..r.clone() };
        assert_eq!(same.command_line("C:/p/a.b.c", "C:/p", true), "cc \"C:/p/a.b.c\" -o \"C:/p/a.b\" && ./a.b.c");
        // a name that is all extension keeps it as the stem
        assert_eq!(same.command_line("/x/.hidden", "/x", false), "cc \"/x/.hidden\" -o \"/x/.hidden\" && ./.hidden");
    }

    #[test]
    fn a_manifest_parses_and_checks() {
        let m = Manifest::parse(OK).unwrap();
        assert_eq!((m.id.as_str(), m.languages.len(), m.run.len(), m.tools[0].programs[0].as_str()), ("python", 1, 1, "python3"));
        assert_eq!(m.detect.files, ["pyproject.toml"]);
        assert_eq!(m.templates[0].files["main.py"], "print(1)\n");
        // unknown fields are ignored (newer modules on an older IDE), missing ones default
        assert!(Manifest::parse(r#"{"id":"x","name":"X","version":"0.1.0","future":true}"#).is_ok());
    }

    #[test]
    fn broken_manifests_say_why() {
        let err = |j: &str| Manifest::parse(j).expect_err("must fail");
        assert!(err("not json").contains("module.json"));
        assert!(err(r#"{"id":"Py Thon","name":"x","version":"1.0.0"}"#).contains("module id"));
        assert!(err(r#"{"id":"x","name":" ","version":"1.0.0"}"#).contains("no name"));
        assert!(err(r#"{"id":"x","name":"X","version":"1.0"}"#).contains("version"));
        assert!(err(r#"{"id":"x","name":"X","version":"1.0.0","languages":[{"id":"l","grammar":[{"match":"(","class":"comment"}]}]}"#).contains("rule 1"));
        assert!(err(r#"{"id":"x","name":"X","version":"1.0.0","run":[{"language":"nope","name":"r","command":"c"}]}"#).contains("unknown language"));
        // a built-in module has no languages of its own to run
        assert!(Manifest::parse(r#"{"id":"lisp","name":"Lisp","version":"1.0.0","builtin":"lisp","run":[{"language":"lisp","name":"r","command":"c"}]}"#).is_ok());
    }

    #[test]
    fn tasks_come_from_json_members_or_a_pattern_in_their_order() {
        let npm = TaskDef { name: "npm".into(), files: vec!["package.json".into()], json: "scripts".into(), command: "npm --prefix \"{dir}\" run \"{task}\"".into(), ..Default::default() };
        let package = r#"{"name":"x","scripts":{"test":"vitest","build":"vite build","lint:fix":"eslint .","bad\"name":"x"},"devDependencies":{"build":"1"}}"#;
        assert_eq!(npm.tasks_in(package), ["test", "build", "lint:fix"]);
        assert!(npm.tasks_in("{not json").is_empty() && npm.tasks_in(r#"{"scripts":[]}"#).is_empty());
        assert!(npm.holds("Package.JSON") && !npm.holds("package-lock.json"));
        assert_eq!(npm.command_line("build", "/p/web/package.json", "/p/web", "/p", false), "npm --prefix \"/p/web\" run \"build\"");
        let presets = TaskDef { name: "CMake".into(), files: vec!["CMakePresets.json".into()], json: "configurePresets".into(), command: "cmake --preset {task}".into(), fixed: vec![FixedTask { name: "fresh".into(), command: "cmake --fresh -S {root}".into(), ..Default::default() }], ..Default::default() };
        assert_eq!(presets.tasks_in(r#"{"configurePresets":[{"name":"base","hidden":true},{"name":"debug"},{"name":"release"}]}"#), ["fresh", "debug", "release"]);
        assert_eq!(presets.command_line("fresh", "/p/CMakePresets.json", "/p", "/p", false), "cmake --fresh -S /p");
        assert_eq!(presets.command_line("debug", "/p/CMakePresets.json", "/p", "/p", false), "cmake --preset debug");
        assert_eq!(presets.tasks_in("not json"), ["fresh"]);
        let make = TaskDef { name: "make".into(), files: vec!["Makefile".into()], pattern: r"(?m)^(?P<name>[A-Za-z0-9_][\w.-]*)[ \t]*:(?:[^=]|$)".into(), command: "make {task}".into(), ..Default::default() };
        assert_eq!(make.tasks_in("all: app\nCC := gcc\napp: main.o\n\tcc -o app\nclean:\n\trm app\nall: again\n"), ["all", "app", "clean"]);
        let module = |t: &str| Manifest::parse(&format!(r#"{{"id":"x","name":"X","version":"1.0.0","tasks":[{t}]}}"#));
        assert!(module(r#"{"name":"npm","files":["package.json"],"json":"scripts","command":"npm run {task}"}"#).is_ok());
        assert!(module(r#"{"name":"npm","files":["package.json"],"command":"npm run {task}"}"#).unwrap_err().contains("either json or a pattern"));
        assert!(module(r#"{"name":"m","files":["Makefile"],"pattern":"^([a-z]+):","command":"make"}"#).unwrap_err().contains("group `name`"));
    }

    #[test]
    fn versions_compare() {
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("a.b.c"), None);
        assert!(is_newer("1.10.0", "1.9.9") && is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.0.0", "1.0.0") && !is_newer("0.9.0", "1.0.0") && !is_newer("garbage", "1.0.0"));
    }
}
