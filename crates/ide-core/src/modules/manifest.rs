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

/// A language server for languages of the module (the Language Server Protocol, see [`crate::lsp`]): started for a
/// project when a file of one of its languages is opened, in the project's Linux or on Windows like a run command,
/// and asked where names are defined and used, how to rename them, what may be typed and what is wrong in the text.
/// Without it (not installed, or it stopped) the IDE answers from the text, as it does for a language without one.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct LspDef {
    /// The language ids it serves.
    pub languages: Vec<String>,
    /// The protocol's name of a language, where it is not the module's (`jsx` is `javascriptreact`).
    pub language_ids: BTreeMap<String, String>,
    /// What the IDE calls it in messages (`clangd`).
    pub name: String,
    /// The shell command line that starts it, in the project's folder. It talks over its stdin and stdout.
    pub command: String,
    /// A different command line for a project that runs on Windows (empty: the same one).
    pub windows: String,
    /// The program that must be installed (empty: the first word of the command).
    pub program: String,
    /// The server's `initializationOptions`, sent as they are.
    pub options: serde_json::Value,
    /// Its settings, by section (`{"basedpyright": {"analysis": {...}}}`): what it asks for with
    /// `workspace/configuration`.
    pub settings: serde_json::Value,
    /// How Light installs the server when it is not there (none: the user installs it).
    pub install: Option<InstallDef>,
}

/// The platforms a server is installed for.
pub const PLATFORMS: [&str; 6] = ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "win-x64", "win-arm64"];

/// A release download: where it is, its SHA-256 and size, and how it unpacks.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ArchiveDef {
    /// An `https://` address.
    pub url: String,
    /// 64 hexadecimal digits.
    pub sha256: String,
    pub size: u64,
    /// Leading folders left out when unpacking (1 for an archive with everything in one folder).
    pub strip: u32,
    /// The folder of the programs, after unpacking (`bin`; empty: the top).
    pub bin: String,
}

/// A runtime servers run on (Node.js for the npm servers), installed once for all of them.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct RuntimeDef {
    /// `node`.
    pub name: String,
    pub version: String,
    /// Where it comes from, as the user is told (`nodejs.org`).
    pub source: String,
    /// By platform (see [`PLATFORMS`]).
    pub archives: BTreeMap<String, ArchiveDef>,
}

impl RuntimeDef {
    /// The folder it is installed in, among the servers: `node-24.22.2`.
    pub fn folder(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }
}

/// How Light installs a server, at the user's word: exact versions, and every download checked (see
/// `docs/language-servers.md`). Either an npm package, from the lockfile the module carries, on a runtime; or a
/// prebuilt archive of the server.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct InstallDef {
    pub version: String,
    /// Where the server comes from, as the user is told (`registry.npmjs.org`).
    pub source: String,
    /// About how much the server takes, in bytes, as the user is told (the runtime's archives say their own).
    pub size: u64,
    pub runtime: Option<RuntimeDef>,
    /// npm: the module's files `<npm>.package.json` and `<npm>.package-lock.json` (installed with `npm ci`).
    pub npm: String,
    /// Or prebuilt archives of the server, by platform.
    pub archives: BTreeMap<String, ArchiveDef>,
}

impl InstallDef {
    /// The folder the server `name` is installed in, among the servers: `basedpyright-1.40.2`.
    pub fn folder(&self, name: &str) -> String {
        format!("{name}-{}", self.version)
    }

    /// Can it be installed on `platform`?
    pub fn available_for(&self, platform: &str) -> bool {
        if self.npm.is_empty() { self.archives.contains_key(platform) } else { self.runtime.as_ref().is_some_and(|r| r.archives.contains_key(platform)) }
    }

    /// The folder of the server's programs inside its install folder.
    pub fn bin(&self, platform: &str) -> String {
        if self.npm.is_empty() { self.archives.get(platform).map(|a| a.bin.clone()).unwrap_or_default() } else { "node_modules/.bin".into() }
    }

