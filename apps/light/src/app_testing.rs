//! Tests in the editor: a run button next to every test and suite a test framework of a module finds in the file
//! ([`ide_core::testing`]), the run in a Run tab of its own, and the outcome of each test on its button afterwards.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ide_core::modules::TestDef;
use ide_core::testing::{self, TestItem};

use crate::app::App;
use crate::app_run::{RunRequest, shell_launch, shell_path};
use crate::editor_view::{GutterIcon, Sev};
use crate::gutter::GutterAction;
use crate::i18n::{self, tr};
use crate::icons::Icon;

/// The tests of a file and the framework that runs them.
#[derive(Clone, Debug)]
pub struct TestFile {
    pub def: TestDef,
    pub items: Vec<TestItem>,
}

#[derive(Default)]
pub struct TestingState {
    /// Per document id: the revision, the project files and the modules it was worked out for, and what was found.
    cache: HashMap<u64, (u64, String, Option<TestFile>)>,
    /// The last outcome of each test: (file, full name) -> passed.
    pub results: HashMap<(PathBuf, String), bool>,
}

impl App {
    fn test_defs(&self) -> Vec<TestDef> {
        self.mods.installed.iter().filter(|m| m.enabled).flat_map(|m| m.manifest.tests.iter().cloned()).collect()
    }

    /// The framework that wants the file `path` of language `language` with this text, and the folder its tests run in.
    fn test_framework(&self, path: &Path, language: &str, text: &str) -> Option<(TestDef, PathBuf)> {
        let root = self.project_root().map(Path::to_path_buf);
        let rel = root.as_ref().and_then(|r| path.strip_prefix(r).ok()).unwrap_or(path).to_string_lossy().replace('\\', "/");
        for def in self.test_defs().into_iter().filter(|d| d.languages.iter().any(|l| l == language)) {
            if !testing::wants(&def, &rel, text) {
                continue;
            }
            // the nearest project file at or above the file's folder, inside the project
            let mut found = None;
            if !def.project_file.is_empty() {
                let mut dir = path.parent();
                while let Some(d) = dir {
                    let candidate = d.join(&def.project_file);
                    if candidate.is_file() {
                        found = Some((d.to_path_buf(), std::fs::read_to_string(&candidate).unwrap_or_default()));
                        break;
                    }
                    if root.as_ref().is_some_and(|r| d == r.as_path()) {
                        break;
                    }
                    dir = d.parent();
                }
            }
            if !def.project_needs.is_empty() && !found.as_ref().is_some_and(|(_, text)| text.contains(&def.project_needs)) {
                continue;
            }
            let run_in = found.map(|(d, _)| d).or_else(|| root.clone()).or_else(|| path.parent().map(Path::to_path_buf)).unwrap_or_default();
            return Some((def, run_in));
        }
        None
    }

    /// The tests of document `idx`, worked out again when its text or the project's files change.
    pub fn tests_of(&mut self, idx: usize) -> Option<TestFile> {
        let doc = self.docs.get(idx)?;
        let (id, rev) = (doc.id, doc.ed.revision());
        let files_gen = format!("{}|{}", self.project.as_ref().map_or(0, |p| p.files_gen), self.mods.installed.iter().filter(|m| m.enabled).map(|m| format!("{}@{}", m.manifest.id, m.manifest.version)).collect::<Vec<_>>().join(","));
        if let Some((r, g, found)) = self.testing.cache.get(&id)
            && *r == rev
            && *g == files_gen
        {
            return found.clone();
        }
        let found = match (doc.path.as_ref(), doc.ed.language(), doc.diff.is_none()) {
            (Some(path), Some(language), true) => self.test_framework(path, language.id(), doc.ed.text()).map(|(def, _)| TestFile { items: testing::find(&def, doc.ed.text()), def }),
            _ => None,
        };
        self.testing.cache.insert(id, (rev, files_gen, found.clone()));
        found
    }

    /// Has `item` of `path` passed (`Some(true)`), failed, or not run yet? A suite has passed when all its tests did and
    /// failed when one did.
    fn test_status(&self, path: &Path, file: &TestFile, item: &TestItem) -> Option<bool> {
        if !item.suite {
            return self.testing.results.get(&(path.to_path_buf(), item.full.clone())).copied();
        }
        let prefix = format!("{}{}", item.full, if file.def.separator.is_empty() { " " } else { &file.def.separator });
        let children: Vec<Option<bool>> = file.items.iter().filter(|i| !i.suite && i.full.starts_with(&prefix)).map(|i| self.testing.results.get(&(path.to_path_buf(), i.full.clone())).copied()).collect();
        if children.contains(&Some(false)) {
            Some(false)
        } else if !children.is_empty() && children.iter().all(|c| *c == Some(true)) {
            Some(true)
        } else {
            None
        }
    }

