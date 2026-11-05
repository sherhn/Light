//! View files of WSL without loading a project; copy them to Windows on a worker.
use std::{path::{Path,PathBuf},sync::{Arc,Mutex,mpsc,atomic::{AtomicBool,Ordering}},time::Duration};
use eframe::egui::{self,Align2,RichText,Vec2};
use ide_core::transfer::{self,Progress,Report};
use crate::{app::App,editor_view::Sev,i18n::{self,tr},widgets};

struct Job {rx:mpsc::Receiver<Result<Report,String>>,cancel:Arc<AtomicBool>,progress:Arc<Mutex<Progress>>}
impl Drop for Job {fn drop(&mut self) {self.cancel.store(true,Ordering::Relaxed);}}
#[derive(Default)]
pub struct TransferState {job:Option<Job>,pub result:Option<Result<Report,String>>,visible:bool,source:PathBuf}
impl TransferState {pub fn busy(&self)->bool {self.job.is_some()}}
fn shown(path:&Path)->String {
    if let Some(linux)=ide_core::wsl::wsl_path(path) {return format!("{}:{}",linux.distro,linux.linux)}
    let value=path.to_string_lossy();value.strip_prefix(r"\\?\").unwrap_or(&value).to_string()
}

impl App {
    pub fn view_file_window(&mut self,path:&Path) {
        let result=std::env::current_exe().map_err(|e|e.to_string()).and_then(|exe| {
            let mut command=std::process::Command::new(exe);command.arg("--view").arg(path);lisp_runtime::no_window(&mut command);
            command.spawn().map(|_|()).map_err(|e|e.to_string())
        });
        if let Err(error)=result {self.toast(i18n::fmt("Cannot open file: {}",&[&error]),Sev::Error);}
    }
    pub fn pick_copy_to_windows(&mut self,source:&Path) {
        if self.transfer.busy() {self.transfer.visible=true;return}
        let home=dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
        if let Some(parent)=rfd::FileDialog::new().set_title(tr("Copy to Windows...")).set_directory(home).pick_folder() {self.start_file_copy(source,&parent);}
    }
    pub fn start_file_copy(&mut self,source:&Path,parent:&Path) {
        if self.transfer.busy() {self.transfer.visible=true;return}
        if cfg!(windows) && ide_core::wsl::wsl_path(parent).is_some() {self.toast(tr("Choose a folder on Windows for the copy"),Sev::Error);return}
        let source=source.to_path_buf();let parent=parent.to_path_buf();let cancel=Arc::new(AtomicBool::new(false));let progress=Arc::new(Mutex::new(Progress::default()));
        let (tx,rx)=mpsc::channel();let worker_cancel=cancel.clone();let worker_progress=progress.clone();let path=source.clone();
        std::thread::spawn(move|| {
            let mut published=std::time::Instant::now();
            let result=transfer::copy_verified(&path,&parent,&worker_cancel,|state|if state.verifying||published.elapsed()>=Duration::from_millis(100) { *worker_progress.lock().unwrap()=state;published=std::time::Instant::now();});
            let _=tx.send(result);
        });
        self.transfer=TransferState {job:Some(Job {rx,cancel,progress}),result:None,visible:true,source};
    }
    pub fn poll_transfer(&mut self,ctx:&egui::Context) {
        let result=self.transfer.job.as_ref().and_then(|job|match job.rx.try_recv() {Ok(result)=>Some(result),Err(mpsc::TryRecvError::Disconnected)=>Some(Err(tr("File copy stopped unexpectedly; the source was preserved").into())),Err(mpsc::TryRecvError::Empty)=>None});
        if let Some(result)=result {
            self.transfer.job=None;self.transfer.visible=true;
            if let Err(error)=&result {self.toast(error.clone(),Sev::Error);}
            self.transfer.result=Some(result);
        }
        if self.transfer.busy() {ctx.request_repaint_after(Duration::from_millis(100));}
    }
    pub fn draw_transfer(&mut self,ctx:&egui::Context) {
        if !self.transfer.visible {return}
        let pal=self.pal;let state=&mut self.transfer;
        egui::Area::new(egui::Id::unique("file_transfer")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER,Vec2::ZERO).show(ctx,|ui| {
            widgets::glass_frame(&pal).show(ui,|ui| {
                ui.set_width((ctx.content_rect().width()-64.0).clamp(260.0,560.0));
                ui.label(RichText::new(tr("Copy to Windows")).font(crate::fonts::semibold(17.0)));
                ui.add(egui::Label::new(shown(&state.source)).wrap());ui.add_space(10.0);
                if let Some(job)=&state.job {
                    let progress=job.progress.lock().unwrap().clone();
                    let fraction=if progress.total>0 {progress.bytes as f32/progress.total as f32}else{0.0};
                    ui.add(egui::ProgressBar::new(fraction.min(1.0)).text(tr(if progress.verifying {"Verifying copied files..."}else{"Copying files..."})).animate(true));
                    ui.label(format!("{} / {} · {} {}",crate::file_info::size(progress.bytes),crate::file_info::size(progress.total),progress.files,tr("files")));
                    if widgets::secondary_button(ui,&pal,tr("Cancel")).clicked() {job.cancel.store(true,Ordering::Relaxed);}
                }else if let Some(result)=&state.result {
                    match result {
                        Ok(report)=>{
                            ui.colored_label(pal.ok,tr("Copy verified. The source was preserved."));
                            ui.add(egui::Label::new(shown(&report.destination)).wrap());
                            ui.label(format!("{} {} · {}",report.files,tr("files"),crate::file_info::size(report.bytes)));
                            if widgets::secondary_button(ui,&pal,tr("Show in File Manager")).clicked() {crate::input::reveal_in_file_manager(&report.destination);}
                        }
                        Err(error)=>{
                            egui::ScrollArea::vertical().max_height(240.0).show(ui,|ui| {ui.add(egui::Label::new(RichText::new(error).color(pal.error)).wrap());});
                            if widgets::secondary_button(ui,&pal,tr("Copy error")).clicked() {ctx.copy_text(error.clone());}
                        }
                    }
                }
                ui.add_space(8.0);
                if widgets::secondary_button(ui,&pal,tr(if state.busy() {"Hide"}else{"Close"})).clicked() {state.visible=false;}
            });
        });
    }
}
