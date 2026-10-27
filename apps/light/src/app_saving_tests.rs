use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture { root: PathBuf, file: PathBuf, next: PathBuf }

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("light-save-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let file = root.join("first/main.txt");
        let next = root.join("second");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&next).unwrap();
        std::fs::write(&file, "original").unwrap();
        Self { root, file, next }
    }

    fn app(&self) -> App {
        let mut app = App::new(Settings::default());
        app.open_project(self.file.parent().unwrap());
        app.open_file(&self.file);
        app.docs[0].ed.select_all();
        app.docs[0].ed.insert_text_raw("important changes");
        app
    }

    fn fail_saving(&self) {
        std::fs::remove_file(&self.file).unwrap();
        std::fs::create_dir(&self.file).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); }
}

#[test]
fn failed_saves_keep_documents_when_closing_all_exiting_or_changing_projects() {
    for command in ["file.close_all", "app.exit", "switch", "close_project"] {
        let fixture = Fixture::new();
        let mut app = fixture.app();
        fixture.fail_saving();
        match command {
            "switch" => app.open_project(&fixture.next),
            "close_project" => app.close_project(),
            _ => app.run_command(&egui::Context::default(), command),
        }
        assert!(app.has_unsaved(), "{command}");
        assert_eq!(app.docs[0].ed.text(), "important changes", "{command}");
        assert!(!app.exit_now, "{command}");
        assert_eq!(app.project_root(), fixture.file.parent(), "{command}");
        assert!(matches!(app.popup, Popup::Confirm { .. }), "{command}");
    }
}

#[test]
fn conflicts_require_a_decision_before_switching_and_can_be_saved_explicitly() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    std::fs::write(&fixture.file, "external version").unwrap();
    app.docs[0].conflict = true;
    app.open_project(&fixture.next);
    assert!(matches!(&app.popup, Popup::Confirm { kind: ConfirmKind::OpenProject(path), .. } if path == &fixture.next));
    assert_eq!(std::fs::read_to_string(&fixture.file).unwrap(), "external version");
    assert!(app.save_unsaved_docs());
    app.finish_open_project(&fixture.next);
    assert_eq!(app.project_root(), Some(fixture.next.as_path()));
    assert_eq!(std::fs::read_to_string(&fixture.file).unwrap(), "important changes");
}

#[test]
fn closing_switching_and_autosave_check_external_changes_before_the_periodic_disk_poll() {
    for action in ["switch", "close_tab", "autosave"] {
        let fixture = Fixture::new();
        let mut app = fixture.app();
        std::fs::write(&fixture.file, "external update").unwrap();
        match action {
            "switch" => app.open_project(&fixture.next),
            "close_tab" => app.request_close_doc(0),
            _ => {
                app.now = 10.0;
                app.docs[0].autosave_rev = app.docs[0].ed.revision();
                app.autosave_tick();
            }
        }
        assert!(app.docs[0].conflict, "{action}");
        assert_eq!(app.project_root(), fixture.file.parent(), "{action}");
        assert_eq!(std::fs::read_to_string(&fixture.file).unwrap(), "external update", "{action}");
        if action != "autosave" { assert!(matches!(app.popup, Popup::Confirm { .. }), "{action}"); }
    }
}

#[test]
fn ordinary_switching_saves_changes_and_explicit_discard_preserves_external_text() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.open_project(&fixture.next);
    assert!(!app.has_unsaved());
    assert_eq!(std::fs::read_to_string(&fixture.file).unwrap(), "important changes");

    app.open_project(fixture.file.parent().unwrap());
    app.open_file(&fixture.file);
    app.docs[0].ed.insert_text_raw("discard this");
    app.docs[0].conflict = true;
    std::fs::write(&fixture.file, "external").unwrap();
    app.open_project(&fixture.next);
    app.finish_open_project(&fixture.next);
    assert!(app.docs.is_empty());
    assert_eq!(std::fs::read_to_string(&fixture.file).unwrap(), "external");
}

#[test]
fn failed_save_as_keeps_the_original_document_path_and_edits() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let doc = &mut app.docs[0];
    assert!(doc.save_as(&fixture.root.join("missing/main.py")).is_err());
    assert_eq!(doc.path.as_deref(), Some(fixture.file.as_path()));
    assert!(doc.ed.is_modified());
    assert_eq!(std::fs::read_to_string(&fixture.file).unwrap(), "original");
}
