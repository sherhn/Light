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