    /// Reasons it cannot be installed safely.
    fn check(&self) -> Result<(), String> {
        let plain = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'));
        if !plain(&self.version) {
            return Err(format!("the version {:?} is not a plain version", self.version));
        }
        if self.npm.is_empty() == self.archives.is_empty() {
            return Err("an install is either npm or archives".into());
        }
        if !self.npm.is_empty() && (!plain(&self.npm) || self.runtime.is_none()) {
            return Err("an npm install names its files plainly and the runtime it runs on".into());
        }
        let archives = self.archives.iter().chain(self.runtime.iter().flat_map(|r| r.archives.iter()));
        for (platform, a) in archives {
            if !PLATFORMS.contains(&platform.as_str()) {
                return Err(format!("unknown platform {platform:?}"));
            }
            if !a.url.starts_with("https://") || a.url.contains(['"', '\'', ' ', '\\']) {
                return Err(format!("the address {:?} is not a plain https address", a.url));
            }
            if a.sha256.len() != 64 || !a.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!("{}: the SHA-256 is not 64 hexadecimal digits", a.url));
            }
            if a.bin.split('/').any(|p| p == ".." || p.starts_with('.')) || a.bin.contains(['\\', ':']) {
                return Err(format!("{}: the folder of its programs {:?} is not inside it", a.url, a.bin));
            }
        }
        if let Some(r) = &self.runtime
            && (!plain(&r.name) || !plain(&r.version))
        {
            return Err("the runtime has no plain name and version".into());
        }
        Ok(())
    }
}

impl LspDef {
    /// The command line that starts the server; `windows`: the project runs on Windows.
    pub fn command_line(&self, windows: bool) -> &str {
        if windows && !self.windows.is_empty() { &self.windows } else { &self.command }
    }

    /// The program the server is.
    pub fn program(&self) -> &str {
        if self.program.is_empty() { self.command.split_whitespace().next().unwrap_or("") } else { &self.program }
    }

