//! Project model: directory listing, file operations, project scaffolding, VCS info.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::search::walk_files;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Folders the coding agents keep their settings, memory and caches in: a project hides them unless it says otherwise.
pub const AGENT_FOLDERS: [&str; 16] = [".claude", ".codex", ".cursor", ".gemini", ".windsurf", ".continue", ".cline", ".roo", ".kilocode", ".junie", ".aider.tags.cache.v3", ".amazonq", ".opencode", ".qwen", ".kiro", ".augment"];

/// What a project leaves out of its tree, Go to File, search and the symbol index: patterns as `.gitignore` writes them,
/// one per entry - `.claude` is every folder (or file) of that name, `/docs/old` only that one under the root,
/// `*.log` every log file.
#[derive(Clone, Debug, Default)]
pub struct Hidden {
    rules: Option<ignore::gitignore::Gitignore>,
}

impl Hidden {
    pub fn new(root: &Path, patterns: &[String]) -> Hidden {
        let mut b = ignore::gitignore::GitignoreBuilder::new(root);
        let mut any = false;
        for p in patterns.iter().map(|p| p.trim()).filter(|p| !p.is_empty() && !p.starts_with('#')) {
            // a pattern that does not make sense is left out, the others still apply
            any |= b.add_line(None, p).is_ok();
        }
        Hidden { rules: any.then(|| b.build().ok()).flatten() }
    }

    /// Is `path` (under the root) or a folder it is in hidden?
    pub fn is_hidden(&self, path: &Path, is_dir: bool) -> bool {
        let Some(g) = &self.rules else { return false };
        // the matcher only answers for paths under the root (it panics on others), and never hides the root itself
        match path.strip_prefix(g.path()) {
            Ok(rel) if !rel.as_os_str().is_empty() => g.matched_path_or_any_parents(path, is_dir).is_ignore(),
            _ => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_none()
    }
}

/// Directory listing for the project tree: folders first, then files, case-insensitive order.
pub fn list_dir(path: &Path) -> io::Result<Vec<Entry>> {
    let mut v: Vec<Entry> = fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if matches!(name.as_str(), ".git" | ".DS_Store" | "Thumbs.db" | ".idea") {
                return None;
            }
            let is_dir = e.file_type().ok()?.is_dir();
            Some(Entry { name, path: e.path(), is_dir })
        })
        .collect();
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(v)
}

/// All project files (relative paths use `/`), for Go to File.
pub fn list_files(root: &Path, limit: usize) -> Vec<PathBuf> {
    walk_files(root).take(limit).collect()
}

/// All project files that are not `hidden`.
pub fn list_project_files(root: &Path, hidden: &Hidden, limit: usize) -> Vec<PathBuf> {
    crate::search::walk_project(root, hidden).take(limit).collect()
}

pub fn relative(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

pub fn create_file(path: &Path, content: &str) -> io::Result<()> {
    if path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", path.display())));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)
}

pub fn create_dir(path: &Path) -> io::Result<()> {
    if path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", path.display())));
    }
    fs::create_dir_all(path)
}

pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", to.display())));
    }
    fs::rename(from, to)
}

pub fn delete(path: &Path) -> io::Result<()> {
    if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

/// Current git branch, read directly from `.git/HEAD` (no git binary needed).
pub fn git_branch(root: &Path) -> Option<String> {
    let mut dir = Some(root);
    while let Some(d) = dir {
        let head = d.join(".git").join("HEAD");
        if let Ok(s) = fs::read_to_string(&head) {
            let s = s.trim();
            return Some(match s.strip_prefix("ref: refs/heads/") {
                Some(b) => b.to_string(),
                None => s.chars().take(7).collect(),
            });
        }
        dir = d.parent();
    }
    None
}

/// Make a valid ASDF system / directory name from user input.
pub fn sanitize_project_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "untitled".into() } else { s }
}

