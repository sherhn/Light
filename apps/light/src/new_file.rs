//! What the New File dialog offers: a file of any name, or a file of a language that a module brings, whose extension
//! is added to the name typed.

use ide_core::modules::store::Installed;
use light_lang::LangDef;

/// A kind of file the New File dialog can make.
#[derive(Clone, Debug, PartialEq)]
pub struct FileKind {
    pub title: String,
    /// The extensions to choose from (without the dot), the usual one first; none for a file of any name.
    pub extensions: Vec<String>,
    /// Names its files have without an extension (`Makefile`, `Dockerfile`): typed as they are, nothing is added.
    pub filenames: Vec<String>,
}

impl FileKind {
    /// What the badge of the kind says: its usual extension (`PY`), or nothing for a file of any name.
    pub fn badge(&self) -> String {
        self.extensions.first().map(|e| e.chars().take(4).collect::<String>().to_uppercase()).unwrap_or_default()
    }
}

/// The extension a language's files are usually given, when its list starts with a rarer one (`cc` before `cpp`).
fn main_extension(lang: &LangDef) -> Option<String> {
    const USUAL: [&str; 3] = ["cpp", "yaml", "md"];
    lang.extensions.iter().find(|e| USUAL.contains(&e.to_lowercase().as_str())).or_else(|| lang.extensions.first()).map(|e| e.to_lowercase())
}

/// The kinds of file: any name first, then one per module that is on (Lisp too), by title. A module of several
/// languages offers the usual extension of each, and the header extensions of C and C++.
pub fn kinds(installed: &[Installed], lisp: bool) -> Vec<FileKind> {
    let mut modules: Vec<FileKind> = installed
        .iter()
        .filter(|m| m.enabled)
        .filter_map(|m| {
            let mut extensions: Vec<String> = Vec::new();
            for lang in &m.manifest.languages {
                let Some(main) = main_extension(lang) else { continue };
                extensions.push(main);
                for header in ["h", "hpp"] {
                    if lang.extensions.iter().any(|e| e.eq_ignore_ascii_case(header)) {
                        extensions.push(header.to_string());
                    }
                }
            }
            extensions.dedup();
            let mut seen = std::collections::HashSet::new();
            extensions.retain(|e| seen.insert(e.clone()));
            let filenames = m.manifest.languages.iter().flat_map(|l| l.filenames.iter()).filter(|f| !f.contains('*')).cloned().collect();
            (!extensions.is_empty()).then(|| FileKind { title: m.manifest.name.clone(), extensions, filenames })
        })
        .collect();
    if lisp && !modules.iter().any(|k| k.extensions.iter().any(|e| e == "lisp")) {
        modules.push(FileKind { title: "Lisp".into(), extensions: vec!["lisp".into(), "asd".into()], filenames: vec![] });
    }
    modules.sort_by_key(|k| k.title.to_lowercase());
    let mut all = vec![FileKind { title: String::new(), extensions: vec![], filenames: vec![] }];
    all.extend(modules);
    all
}

/// Does `name` need no extension: it has one already, starts with a dot, or is a name the kind's files have as it is?
fn complete(name: &str, filenames: &[String]) -> bool {
    let base = std::path::Path::new(name.trim()).file_name().and_then(|s| s.to_str()).unwrap_or(name);
    std::path::Path::new(base).extension().is_some() || base.starts_with('.') || filenames.iter().any(|f| f.eq_ignore_ascii_case(base))
}

/// The name the file is made with: `extension` is added unless the name needs none.
pub fn file_name(input: &str, extension: &str, filenames: &[String]) -> String {
    let name = input.trim();
    let extension = extension.trim().trim_start_matches('.');
    if extension.is_empty() || complete(name, filenames) { name.to_string() } else { format!("{name}.{extension}") }
}

/// What the name field shows, dimmed, after the text typed: the extension that will be added.
pub fn ghost(input: &str, extension: &str, filenames: &[String]) -> String {
    let extension = extension.trim().trim_start_matches('.');
    if extension.is_empty() || input.trim().is_empty() || complete(input, filenames) { String::new() } else { format!(".{extension}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_core::modules::manifest::Manifest;

    fn module(name: &str, langs: &[(&[&str], &[&str])], enabled: bool) -> Installed {
        let languages: Vec<serde_json::Value> = langs.iter().enumerate().map(|(i, (ext, names))| serde_json::json!({"id": format!("{name}{i}"), "name": name, "extensions": ext, "filenames": names})).collect();
        let id: String = name.to_lowercase().chars().filter(char::is_ascii_alphanumeric).collect();
        let json = serde_json::json!({"id": id, "name": name, "version": "1.0.0", "category": "Languages", "summary": "s", "languages": languages});
        Installed { manifest: Manifest::parse(&json.to_string()).unwrap(), dir: std::path::PathBuf::new(), enabled }
    }

    #[test]
    fn a_file_of_any_name_comes_first_then_each_module_that_is_on() {
        let installed = [
            module("Python", &[(&["py", "pyw"], &["Pipfile"])], true),
            module("Assembler", &[(&["asm", "nasm"], &[]), (&["s"], &[])], true),
            module("C and C++", &[(&["c", "h", "ld"], &[]), (&["cc", "cpp", "hpp"], &[])], true),
            module("CSS", &[(&["css"], &[])], false),
            module("Docker", &[(&["dockerfile"], &["Dockerfile", "Dockerfile.*"]), (&[], &["compose.yaml"])], true),
        ];
        let kinds = kinds(&installed, true);
        let titles: Vec<&str> = kinds.iter().map(|k| k.title.as_str()).collect();
        assert_eq!(titles, ["", "Assembler", "C and C++", "Docker", "Lisp", "Python"]);
        assert!(kinds[0].extensions.is_empty() && kinds[0].badge().is_empty());
        assert_eq!(kinds[1].extensions, ["asm", "s"]);
        assert_eq!(kinds[2].extensions, ["c", "h", "cpp", "hpp"]);
        assert_eq!(kinds[3].filenames, ["Dockerfile", "compose.yaml"]);
        assert_eq!(kinds[5].badge(), "PY");
        // without the Lisp module there is no Lisp
        assert!(!super::kinds(&installed, false).iter().any(|k| k.title == "Lisp"));
    }

    #[test]
    fn the_extension_is_added_unless_the_name_needs_none() {
        let names = ["Dockerfile".to_string()];
        assert_eq!(file_name(" main ", "py", &[]), "main.py");
        assert_eq!(file_name("src/app", ".py", &[]), "src/app.py");
        assert_eq!(file_name("app.pyw", "py", &[]), "app.pyw");
        assert_eq!(file_name(".env", "py", &[]), ".env");
        assert_eq!(file_name("dockerfile", "dockerfile", &names), "dockerfile");
        assert_eq!(file_name("web", "dockerfile", &names), "web.dockerfile");
        assert_eq!(file_name("notes", "", &[]), "notes");
        assert_eq!(ghost("main", "py", &[]), ".py");
        assert_eq!(ghost("main.py", "py", &[]), "");
        assert_eq!(ghost("", "py", &[]), "");
        assert_eq!(ghost("Dockerfile", "dockerfile", &names), "");
        assert_eq!(ghost("notes", "", &[]), "");
    }
}