    /// The protocol's name of the module's language `lang`.
    pub fn language_id<'a>(&'a self, lang: &'a str) -> &'a str {
        self.language_ids.get(lang).map_or(lang, String::as_str)
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
    /// Language servers of the module's languages.
    pub lsp: Vec<LspDef>,
    pub tools: Vec<ToolDef>,
    pub detect: Detect,
    pub templates: Vec<Template>,
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
        for s in &self.lsp {
            if s.command.trim().is_empty() {
                return Err(format!("{}: the language server {:?} has no command", self.id, s.name));
            }
            if s.languages.is_empty() {
                return Err(format!("{}: the language server {:?} serves no language", self.id, s.name));
            }
            if let Some(l) = s.languages.iter().chain(s.language_ids.keys()).find(|l| !self.languages.iter().any(|x| x.id == **l)) {
                return Err(format!("{}: the language server {:?} is for the unknown language {l:?}", self.id, s.name));
            }
            if let Some(i) = &s.install {
                if !s.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) || s.name.is_empty() {
                    return Err(format!("{}: a language server Light installs needs a plain name, not {:?}", self.id, s.name));
                }
                i.check().map_err(|e| format!("{}: installing {:?}: {e}", self.id, s.name))?;
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
        let lang = r#""languages":[{"id":"c","grammar":[]}]"#;
        assert!(err(&format!(r#"{{"id":"x","name":"X","version":"1.0.0",{lang},"lsp":[{{"languages":["c"],"name":"s"}}]}}"#)).contains("no command"));
        assert!(err(&format!(r#"{{"id":"x","name":"X","version":"1.0.0",{lang},"lsp":[{{"name":"s","command":"s"}}]}}"#)).contains("serves no language"));
        assert!(err(&format!(r#"{{"id":"x","name":"X","version":"1.0.0",{lang},"lsp":[{{"languages":["c","go"],"name":"s","command":"s"}}]}}"#)).contains("unknown language \"go\""));
    }

    #[test]
    fn a_language_server() {
        let m = Manifest::parse(r#"{"id":"c-cpp","name":"C","version":"1.0.0","languages":[{"id":"c","grammar":[]},{"id":"cpp","grammar":[]}],
            "lsp":[{"languages":["c","cpp"],"name":"clangd","command":"clangd --log=error","windows":"clangd.exe --log=error","options":{"fallbackFlags":["-std=c11"]}}]}"#).unwrap();
        let s = &m.lsp[0];
        assert_eq!((s.command_line(false), s.command_line(true), s.program()), ("clangd --log=error", "clangd.exe --log=error", "clangd"));
        assert_eq!(s.options["fallbackFlags"][0], "-std=c11");
        let named = LspDef { program: "pyright".into(), windows: String::new(), ..s.clone() };
        assert_eq!((named.program(), named.command_line(true)), ("pyright", "clangd --log=error"));
        assert_eq!(s.language_id("cpp"), "cpp");
        let js = Manifest::parse(r#"{"id":"js","name":"JS","version":"1.0.0","languages":[{"id":"javascript","grammar":[]},{"id":"jsx","grammar":[]}],
            "lsp":[{"languages":["javascript","jsx"],"language_ids":{"jsx":"javascriptreact"},"name":"ts","command":"typescript-language-server --stdio"}]}"#).unwrap();
        assert_eq!((js.lsp[0].language_id("jsx"), js.lsp[0].language_id("javascript")), ("javascriptreact", "javascript"));
        assert!(Manifest::parse(r#"{"id":"js","name":"JS","version":"1.0.0","languages":[{"id":"javascript","grammar":[]}],
            "lsp":[{"languages":["javascript"],"language_ids":{"tsx":"typescriptreact"},"name":"ts","command":"t"}]}"#).is_err());
        // an older module has none
        assert!(Manifest::parse(OK).unwrap().lsp.is_empty());
    }

    fn with_install(install: &str) -> Result<Manifest, String> {
        Manifest::parse(&format!(r#"{{"id":"py","name":"Py","version":"1.0.0","languages":[{{"id":"python","grammar":[]}}],
            "lsp":[{{"languages":["python"],"name":"basedpyright","command":"basedpyright-langserver --stdio","install":{install}}}]}}"#))
    }

    const NODE: &str = r#"{"name":"node","version":"24.22.2","source":"nodejs.org","archives":{
        "linux-x64":{"url":"https://nodejs.org/dist/v24.22.2/node-v24.22.2-linux-x64.tar.gz","sha256":"361718c444e49af2ca5a110c83ba6a8901143f23ab285ebbef0341f391f482a9","size":58300000,"strip":1,"bin":"bin"},
        "win-x64":{"url":"https://nodejs.org/dist/v24.22.2/node-v24.22.2-win-x64.zip","sha256":"ca1bff5d8ead13c9f2ea404a176db46bd430704d5b5113734049fa365e7799e2","size":37800000,"strip":1,"bin":""}}}"#;

    #[test]
    fn installs_are_checked() {
        let m = with_install(&format!(r#"{{"version":"1.40.2","source":"registry.npmjs.org","size":36000000,"npm":"basedpyright","runtime":{NODE}}}"#)).unwrap();
        let i = m.lsp[0].install.as_ref().unwrap();
        assert_eq!((i.folder("basedpyright"), i.runtime.as_ref().unwrap().folder(), i.bin("win-x64")), ("basedpyright-1.40.2".into(), "node-24.22.2".into(), "node_modules/.bin".into()));
        assert!(i.available_for("linux-x64") && i.available_for("win-x64") && !i.available_for("darwin-arm64"));
        let asm = with_install(r#"{"version":"0.10.1","archives":{"linux-x64":{"url":"https://github.com/x/releases/download/v0.10.1/a.tar.gz","sha256":"2a2f386c10348e365df484d5c66d084813e7dbf8ab4e9eb7a7cc0bb0acf2f8f8","bin":""}}}"#).unwrap();
        assert!(asm.lsp[0].install.as_ref().unwrap().available_for("linux-x64"));
        let err = |i: &str| with_install(i).expect_err("must fail");
        assert!(err(r#"{"version":"1 ; rm","npm":"x","runtime":{"name":"node","version":"1"}}"#).contains("plain version"));
        assert!(err(r#"{"version":"1.0.0","npm":"x"}"#).contains("runtime"));
        assert!(err(r#"{"version":"1.0.0"}"#).contains("either"));
        assert!(err(r#"{"version":"1.0.0","archives":{"linux-x64":{"url":"http://example.com/a.tgz","sha256":"2a2f386c10348e365df484d5c66d084813e7dbf8ab4e9eb7a7cc0bb0acf2f8f8"}}}"#).contains("https"));
        assert!(err(r#"{"version":"1.0.0","archives":{"linux-x64":{"url":"https://example.com/a.tgz","sha256":"abc"}}}"#).contains("SHA-256"));
        assert!(err(r#"{"version":"1.0.0","archives":{"amiga-68k":{"url":"https://example.com/a.tgz","sha256":"2a2f386c10348e365df484d5c66d084813e7dbf8ab4e9eb7a7cc0bb0acf2f8f8"}}}"#).contains("platform"));
        assert!(err(r#"{"version":"1.0.0","archives":{"linux-x64":{"url":"https://example.com/a.tgz","sha256":"2a2f386c10348e365df484d5c66d084813e7dbf8ab4e9eb7a7cc0bb0acf2f8f8","bin":"../.."}}}"#).contains("inside"));
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