/// Scaffold an ASDF based Common Lisp project. Returns the created project directory.
pub fn create_lisp_project(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let name = sanitize_project_name(name);
    let root = parent.join(&name);
    if root.exists() && fs::read_dir(&root)?.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} is not empty", root.display())));
    }
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("test"))?;
    fs::write(
        root.join(format!("{name}.asd")),
        format!(
            "(asdf:defsystem \"{name}\"\n  :description \"{name}\"\n  :version \"0.1.0\"\n  :depends-on ()\n  :serial t\n  :components ((:module \"src\"\n                :components ((:file \"package\")\n                             (:file \"main\"))))\n  :in-order-to ((test-op (test-op \"{name}/test\"))))\n\n(asdf:defsystem \"{name}/test\"\n  :depends-on (\"{name}\")\n  :components ((:module \"test\"\n                :components ((:file \"tests\"))))\n  :perform (test-op (o c) (uiop:symbol-call :{name}/test :run)))\n"
        ),
    )?;
    fs::write(
        root.join("src/package.lisp"),
        format!("(defpackage :{name}\n  (:use :cl)\n  (:export #:main #:greet))\n"),
    )?;
    fs::write(
        root.join("src/main.lisp"),
        format!("(in-package :{name})\n\n(defun greet (name)\n  \"Return a greeting for NAME.\"\n  (format nil \"Hello, ~a!\" name))\n\n(defun main ()\n  (format t \"~a~%\" (greet \"world\")))\n"),
    )?;
    fs::write(
        root.join("test/tests.lisp"),
        format!("(defpackage :{name}/test\n  (:use :cl)\n  (:export #:run))\n(in-package :{name}/test)\n\n(defun run ()\n  (assert (string= (funcall (intern \"GREET\" :{name}) \"Lisp\") \"Hello, Lisp!\"))\n  (format t \"All tests passed.~%\"))\n"),
    )?;
    fs::write(root.join("README.md"), format!("# {name}\n\nA Common Lisp project created with Light.\n"))?;
    fs::write(root.join(".gitignore"), "*.fasl\n")?;
    Ok(root)
}

/// A name safe to use as a folder and in `{{ident}}`: lower case letters and digits separated by single dashes.
pub fn identifier(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "project".into() } else { out }
}

