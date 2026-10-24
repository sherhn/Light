//! Splitting the editor area into two panes, each showing one of the open documents.

use crate::app::App;
use crate::editor_view::ViewState;
use crate::state::Split;

impl App {
    /// Index of the document shown in the pane that does not have the focus.
    pub fn split_other_index(&self) -> Option<usize> {
        let sp = self.split.as_ref()?;
        self.docs.iter().position(|d| d.id == sp.other)
    }

    /// Drop the split when its second document is gone (tab closed, project switched).
    pub fn fix_split(&mut self) {
        if self.split.is_some() && (self.docs.is_empty() || self.split_other_index().is_none()) {
            self.split = None;
        }
    }

    /// Split the editor area (side by side or stacked). A new split shows the current document in both panes and
    /// focuses the new one; pick another tab to look at two files at once.
    pub fn split_editor(&mut self, vertical: bool) {
        let Some(d) = self.docs.get(self.active) else { return };
        match self.split.as_mut() {
            Some(sp) => sp.vertical = vertical,
            None => {
                let mut view = ViewState::default();
                view.scroll = d.view.scroll;
                view.scroll_to_caret = true;
                self.split = Some(Split { vertical, other: d.id, second_focused: true, view });
            }
        }
    }

    pub fn unsplit(&mut self) {
        self.split = None;
    }

    /// Move the keyboard focus to the other pane.
    pub fn focus_other_pane(&mut self) {
        let Some(o) = self.split_other_index() else { return };
        let a = self.active;
        let active_id = self.docs[a].id;
        let Some(sp) = self.split.as_mut() else { return };
        // both panes on one document: the scroll positions belong to the panes, so they trade places too
        if self.docs[o].id == active_id {
            std::mem::swap(&mut self.docs[a].view, &mut sp.view);
        }
        sp.other = active_id;
        sp.second_focused = !sp.second_focused;
        self.active = o;
        self.editor_focus = true;
        self.completion.active = false;
    }

    /// From the diff tab: open its file in the other pane and keep the diff on screen, so the file can be edited
    /// while its changes stay visible (the diff updates as you type).
    pub fn diff_edit_beside(&mut self) {
        let Some(t) = self.active_diff() else { return };
        let abs = t.abs.clone();
        if !abs.is_file() {
            return;
        }
        let diff_id = self.docs[self.active].id;
        if !self.open_file(&abs) {
            return;
        }
        // open_file made the file's tab active; the diff goes to the first pane, the file to the second
        self.split = Some(Split { vertical: true, other: diff_id, second_focused: true, view: ViewState::default() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_core::settings::Settings;

    fn app_with(tag: &str, files: &[&str]) -> (App, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("light-split-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(Settings::default());
        for f in files {
            let p = dir.join(f);
            std::fs::write(&p, format!("(print \"{f}\")\n")).unwrap();
            assert!(app.open_file(&p));
        }
        (app, dir)
    }

    #[test]
    fn split_shows_the_same_document_twice_and_focuses_the_new_pane() {
        let (mut app, dir) = app_with("same", &["a.lisp", "b.lisp"]);
        app.split_editor(true);
        let sp = app.split.as_ref().unwrap();
        assert!(sp.vertical && sp.second_focused);
        assert_eq!(app.split_other_index(), Some(app.active));
        app.split_editor(false);
        assert!(!app.split.as_ref().unwrap().vertical, "splitting again only turns the layout");
        app.unsplit();
        assert!(app.split.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tabs_fill_the_focused_pane_and_the_other_tab_moves_focus() {
        let (mut app, dir) = app_with("tabs", &["a.lisp", "b.lisp", "c.lisp"]);
        app.show_doc(0);
        app.split_editor(true); // both panes: a
        app.show_doc(1); // focused pane now b, the other one stays a
        assert_eq!((app.active, app.split_other_index()), (1, Some(0)));
        app.show_doc(0); // the tab of the other pane's document focuses that pane
        assert_eq!((app.active, app.split_other_index()), (0, Some(1)));
        assert!(!app.split.as_ref().unwrap().second_focused, "the focus went back to the first pane");
        app.show_doc(2);
        assert_eq!((app.active, app.split_other_index()), (2, Some(1)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn closing_the_other_document_ends_the_split_and_ids_survive_index_shifts() {
        let (mut app, dir) = app_with("close", &["a.lisp", "b.lisp", "c.lisp"]);
        app.show_doc(2);
        app.split_editor(true);
        app.show_doc(1); // other = c (index 2)
        app.close_doc(0); // indices shift: c is now 1, b is 0
        app.fix_split();
        assert_eq!(app.split_other_index(), Some(1), "the pane still points at c");
        assert_eq!(app.docs[app.active].name(), "b.lisp");
        app.close_doc(1);
        app.fix_split();
        assert!(app.split.is_none(), "closing the other pane's document unsplits");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_document_in_both_panes_trades_scroll_state_with_the_focus() {
        let (mut app, dir) = app_with("scroll", &["a.lisp"]);
        app.docs[0].view.scroll.y = 120.0;
        app.split_editor(true);
        assert_eq!(app.split.as_ref().unwrap().view.scroll.y, 120.0, "the new pane starts where the old one is");
        app.split.as_mut().unwrap().view.scroll.y = 40.0;
        app.focus_other_pane();
        assert_eq!(app.docs[0].view.scroll.y, 40.0);
        assert_eq!(app.split.as_ref().unwrap().view.scroll.y, 120.0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