    pub fn test_icons(&mut self, idx: usize) -> Vec<GutterIcon> {
        let Some(file) = self.tests_of(idx) else { return vec![] };
        let Some(path) = self.docs[idx].path.clone() else { return vec![] };
        let pal = self.pal;
        file.items.iter().filter(|i| i.runnable()).map(|item| {
            let (icon, color) = match self.test_status(&path, &file, item) {
                Some(true) => (Icon::Ok, pal.ok),
                Some(false) => (Icon::Error, pal.error),
                None => (Icon::Play, pal.ok),
            };
            GutterIcon { line: item.line, icon, color, tip: i18n::fmt("Run '{}'", &[&item.full]) }
        }).collect()
    }

    pub fn test_actions(&mut self, idx: usize, line: usize) -> Vec<(Icon, String, GutterAction)> {
        let Some(file) = self.tests_of(idx) else { return vec![] };
        let Some(path) = self.docs[idx].path.clone() else { return vec![] };
        let Some(item) = file.items.iter().find(|i| i.line == line && i.runnable()) else { return vec![] };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        vec![
            (Icon::Play, i18n::fmt("Run '{}'", &[&item.name]), GutterAction::RunTests(path.clone(), Some(item.clone()))),
            (Icon::Play, i18n::fmt("Run all tests in '{}'", &[&name]), GutterAction::RunTests(path, None)),
        ]
    }

    /// Run `item` of the test file `path` (or all of its tests) in a Run tab of its own.
    pub fn run_tests(&mut self, path: &Path, item: Option<&TestItem>) {
        self.save_all();
        let text = match self.docs.iter().find(|d| d.path.as_deref() == Some(path)) {
            Some(d) => d.ed.text().to_string(),
            None => std::fs::read_to_string(path).unwrap_or_default(),
        };
        let language = self.language_for(Some(path), text.lines().next()).map(|l| l.id().to_string()).unwrap_or_default();
        let Some((def, root)) = self.test_framework(path, &language, &text) else {
            self.toast(tr("No test framework of the modules that are on runs this file"), Sev::Weak);
            return;
        };
        // the test may have moved or been renamed since the button was drawn: it is looked up by its full name
        let item = match item {
            Some(wanted) => match testing::find(&def, &text).into_iter().find(|i| i.full == wanted.full && i.suite == wanted.suite) {
                Some(i) => Some(i),
                None => {
                    self.toast(i18n::fmt("The test '{}' is not in the file any more", &[&wanted.full]), Sev::Weak);
                    return;
                }
            },
            None => None,
        };
        let t = self.wsl_target();
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let line = testing::command_line(&def, item.as_ref(), &shell_path(path, t.as_ref()), &shell_path(&dir, t.as_ref()), &shell_path(&root, t.as_ref()), t.is_none());
        let env: Vec<(String, String)> = def.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let launch = shell_launch(&line, &root, t.as_ref(), &env);
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = format!("{}: {}", def.name, item.as_ref().map_or(file_name, |i| i.name.clone()));
        let key = format!("test:{}:{}", path.display(), item.as_ref().map_or("*", |i| i.full.as_str()));
        if self.start_run(key, title.clone(), launch, &title, root, t) {
            self.run_request = Some(RunRequest::Test(Box::new(def), path.to_path_buf(), item));
        }
    }

    /// A test run ended with `code`: what its output says goes to the buttons of the tests it ran.
    pub fn tests_finished(&mut self, code: Option<i32>) {
        let Some(RunRequest::Test(def, path, item)) = self.run_request.clone() else { return };
        let output: String = self.run.log.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        let outcome = testing::outcome(&def, &output);
        let text = match self.docs.iter().find(|d| d.path.as_deref() == Some(path.as_path())) {
            Some(d) => d.ed.text().to_string(),
            None => std::fs::read_to_string(&path).unwrap_or_default(),
        };
        let sep = if def.separator.is_empty() { " ".to_string() } else { def.separator.clone() };
        let in_scope = |i: &TestItem| !i.suite && match &item {
            None => true,
            Some(s) if s.suite => i.full.starts_with(&format!("{}{sep}", s.full)),
            Some(t) => i.full == t.full,
        };
        let scope: Vec<TestItem> = testing::find(&def, &text).into_iter().filter(in_scope).collect();
        let (mut passed, mut failed) = (0, 0);
        for test in &scope {
            let key = (path.clone(), test.full.clone());
            let status = if outcome.failed.iter().any(|n| testing::is_named(&def, test, n)) {
                Some(false)
            } else if outcome.passed.iter().any(|n| testing::is_named(&def, test, n)) || code == Some(0) {
                Some(true)
            } else if def.passed.is_empty() && !outcome.failed.is_empty() {
                // the framework names only the failures: the others of the run passed
                Some(true)
            } else if scope.len() == 1 && code.is_some() {
                Some(false)
            } else {
                None
            };
            match status {
                Some(ok) => {
                    if ok { passed += 1 } else { failed += 1 }
                    self.testing.results.insert(key, ok);
                }
                None => {
                    self.testing.results.remove(&key);
                }
            }
        }
        self.status = i18n::fmt("Tests: {0} passed, {1} failed", &[&passed, &failed]);
    }
}
