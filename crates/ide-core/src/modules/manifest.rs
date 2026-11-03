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
        RunDef { language: self.language.clone(), name: self.name.clone(), command: self.command.clone(), windows: self.windows.clone() }.command_line(file, dir, windows)
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
    /// The tasks are the members of this object of a JSON file (`scripts`)...
    pub json: String,
    /// ...or the matches of this regular expression (with a group `name`) over the file's text.
    pub pattern: String,
    /// A shell command line with `{task}` (the task's name), `{file}` and `{dir}` (the file holding the tasks and its
    /// folder, as the shell sees them); it runs in `{dir}`.
    pub command: String,
    /// A different command line for a project that runs on Windows (empty: the same one).
    pub windows: String,
    /// Folders whose files never hold the project's own tasks (`node_modules`): they are not looked in.
    pub skip: Vec<String>,
}

impl TaskDef {
    /// The names of the tasks that `text` (the content of one of the `files`) defines, in the order they are written.
    /// Names a command line could not carry safely (quotes, line breaks) are left out.
    pub fn tasks_in(&self, text: &str) -> Vec<String> {
        let mut names: Vec<String> = if !self.json.is_empty() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return vec![] };
            let Some(object) = value.get(&self.json).and_then(|v| v.as_object()) else { return vec![] };
            // serde_json keeps the members sorted: the file says in which order they come
            let from = text.find(&format!("\"{}\"", self.json)).unwrap_or(0);
            let mut keyed: Vec<(usize, String)> = object.keys().map(|k| {
                let quoted = serde_json::to_string(k).unwrap_or_default();
                (text[from..].find(&quoted).map_or(usize::MAX, |i| from + i), k.clone())
            }).collect();
            keyed.sort();
            keyed.into_iter().map(|(_, k)| k).collect()
        } else {
            let Ok(re) = regex::Regex::new(&self.pattern) else { return vec![] };
            re.captures_iter(text).filter_map(|c| c.name("name").map(|m| m.as_str().to_string())).collect()
        };
        let mut seen = std::collections::HashSet::new();
        names.retain(|n| !n.trim().is_empty() && !n.contains(['"', '\n', '\r', '`', '$', '%']) && seen.insert(n.clone()));
        names
    }

    /// Is `file_name` one of the files that hold these tasks?
    pub fn holds(&self, file_name: &str) -> bool {
        self.files.iter().any(|f| f.eq_ignore_ascii_case(file_name))
    }

    /// The command line that runs `task` of the file `file` in `dir` (see [`RunDef::command_line`]).
    pub fn command_line(&self, task: &str, file: &str, dir: &str, windows: bool) -> String {
        let template = if windows && !self.windows.is_empty() { &self.windows } else { &self.command };
        template.replace("{task}", task).replace("{file}", file).replace("{dir}", dir)
    }
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
        for t in &self.tasks {
            if t.files.is_empty() || t.command.trim().is_empty() || t.json.is_empty() == t.pattern.is_empty() {
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
        let r = RunDef { language: "c".into(), name: "Run".into(), command: "cc \"{file}\" -o \"{dir}/{stem}\" && ./{name}".into(), windows: "gcc \"{file}\" -o \"{dir}/{stem}.exe\"".into() };
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
        assert_eq!(npm.command_line("build", "/p/web/package.json", "/p/web", false), "npm --prefix \"/p/web\" run \"build\"");
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