/// Create the folder `name` in `parent` and write `files` (relative path -> content, see
/// [`Template`](crate::modules::Template) for `{{name}}` and `{{ident}}`) into it; nothing in it may exist yet. Paths
/// that would leave the project are refused. Returns the project's folder.
pub fn create_project(parent: &Path, name: &str, files: &std::collections::BTreeMap<String, String>) -> io::Result<PathBuf> {
    let name = sanitize_project_name(name);
    let ident = identifier(&name);
    let root = parent.join(&name);
    if root.exists() && fs::read_dir(&root)?.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} is not empty", root.display())));
    }
    let subst = |s: &str| s.replace("{{name}}", &name).replace("{{ident}}", &ident);
    // check every path before writing anything
    let mut targets = Vec::new();
    for (rel, content) in files {
        let rel = subst(rel);
        let p = Path::new(&rel);
        if rel.is_empty() || p.is_absolute() || p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{rel}: not a path inside the project")));
        }
        targets.push((root.join(p), subst(content)));
    }
    fs::create_dir_all(&root)?;
    for (path, content) in targets {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, content)?;
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn empty_projects_do_not_seed_any_files() {
        let temp = TempDir::new("empty-project");
        let root = create_project(temp.path(), "empty", &Default::default()).unwrap();
        assert_eq!(fs::read_dir(root).unwrap().count(), 0);
    }

    #[test]
    fn listing_sorts_dirs_first_and_hides_git() {
        let d = TempDir::new("list");
        fs::create_dir_all(d.path().join("b-dir")).unwrap();
        fs::create_dir_all(d.path().join(".git")).unwrap();
        fs::write(d.path().join("Zeta.lisp"), "").unwrap();
        fs::write(d.path().join("alpha.lisp"), "").unwrap();
        let names: Vec<_> = list_dir(d.path()).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["b-dir", "alpha.lisp", "Zeta.lisp"]);
    }

    #[test]
    fn file_operations() {
        let d = TempDir::new("ops");
        let f = d.path().join("sub/new.lisp");
        create_file(&f, "(a)").unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "(a)");
        assert!(create_file(&f, "x").is_err(), "must not overwrite");
        let g = d.path().join("sub/renamed.lisp");
        rename(&f, &g).unwrap();
        assert!(!f.exists() && g.exists());
        assert!(rename(&g, &g).is_err());
        delete(&d.path().join("sub")).unwrap();
        assert!(!d.path().join("sub").exists());
    }

    #[test]
    fn reads_git_branch() {
        let d = TempDir::new("git");
        fs::create_dir_all(d.path().join(".git")).unwrap();
        fs::write(d.path().join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(git_branch(d.path()).as_deref(), Some("feature/x"));
        fs::write(d.path().join(".git/HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(git_branch(d.path()).as_deref(), Some("0123456"));
        let other = TempDir::new("nogit");
        // may find a parent repo on dev machines, but must not panic
        let _ = git_branch(other.path());
    }

    #[test]
    fn name_sanitising() {
        assert_eq!(sanitize_project_name("My Cool App!"), "my-cool-app");
        assert_eq!(sanitize_project_name("  "), "untitled");
        assert_eq!(sanitize_project_name("a_b.c"), "a_b.c");
    }

    #[test]
    fn scaffold_project() {
        let d = TempDir::new("proj");
        let root = create_lisp_project(d.path(), "Hello World").unwrap();
        assert!(root.ends_with("hello-world"));
        for f in ["hello-world.asd", "src/package.lisp", "src/main.lisp", "test/tests.lisp", "README.md"] {
            assert!(root.join(f).exists(), "{f}");
        }
        // generated code must be balanced
        for f in ["hello-world.asd", "src/package.lisp", "src/main.lisp", "test/tests.lisp"] {
            let src = fs::read_to_string(root.join(f)).unwrap();
            let p = lisp_syntax::parse(&src, lisp_syntax::Dialect::CommonLisp);
            assert!(p.diagnostics.is_empty(), "{f}: {:?}", p.diagnostics);
        }
        assert!(create_lisp_project(d.path(), "hello world").is_err(), "non-empty target");
    }
}

/// Answers "is this path ignored by .gitignore?" for the project tree, honouring nested `.gitignore`
/// files and `.git/info/exclude`. Rules of deeper directories override those of their parents.
pub struct IgnoreMatcher {
    root: PathBuf,
    by_dir: std::collections::HashMap<PathBuf, Option<ignore::gitignore::Gitignore>>,
    exclude: Option<ignore::gitignore::Gitignore>,
}

impl IgnoreMatcher {
    pub fn new(root: &Path) -> IgnoreMatcher {
        let exclude_file = root.join(".git").join("info").join("exclude");
        let exclude = exclude_file.is_file().then(|| {
            let mut b = ignore::gitignore::GitignoreBuilder::new(root);
            b.add(&exclude_file);
            b.build().ok()
        });
        IgnoreMatcher { root: root.to_path_buf(), by_dir: Default::default(), exclude: exclude.flatten() }
    }

    /// Drop cached `.gitignore` files (after the user edited one).
    pub fn reset(&mut self) {
        let root = self.root.clone();
        *self = IgnoreMatcher::new(&root);
    }

    fn gitignore_of(&mut self, dir: &Path) -> Option<&ignore::gitignore::Gitignore> {
        self.by_dir
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                let f = dir.join(".gitignore");
                if !f.is_file() {
                    return None;
                }
                let mut b = ignore::gitignore::GitignoreBuilder::new(dir);
                b.add(&f);
                b.build().ok()
            })
            .as_ref()
    }

    pub fn is_ignored(&mut self, path: &Path, is_dir: bool) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else { return false };
        if rel.as_os_str().is_empty() {
            return false;
        }
        let mut verdict: Option<bool> = None;
        if let Some(g) = &self.exclude {
            match g.matched_path_or_any_parents(path, is_dir) {
                ignore::Match::Ignore(_) => verdict = Some(true),
                ignore::Match::Whitelist(_) => verdict = Some(false),
                ignore::Match::None => {}
            }
        }
        // directories from the root down to the path's parent
        let mut dirs: Vec<PathBuf> = vec![self.root.clone()];
        let mut cur = self.root.clone();
        let comps: Vec<_> = rel.components().collect();
        for c in &comps[..comps.len().saturating_sub(1)] {
            cur = cur.join(c);
            dirs.push(cur.clone());
        }
        for d in dirs {
            if let Some(g) = self.gitignore_of(&d) {
                match g.matched_path_or_any_parents(path, is_dir) {
                    ignore::Match::Ignore(_) => verdict = Some(true),
                    ignore::Match::Whitelist(_) => verdict = Some(false),
                    ignore::Match::None => {}
                }
            }
        }
        verdict.unwrap_or(false)
    }
}

