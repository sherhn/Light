//! Explicit Tab completion. Directory I/O and WSL home lookup stay off the UI thread.
use std::{path::PathBuf,sync::{Arc,Mutex}};
use eframe::egui::{self,Id,Key,Modifiers};
use ide_core::termcomplete::{self,PathEdit};

#[derive(Clone,Debug,PartialEq,Eq)]
pub struct Target { pub cwd:PathBuf, pub distro:Option<String>, pub directories:bool, terminal:bool }
impl Target {
    pub fn local(directories:bool)->Self { Self {cwd:std::env::current_dir().unwrap_or_else(|_|PathBuf::from(".")),distro:None,directories,terminal:false} }
    pub fn linux(distro:String,directories:bool)->Self {
        let cwd=if cfg!(windows) {ide_core::wsl::windows_path(&distro,"/")}else{PathBuf::from("/")};
        Self {cwd,distro:Some(distro),directories,terminal:false}
    }
    pub fn at(mut self,cwd:PathBuf)->Self {self.cwd=cwd;self}
    pub fn terminal(cwd:PathBuf,distro:Option<String>)->Self {Self {cwd,distro,directories:false,terminal:true}}
}

#[derive(Clone,PartialEq,Eq)]
struct Snapshot {text:String,cursor:usize,selection:(usize,usize)}
struct Pending {snapshot:Snapshot,target:Target,result:Option<Option<PathEdit>>}
#[derive(Default)]
struct State {token:u64,last_frame:u64,pending:Option<Pending>}
type Slot=Arc<Mutex<State>>;
fn slot(ctx:&egui::Context,id:Id)->Slot {
    ctx.data_mut(|data|data.get_temp_mut_or_default::<Slot>(id.with("path_completion")).clone())
}
fn snapshot(ctx:&egui::Context,id:Id,text:&str)->Snapshot {
    let count=text.chars().count();
    let range=egui::TextEdit::load_state(ctx,id).and_then(|state|state.cursor.char_range());
    let selection=range.map_or((count,count),|range|(range.primary.index.0.min(count),range.secondary.index.0.min(count)));
    let cursor=text.char_indices().nth(selection.0).map_or(text.len(),|(index,_)|index);
    Snapshot {text:text.into(),cursor,selection}
}
fn calculate(snapshot:&Snapshot,target:&Target)->Option<PathEdit> {
    let linux=target.distro.is_some();
    let separator=if linux || snapshot.text.contains('/') && !snapshot.text.contains('\\') {'/'}else{std::path::MAIN_SEPARATOR};
    let root=if cfg!(windows) {target.distro.as_ref().map(|distro|ide_core::wsl::windows_path(distro,"/"))}else{None};
    let home=if cfg!(windows) && linux && snapshot.text[..snapshot.cursor].contains('~') {
        let distro=target.distro.as_ref()?;
        Some(ide_core::wsl::windows_path(distro,&lisp_runtime::wsl::home_dir(distro).ok()?))
    } else {dirs::home_dir()};
    let env=termcomplete::Env {cwd:&target.cwd,home:home.as_deref(),commands:&[],separator,ignore_case:cfg!(windows)&&!linux};
    if target.terminal {termcomplete::first_path(&snapshot.text,snapshot.cursor,&env,root.as_deref())}
    else {termcomplete::path_field(&snapshot.text,snapshot.cursor,&env,root.as_deref(),target.directories)}
}

pub fn poll(ctx:&egui::Context,id:Id,text:&mut String,target:&Target)->bool {
    let shared=slot(ctx,id);let mut state=shared.lock().unwrap();
    let frame=ctx.cumulative_frame_nr();
    if frame>state.last_frame.saturating_add(1) {
        state.token=state.token.wrapping_add(1);state.pending=None;
    }
    state.last_frame=frame;
    let current=snapshot(ctx,id,text);
    let editing=ctx.input(|input|input.pointer.any_pressed() || input.events.iter().any(|event|match event {
        egui::Event::Text(_)|egui::Event::Paste(_)|egui::Event::Cut|egui::Event::Copy=>true,
        egui::Event::Key {key,pressed:true,modifiers,..}=>*key!=Key::Tab || modifiers.any(),
        _=>false,
    }));
    if state.pending.as_ref().is_some_and(|pending|editing||pending.snapshot!=current||pending.target!=*target||!ctx.memory(|m|m.has_focus(id))) {
        state.token=state.token.wrapping_add(1);state.pending=None;
    }
    let Some(pending)=state.pending.as_mut() else{return false};
    let Some(result)=pending.result.take() else {ctx.request_repaint_after(std::time::Duration::from_millis(100));return false};
    state.pending=None;
    let mut changed=false;
    if let Some(edit)=result && edit.start<=edit.end && edit.end<=text.len() && text.is_char_boundary(edit.start) && text.is_char_boundary(edit.end) {
        let original=text.clone();text.replace_range(edit.start..edit.end,&edit.text);
        if edit.caret<=text.len() && text.is_char_boundary(edit.caret) {
            let caret=text[..edit.caret].chars().count();
            let mut field=egui::TextEdit::load_state(ctx,id).unwrap_or_default();
            field.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(caret))));field.store(ctx,id);
        }
        changed=*text!=original;
    }
    if !changed && !target.terminal {ctx.memory_mut(|memory|memory.move_focus(egui::FocusDirection::Next));}
    changed
}

