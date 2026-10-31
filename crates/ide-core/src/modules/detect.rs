//! Which modules a project needs: its files say so (`*.py` -> Python, `Makefile` -> C and C++).

use std::collections::HashSet;
use std::path::Path;

use ignore::WalkBuilder;

use super::manifest::Detect;

/// Files looked at at most (a huge tree must not make opening a project slow).
const MAX_FILES: usize = 20_000;

/// The extensions and file names found in `root` (lower case), without what `.gitignore` hides.
pub fn scan(root: &Path) -> (HashSet<String>, HashSet<String>) {
    let (mut exts, mut names) = (HashSet::new(), HashSet::new());
    // Dotfiles such as .env.example and .editorconfig identify languages too.
    // Keep skipping hidden directories so caches and VCS internals are not scanned.
    let walker = WalkBuilder::new(root).max_depth(Some(8)).hidden(false).git_ignore(true).git_global(false)
        .filter_entry(|e| e.depth() == 0 || !e.file_type().is_some_and(|t| t.is_dir()) || !e.file_name().to_string_lossy().starts_with('.'))
        .build();
    for (n, e) in walker.flatten().filter(|e| e.file_type().is_some_and(|t| t.is_file())).enumerate() {
        if n >= MAX_FILES {
            break;
        }
        let p = e.path();
        if let Some(name) = p.file_name() {
            names.insert(name.to_string_lossy().to_ascii_lowercase());
        }
        if let Some(ext) = p.extension() {
            exts.insert(ext.to_string_lossy().to_ascii_lowercase());
        }
    }
    (exts, names)
}

/// The extensions and file names (lower case) of a list of files.
pub fn scan_files<'a>(files: impl IntoIterator<Item = &'a Path>) -> (HashSet<String>, HashSet<String>) {
    let (mut exts, mut names) = (HashSet::new(), HashSet::new());
    for p in files {
        if let Some(name) = p.file_name() {
            names.insert(name.to_string_lossy().to_ascii_lowercase());
        }
        if let Some(ext) = p.extension() {
            exts.insert(ext.to_string_lossy().to_ascii_lowercase());
        }
    }
    (exts, names)
}

/// The ids of the modules (given as `(id, how to detect it)`, in this order) whose extensions or files are in `root`.
pub fn needed(root: &Path, modules: &[(String, Detect)]) -> Vec<String> {
    let (exts, names) = scan(root);
    needed_in(&exts, &names, modules)
}

/// The same from a scan (see [`scan`] and [`scan_files`]).
pub fn needed_in(exts: &HashSet<String>, names: &HashSet<String>, modules: &[(String, Detect)]) -> Vec<String> {
    modules
        .iter()
        .filter(|(_, d)| d.extensions.iter().any(|e| exts.contains(&e.to_ascii_lowercase())) || d.files.iter().any(|f| if f.contains('*') { names.iter().any(|name| light_lang::matches_file_name(f, name)) } else { names.contains(&f.to_ascii_lowercase()) }))
        .map(|(id, _)| id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn detect(exts: &[&str], files: &[&str]) -> Detect {
        Detect { extensions: exts.iter().map(|s| s.to_string()).collect(), files: files.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn files_say_which_modules_a_project_needs() {
        let d = TempDir::new("detect");
        std::fs::create_dir_all(d.path().join("src/deep")).unwrap();
        std::fs::write(d.path().join("src/deep/tool.PY"), "").unwrap();
        std::fs::write(d.path().join("Makefile"), "").unwrap();
        std::fs::write(d.path().join("README.md"), "").unwrap();
        let mods = vec![
            ("python".to_string(), detect(&["py"], &["pyproject.toml"])),
            ("c-cpp".to_string(), detect(&["c", "h"], &["makefile", "CMakeLists.txt"])),
            ("lisp".to_string(), detect(&["lisp", "asd"], &[])),
            ("markdown".to_string(), detect(&["md"], &[])),
            ("assembler".to_string(), detect(&["asm"], &[])),
        ];
        assert_eq!(needed(d.path(), &mods), ["python", "c-cpp", "markdown"], "any case, any depth, in the order given");
        assert!(needed(&d.path().join("empty"), &mods).is_empty());
    }

    #[test]
    fn dotfiles_identify_modules_without_scanning_hidden_directories() {
        let d = TempDir::new("detect-dotfiles");
        std::fs::write(d.path().join(".env.example"), "PORT=8080\n").unwrap();
        std::fs::write(d.path().join(".editorconfig"), "root = true\n").unwrap();
        std::fs::create_dir(d.path().join(".cache")).unwrap();
        std::fs::write(d.path().join(".cache/generated.py"), "").unwrap();
        let mods = vec![("dotenv".to_string(), detect(&["env"], &[".env", ".env.*"])), ("ini".to_string(), detect(&["ini"], &[".editorconfig"])), ("python".to_string(), detect(&["py"], &[]))];
        assert_eq!(needed(d.path(), &mods), ["dotenv", "ini"]);
    }

    #[test]
    fn ignored_files_do_not_count() {
        let d = TempDir::new("detect-ignore");
        std::process::Command::new("git").args(["init", "-q"]).current_dir(d.path()).output().ok();
        std::fs::write(d.path().join(".gitignore"), "build/\n").unwrap();
        std::fs::create_dir_all(d.path().join("build")).unwrap();
        std::fs::write(d.path().join("build/gen.c"), "").unwrap();
        std::fs::write(d.path().join("main.lisp"), "").unwrap();
        let mods = vec![("c-cpp".to_string(), detect(&["c"], &[])), ("lisp".to_string(), detect(&["lisp"], &[]))];
        if d.path().join(".git").exists() {
            assert_eq!(needed(d.path(), &mods), ["lisp"]);
        }
    }
}
