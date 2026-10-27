use super::*;

struct SessionFixture { root: PathBuf, file: PathBuf, next: PathBuf }

impl SessionFixture {
    fn new() -> Self {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("light-session-{}-{unique}", std::process::id()));
        let file = root.join("first/main.txt");
        let next = root.join("second");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&next).unwrap();
        std::fs::write(&file, "original").unwrap();
        Self { root, file, next }
    }
}

impl Drop for SessionFixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); }
}

#[test]
fn projects_restore_tabs_active_file_and_expanded_folders_on_reopen() {
    let fixture = SessionFixture::new();
    let root = fixture.file.parent().unwrap();
    let folder = root.join("nested");
    std::fs::create_dir(&folder).unwrap();
    let second = folder.join("second.txt");
    std::fs::write(&second, "second").unwrap();
    let mut app = App::new(Settings::default());
    app.open_project(root);
    app.open_file(&fixture.file);
    app.open_file(&second);
    app.open_file(&fixture.file);
    app.project.as_mut().unwrap().expanded.insert(folder.clone());
    app.close_project();
    app.open_project(root);
    assert_eq!(app.docs.iter().filter_map(|d| d.path.clone()).collect::<Vec<_>>(), vec![fixture.file.clone(), second.clone()]);
    assert_eq!(app.doc().unwrap().path.as_ref(), Some(&fixture.file));
    assert!(app.project.as_ref().unwrap().expanded.contains(&folder));
    app.save_settings();
    let mut restarted = App::new(app.settings.clone());
    restarted.open_project(root);
    assert_eq!(restarted.docs.len(), 2);
    assert_eq!(restarted.doc().unwrap().path.as_ref(), Some(&fixture.file));
    assert!(restarted.project.as_ref().unwrap().expanded.contains(&folder));
    restarted.open_project(&fixture.next);
    assert!(restarted.docs.is_empty());
    restarted.open_project(root);
    assert_eq!(restarted.docs.len(), 2);
}

#[test]
fn a_file_opened_again_comes_back_where_it_was_left() {
    let fixture = SessionFixture::new();
    let root = fixture.file.parent().unwrap();
    let long: String = (0..400).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&fixture.file, &long).unwrap();
    let other = root.join("other.txt");
    std::fs::write(&other, "x").unwrap();
    let mut app = App::new(Settings::default());
    app.open_project(root);
    app.open_file(&fixture.file);
    let caret = long.find("line 250").unwrap();
    app.doc_mut().unwrap().ed.set_caret(caret, false);
    app.doc_mut().unwrap().view.top_line_f = 240.0;
    app.close_doc(app.active);
    app.open_file(&other);
    // opened again: the caret and the top line it had
    app.open_file(&fixture.file);
    let d = app.doc().unwrap();
    assert_eq!(d.ed.caret(), caret);
    assert_eq!(d.view.scroll_to_line_f, Some(240.0));
    // and after a restart, from the project's session
    app.save_settings();
    let mut restarted = App::new(app.settings.clone());
    restarted.open_project(root);
    let i = restarted.docs.iter().position(|d| d.path.as_ref() == Some(&fixture.file)).unwrap();
    assert_eq!(restarted.docs[i].ed.caret(), caret);
    assert_eq!(restarted.docs[i].view.scroll_to_line_f, Some(240.0));
    // a file that became shorter in the meantime does not put the caret past its end
    std::fs::write(&fixture.file, "short").unwrap();
    restarted.close_doc(i);
    restarted.open_file(&fixture.file);
    assert_eq!(restarted.doc().unwrap().ed.caret(), 5);
}
