//! Pictures in tabs, SVG next to its code, and the files `.gitignore` names having no change markers.

use super::*;
use std::process::Command;

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("light-{tag}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        Dir(root)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(w, h, image::Rgba([20, 120, 220, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#f00"/></svg>"##;

#[test]
fn a_raster_image_opens_in_a_picture_tab_that_is_never_written() {
    let d = Dir::new("picture-tab");
    let file = d.0.join("logo.png");
    let bytes = png(30, 20);
    std::fs::write(&file, &bytes).unwrap();
    let mut app = App::new(Settings::default());
    app.open_project(&d.0);
    assert!(app.open_file(&file));
    assert_eq!(app.docs.len(), 1);
    let doc = &app.docs[0];
    assert!(doc.is_picture_tab() && doc.diff.is_some(), "a page: the editor commands stay off it");
    assert_eq!(doc.name(), "logo.png");
    assert_eq!(doc.path.as_deref(), Some(file.as_path()));
    assert_eq!(doc.picture.as_ref().unwrap().natural(), Some([30.0, 20.0]));
    assert!(!doc.ed.is_modified());
    // opened again: the same tab
    assert!(app.open_file(&file));
    assert_eq!(app.docs.len(), 1);
    // saving, in every way, leaves the picture as it was
    app.save_all();
    assert!(app.save_doc(0));
    assert!(app.flush_saves() == 0);
    app.docs[0].save().unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    assert!(!app.has_unsaved());
    assert!(app.recent_files.contains(&file));
}

#[test]
fn a_picture_tab_reads_the_file_again_when_it_changes() {
    let d = Dir::new("picture-reload");
    let file = d.0.join("a.png");
    std::fs::write(&file, png(10, 10)).unwrap();
    let mut app = App::new(Settings::default());
    app.open_file(&file);
    app.docs[0].picture.as_mut().unwrap().zoom = Some(3.0);
    std::fs::write(&file, png(50, 40)).unwrap();
    app.docs[0].disk_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
    assert!(app.docs[0].sync_with_disk());
    let view = app.docs[0].picture.as_ref().unwrap();
    assert_eq!(view.natural(), Some([50.0, 40.0]));
    assert_eq!(view.zoom, Some(3.0), "where it was zoomed stays");
    // the file goes away: the tab says so instead of showing a stale picture
    std::fs::remove_file(&file).unwrap();
    app.docs[0].disk_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
    app.docs[0].sync_with_disk();
    assert!(app.docs[0].picture.as_ref().unwrap().error.is_some());
}

#[test]
fn a_file_that_is_not_a_picture_still_opens_with_the_reason() {
    let d = Dir::new("picture-fake");
    let file = d.0.join("empty.png");
    std::fs::write(&file, b"").unwrap();
    let mut app = App::new(Settings::default());
    assert!(app.open_file(&file));
    assert!(app.docs[0].picture.as_ref().unwrap().error.is_some());
    assert!(app.docs[0].picture.as_ref().unwrap().natural().is_none());
}

#[test]
fn a_picture_that_is_too_large_is_refused_with_a_message() {
    let d = Dir::new("picture-big");
    let file = d.0.join("huge.bmp");
    std::fs::File::create(&file).unwrap().set_len(crate::imageview::MAX_PICTURE_BYTES + 1).unwrap();
    let mut app = App::new(Settings::default());
    assert!(!app.open_file(&file));
    assert!(app.docs.is_empty());
    assert!(app.toasts.iter().any(|t| t.text.contains("too large")));
}

#[test]
fn picture_tabs_come_back_with_the_session() {
    let d = Dir::new("picture-session");
    let (file, text) = (d.0.join("a.png"), d.0.join("b.txt"));
    std::fs::write(&file, png(8, 8)).unwrap();
    std::fs::write(&text, "text").unwrap();
    let mut app = App::new(Settings::default());
    app.open_project(&d.0);
    app.open_file(&text);
    app.open_file(&file);
    app.close_project();
    app.open_project(&d.0);
    let names: Vec<_> = app.docs.iter().map(|d| d.name()).collect();
    assert_eq!(names, ["b.txt", "a.png"]);
    assert!(app.docs[1].is_picture_tab() && !app.docs[0].is_picture_tab());
}

#[test]
fn an_svg_is_text_with_a_preview_and_other_files_have_none() {
    let d = Dir::new("svg-doc");
    let (svg, upper, txt) = (d.0.join("a.svg"), d.0.join("B.SVG"), d.0.join("a.txt"));
    for f in [&svg, &upper] {
        std::fs::write(f, SVG).unwrap();
    }
    std::fs::write(&txt, SVG).unwrap();
    let mut app = App::new(Settings::default());
    for f in [&svg, &upper, &txt] {
        assert!(app.open_file(f));
    }
    assert!(!app.docs[0].is_picture_tab(), "SVG is code: it opens in the editor");
    assert_eq!(app.docs[0].ed.text(), SVG);
    assert_eq!(app.doc_preview(0), Some(crate::ui_preview::PreviewKind::Svg));
    assert_eq!(app.doc_preview(1), Some(crate::ui_preview::PreviewKind::Svg));
    assert_eq!(app.doc_preview(2), None, "only the extension makes an SVG");
    // the same three modes as Markdown, and the same default
    assert_eq!(app.preview_mode(0), crate::preview::PreviewMode::Split);
    assert_eq!(app.preview_mode(2), crate::preview::PreviewMode::Code);
    app.set_preview_mode(0, crate::preview::PreviewMode::Preview);
    assert_eq!(app.preview_mode(0), crate::preview::PreviewMode::Preview);
    assert!(app.editor_hidden() || app.active != 0);
    app.set_preview_mode(2, crate::preview::PreviewMode::Code);
    assert_eq!(app.settings.markup_preview, "preview", "a file without a preview does not change the default");
}

// ---------------------------------------------------------------------- .gitignore and the markers

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").args(["-c", "user.email=t@example.com", "-c", "user.name=T", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"]).args(args).current_dir(dir).output().expect("git is installed");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn marks(app: &mut App, file: &Path) -> usize {
    app.open_file(file);
    app.git_update_bases();
    let i = app.docs.iter().position(|d| d.path.as_deref() == Some(file)).unwrap();
    App::git_marks(&mut app.docs[i]).len()
}