#[cfg(test)]
mod ignore_tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn root_and_nested_gitignore_rules() {
        let d = TempDir::new("ign");
        fs::create_dir_all(d.path().join("src/sub")).unwrap();
        fs::create_dir_all(d.path().join("node_modules/dep")).unwrap();
        fs::write(d.path().join(".gitignore"), "node_modules/\n*.fasl\n/build\n").unwrap();
        fs::write(d.path().join("src/.gitignore"), "*.tmp\n!keep.fasl\n").unwrap();
        let mut m = IgnoreMatcher::new(d.path());
        assert!(m.is_ignored(&d.path().join("node_modules"), true));
        assert!(m.is_ignored(&d.path().join("node_modules/dep/x.js"), false), "children of ignored dirs");
        assert!(m.is_ignored(&d.path().join("a.fasl"), false));
        assert!(m.is_ignored(&d.path().join("src/sub/b.fasl"), false));
        assert!(!m.is_ignored(&d.path().join("src/keep.fasl"), false), "nested whitelist wins");
        assert!(m.is_ignored(&d.path().join("src/x.tmp"), false));
        assert!(!m.is_ignored(&d.path().join("x.tmp"), false), "nested rule does not leak upward");
        assert!(m.is_ignored(&d.path().join("build"), true));
        assert!(!m.is_ignored(&d.path().join("src/build"), true), "anchored pattern");
        assert!(!m.is_ignored(&d.path().join("src/main.lisp"), false));
        assert!(!m.is_ignored(d.path(), true));
    }

    #[test]
    fn git_info_exclude_and_reset() {
        let d = TempDir::new("ign2");
        fs::create_dir_all(d.path().join(".git/info")).unwrap();
        fs::write(d.path().join(".git/info/exclude"), "/.light/\nsecret.txt\n").unwrap();
        let mut m = IgnoreMatcher::new(d.path());
        assert!(m.is_ignored(&d.path().join(".light"), true));
        assert!(m.is_ignored(&d.path().join("secret.txt"), false));
        assert!(!m.is_ignored(&d.path().join("a.txt"), false));
        fs::write(d.path().join(".gitignore"), "a.txt\n").unwrap();
        assert!(!m.is_ignored(&d.path().join("a.txt"), false), "cached until reset");
        m.reset();
        assert!(m.is_ignored(&d.path().join("a.txt"), false));
    }

    #[test]
    fn project_walks_skip_ignored_files() {
        let d = TempDir::new("ign3");
        fs::create_dir_all(d.path().join("node_modules")).unwrap();
        fs::create_dir_all(d.path().join("vendor")).unwrap();
        fs::write(d.path().join(".gitignore"), "vendor/\n").unwrap();
        fs::write(d.path().join("node_modules/a.lisp"), "(defun nm ())").unwrap();
        fs::write(d.path().join("vendor/b.lisp"), "(defun v ())").unwrap();
        fs::write(d.path().join("c.lisp"), "(defun c ())").unwrap();
        let files = list_files(d.path(), 100);
        let names: Vec<String> = files.iter().map(|f| relative(d.path(), f)).collect();
        assert!(names.contains(&"c.lisp".to_string()) && names.contains(&".gitignore".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("vendor") || n.contains("node_modules")), "{names:?}");
        let idx = crate::index::ProjectIndex::build(d.path());
        assert!(idx.find_definitions("v").is_empty() && idx.find_definitions("nm").is_empty());
        assert_eq!(idx.find_definitions("c").len(), 1);
    }

    #[test]
    fn hidden_folders_leave_the_project() {
        let d = TempDir::new("hidden");
        for dir in [".claude/agents", "src/.cursor", "docs/old", "docs/new"] {
            fs::create_dir_all(d.path().join(dir)).unwrap();
        }
        for f in [".claude/agents/a.md", "src/.cursor/rules.md", "src/main.c", "docs/old/x.md", "docs/new/y.md", "build.log"] {
            fs::write(d.path().join(f), "").unwrap();
        }
        let pats: Vec<String> = [".claude", ".cursor", "/docs/old", "*.log", "", "# a comment"].iter().map(|s| s.to_string()).collect();
        let h = Hidden::new(d.path(), &pats);
        assert!(h.is_hidden(&d.path().join(".claude"), true));
        assert!(h.is_hidden(&d.path().join(".claude/agents/a.md"), false), "what is inside a hidden folder");
        assert!(h.is_hidden(&d.path().join("src/.cursor"), true), "a name hides the folder at any depth");
        assert!(h.is_hidden(&d.path().join("docs/old"), true) && !h.is_hidden(&d.path().join("docs/new"), true));
        assert!(!h.is_hidden(d.path(), true), "never the project itself");
        assert!(!h.is_hidden(&std::env::temp_dir().join("elsewhere/.claude"), true), "nor anything outside it");
        let names: Vec<String> = list_project_files(d.path(), &h, 100).iter().map(|f| relative(d.path(), f)).collect();
        let mut names = names;
        names.sort();
        assert_eq!(names, ["docs/new/y.md", "src/main.c"]);
        assert!(Hidden::new(d.path(), &[]).is_empty() && !Hidden::new(d.path(), &[]).is_hidden(&d.path().join(".claude"), true));
        assert_eq!(list_project_files(d.path(), &Hidden::default(), 100).len(), 6);
    }

    #[test]
    fn projects_are_made_from_template_files() {
        let d = TempDir::new("create-project");
        let mut files = std::collections::BTreeMap::new();
        files.insert("{{ident}}.asd".to_string(), "(defsystem \"{{ident}}\")".to_string());
        files.insert("src/main.lisp".to_string(), "; {{name}}\n".to_string());
        let root = create_project(d.path(), "My Project!", &files).unwrap();
        assert_eq!(root.file_name().unwrap(), "my-project");
        assert_eq!(fs::read_to_string(root.join("my-project.asd")).unwrap(), "(defsystem \"my-project\")");
        assert_eq!(fs::read_to_string(root.join("src/main.lisp")).unwrap(), "; my-project\n");
        // not into a folder that has things in it
        assert!(create_project(d.path(), "My Project!", &files).is_err());
        // and never outside the project
        for bad in ["../x", "/abs", "a/../../b", ""] {
            let mut f = std::collections::BTreeMap::new();
            f.insert(bad.to_string(), "x".to_string());
            assert!(create_project(d.path(), "safe", &f).is_err(), "{bad}");
            assert!(!d.path().join("safe").exists(), "nothing is created before the paths are checked");
        }
        assert_eq!(identifier("Hello, World 2"), "hello-world-2");
        assert_eq!(identifier("---"), "project");
    }
}