pub fn request(ctx:&egui::Context,id:Id,text:&str,target:&Target) {
    let shared=slot(ctx,id);let mut state=shared.lock().unwrap();
    if state.pending.is_some() {return}
    state.last_frame=ctx.cumulative_frame_nr();state.token=state.token.wrapping_add(1);
    let token=state.token;let snapshot=snapshot(ctx,id,text);let target=target.clone();
    state.pending=Some(Pending {snapshot:snapshot.clone(),target:target.clone(),result:None});
    drop(state);let ctx=ctx.clone();
    std::thread::spawn(move|| {
        let result=calculate(&snapshot,&target);
        let mut state=shared.lock().unwrap();
        if state.token==token && let Some(pending)=state.pending.as_mut() {pending.result=Some(result);}
        drop(state);ctx.request_repaint();
    });
}

/// Called before TextEdit so Tab does not become text or move focus while a match is pending.
pub fn field(ctx:&egui::Context,id:Id,text:&mut String,target:&Target)->(bool,bool) {
    let changed=poll(ctx,id,text,target);
    let mut complete=false;
    if ctx.memory(|memory|memory.has_focus(id)) {
        let shift=ctx.input_mut(|input|input.consume_key(Modifiers::SHIFT,Key::Tab));
        if shift {ctx.memory_mut(|memory|memory.move_focus(egui::FocusDirection::Previous));}
        else if ctx.input_mut(|input|input.consume_key(Modifiers::NONE,Key::Tab)) {
            // egui decides focus traversal at frame start, before events are consumed.
            // Cancel that decision even on the first frame after requesting focus.
            ctx.memory_mut(|memory|memory.move_focus(egui::FocusDirection::None));
            complete=true;
        }
    }
    (changed,complete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration,Instant};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new()->Self {
            static NEXT:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);
            let path=std::env::temp_dir().join(format!("light-path-test-{}-{}",std::process::id(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
            std::fs::create_dir(&path).unwrap();Self(path)
        }
        fn path(&self)->&std::path::Path {&self.0}
    }
    impl Drop for Fixture {fn drop(&mut self) {assert_eq!(self.0.parent(),Some(std::env::temp_dir().as_path()));assert!(self.0.file_name().unwrap().to_string_lossy().starts_with("light-path-test-"));std::fs::remove_dir_all(&self.0).unwrap();}}
    fn frame(ctx:&egui::Context,id:Id,text:&mut String,target:&Target,events:Vec<egui::Event>,focus:bool) {
        let input=egui::RawInput {events,focused:true,..Default::default()};
        let mut output=ctx.run_ui(input,|ui| {
            let response=crate::widgets::TextField::new(text).id(id).paths(target.clone()).show(ui,&crate::theme::Palette::dark());
            if focus {response.request_focus();}
            ui.button("Next field").clicked();
        });
        output.textures_delta.clear();
    }
    fn tab()->egui::Event {egui::Event::Key {key:Key::Tab,physical_key:None,pressed:true,repeat:false,modifiers:Modifiers::NONE}}
    #[test]
    fn real_field_tab_completes_a_path_with_spaces_and_keeps_focus() {
        let dir=Fixture::new();std::fs::create_dir(dir.path().join("My Projects")).unwrap();
        let target=Target::local(true).at(dir.path().into());let ctx=egui::Context::default();crate::fonts::install(&ctx);
        let id=Id::unique("path-field-test");let mut text="My P".to_owned();
        frame(&ctx,id,&mut text,&target,vec![],true);
        frame(&ctx,id,&mut text,&target,vec![tab()],false);
        let deadline=Instant::now()+Duration::from_secs(5);
        while text=="My P" && Instant::now()<deadline {std::thread::sleep(Duration::from_millis(2));frame(&ctx,id,&mut text,&target,vec![],false);}
        assert_eq!(text,format!("My Projects{}",std::path::MAIN_SEPARATOR));
        assert!(ctx.memory(|memory|memory.has_focus(id)));
        let state=egui::TextEdit::load_state(&ctx,id).unwrap();
        assert_eq!(state.cursor.char_range().unwrap().primary.index.0,text.chars().count());
        assert!(!text.contains('\t'));
    }
    #[test]
    fn late_results_do_not_replace_new_text_selection_or_a_different_destination() {
        for change in 0..4 {
            let ctx=egui::Context::default();crate::fonts::install(&ctx);let id=Id::unique("stale-path-test");
            let mut text="old".to_string();let mut target=Target::local(false);
            frame(&ctx,id,&mut text,&target,vec![],true);
            let captured=snapshot(&ctx,id,&text);let shared=slot(&ctx,id);
            shared.lock().unwrap().pending=Some(Pending {snapshot:captured,target:target.clone(),result:Some(Some(PathEdit {start:0,end:3,text:"WRONG".into(),caret:5}))});
            let mut events=vec![];
            match change {
                0=>text="new".into(),
                1=>{let mut state=egui::TextEdit::load_state(&ctx,id).unwrap();state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(0))));state.store(&ctx,id);},
                2=>target.cwd=PathBuf::from("different"),
                _=>events.push(egui::Event::Text("x".into())),
            }
            frame(&ctx,id,&mut text,&target,events,false);
            assert!(!text.contains("WRONG"));assert!(shared.lock().unwrap().pending.is_none());
        }
    }
    #[test]
    fn directory_only_fields_and_posix_targets_use_their_own_filesystem() {
        let dir=Fixture::new();std::fs::write(dir.path().join("sample.txt"),"").unwrap();
        let snapshot=Snapshot {text:"sam".into(),cursor:3,selection:(3,3)};
        assert!(calculate(&snapshot,&Target::local(true).at(dir.path().into())).is_none());
        let edit=calculate(&snapshot,&Target::local(false).at(dir.path().into())).unwrap();
        assert_eq!(edit.text,"sample.txt");
    }
}