fn repo_with_ignore_rules() -> (Dir, App) {
    let d = Dir::new("ignored-marks");
    let w = |name: &str, text: &str| {
        let p = d.0.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    w(".gitignore", "*.log\n/out/\n");
    w("tracked.txt", "one\ntwo\nthree\n");
    w("forced.log", "kept\nby force\n");
    git(&d.0, &["init", "-q", "-b", "main"]);
    git(&d.0, &["add", "-A"]);
    git(&d.0, &["add", "-f", "forced.log"]);
    git(&d.0, &["commit", "-q", "-m", "base"]);
    w("new.txt", "a\nb\nc\n");
    w("debug.log", "x\ny\nz\n");
    w("out/app.bin", "p\nq\n");
    w("tracked.txt", "one\n2\nthree\n");
    w("forced.log", "kept\nchanged\n");
    let mut app = App::new(Settings::default());
    app.open_project(&d.0);
    assert!(app.git.repo.is_some(), "the project is a git repository");
    (d, app)
}

#[test]
fn files_that_gitignore_names_show_no_changes_but_other_new_files_do() {
    let (d, mut app) = repo_with_ignore_rules();
    assert_eq!(marks(&mut app, &d.0.join("debug.log")), 0, "`*.log`");
    assert_eq!(marks(&mut app, &d.0.join("out/app.bin")), 0, "`/out/`");
    assert!(marks(&mut app, &d.0.join("new.txt")) >= 3, "a new file that is not ignored is all new");
    assert_eq!(marks(&mut app, &d.0.join("tracked.txt")), 1, "a tracked file shows what changed");
    // git goes on tracking what was added by force, whatever the rules say
    assert_eq!(marks(&mut app, &d.0.join("forced.log")), 1);
    assert!(app.git_ignores(&d.0.join("debug.log")) && app.git_ignores(&d.0.join("out/app.bin")));
    assert!(!app.git_ignores(&d.0.join("new.txt")) && !app.git_ignores(&d.0.join("tracked.txt")) && !app.git_ignores(&d.0.join("forced.log")));
}

#[test]
fn editing_an_ignored_file_adds_no_markers() {
    let (d, mut app) = repo_with_ignore_rules();
    let log = d.0.join("debug.log");
    assert_eq!(marks(&mut app, &log), 0);
    let i = app.docs.iter().position(|d| d.path.as_deref() == Some(log.as_path())).unwrap();
    app.docs[i].ed.set_caret(0, false);
    app.docs[i].ed.type_text("more\n");
    assert_eq!(App::git_marks(&mut app.docs[i]).len(), 0);
}

#[test]
fn a_diff_of_an_ignored_file_is_not_offered() {
    let (d, mut app) = repo_with_ignore_rules();
    app.open_diff_tab("debug.log");
    assert!(app.docs.iter().all(|d| d.diff.is_none()), "no diff tab");
    assert!(app.toasts.iter().any(|t| t.text.contains("ignored by Git")));
    app.open_diff_tab("new.txt");
    assert_eq!(app.diff_current_rel(), Some("new.txt"));
    let _ = d;
}

#[test]
fn the_markers_follow_a_change_of_the_rules() {
    let (d, mut app) = repo_with_ignore_rules();
    let log = d.0.join("debug.log");
    assert_eq!(marks(&mut app, &log), 0);
    // the rule goes: the file is new now, and the open tab learns it when git is asked again
    std::fs::write(d.0.join(".gitignore"), "/out/\n").unwrap();
    let before = app.git.changes.iter().map(|c| c.path.clone()).collect::<HashSet<_>>();
    assert!(!before.contains("debug.log"));
    // git is not asked while the project is still being indexed
    for _ in 0..400 {
        app.poll_index();
        if app.project.as_ref().is_some_and(|p| p.index_rx.is_none()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    app.git.rx = None;
    app.git_refresh();
    for _ in 0..200 {
        app.now += 0.1;
        app.git_poll();
        if app.git.changes.iter().any(|c| c.path == "debug.log") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(app.git.changes.iter().any(|c| c.path == "debug.log"), "git now reports it");
    app.git_update_bases();
    let i = app.docs.iter().position(|d| d.path.as_deref() == Some(log.as_path())).unwrap();
    assert!(App::git_marks(&mut app.docs[i]).len() >= 3, "every line is new now");
}
