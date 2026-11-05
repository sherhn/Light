//! Run buttons for files, declarations supplied by modules, Lisp functions and build tasks.
use std::path::Path;
use ide_core::{code_actions::{self,LispFunction},modules::CodeActionDef};
use lisp_runtime::{Arg,runner};
use crate::{app::App,app_run::{RunRequest,shell_launch,shell_path},editor_view::{GutterIcon,Sev},gutter::GutterAction,i18n::{self,tr},icons::Icon};

pub struct Cache {revision:u64,key:String,items:Vec<(usize,GutterAction)>}
fn label(action:&GutterAction)->String {
    match action {
        GutterAction::RunCode(_,def,symbol)=>i18n::tr_dyn(&def.name).replace("{symbol}",symbol),
        GutterAction::RunLispFunction(_,function)=>i18n::fmt("Run function '{}'",&[&function.name]),
        GutterAction::RunTask(_,def,name)=>format!("{} {name}",def.name),
        GutterAction::RunLanguage(_,def)=>i18n::tr_dyn(&def.name).into_owned(),
        GutterAction::RunFile(path)=>i18n::fmt("Run file '{}'",&[&path.file_name().unwrap_or_default().to_string_lossy()]),
        _=>String::new(),
    }
}
impl App {
    fn runnable_items(&mut self,idx:usize)->Vec<(usize,GutterAction)> {
        let Some(doc)=self.docs.get(idx)else{return vec![]};
        let Some(path)=doc.path.clone().filter(|_|doc.diff.is_none()&&doc.page.is_none())else{return vec![]};
        let language=doc.ed.language().map_or("common-lisp",|lang|lang.id()).to_string();
        let actions:Vec<_>=self.mods.installed.iter().filter(|module|module.enabled).flat_map(|module|module.manifest.code_actions.iter()).filter(|action|action.language==language).cloned().collect();
        let file_run=self.run_def_for(&language);let native=doc.ed.is_lisp()&&self.lisp_enabled();
        let filename=path.file_name().unwrap_or_default().to_string_lossy();
        let tasks:Vec<_>=self.task_defs().into_iter().filter(|task|task.holds(&filename)&&!path.parent().is_some_and(|parent|parent.components().any(|part|task.skip.iter().any(|skip|part.as_os_str().eq_ignore_ascii_case(skip))))).collect();
        let key=format!("{native}|{}",serde_json::to_string(&(&actions,&file_run,&tasks)).unwrap_or_default());
        let revision=doc.ed.revision();
        if let Some(cache)=&doc.runnable_cache && cache.revision==revision&&cache.key==key {return cache.items.clone()}
        let doc=&mut self.docs[idx];let mut items=Vec::new();
        let first=doc.ed.text().lines().position(|line|!line.trim().is_empty()).unwrap_or(0);
        if let Some(def)=file_run.filter(|_|tasks.is_empty()) {
            let line=if matches!(language.as_str(),"c"|"cpp") {doc.defs().iter().find(|def|def.name=="main").map_or(first,|def|def.line)}else{first};
            items.push((line,GutterAction::RunLanguage(path.clone(),def)));
        }else if native {items.push((first,GutterAction::RunFile(path.clone())));}
        let dead:Vec<_>=doc.ed.language_spans().map(|spans|spans.iter().filter(|span|matches!(span.class,lisp_syntax::highlight::Class::Comment|lisp_syntax::highlight::Class::String|lisp_syntax::highlight::Class::Code)).map(|span|(span.start,span.end)).collect()).unwrap_or_else(||doc.ed.parsed().tokens.iter().filter(|token|token.kind.is_comment()||matches!(token.kind,lisp_syntax::TokenKind::String|lisp_syntax::TokenKind::UnterminatedString)).map(|token|(token.start,token.end)).collect());
        for item in code_actions::find(&actions,doc.ed.text(),&dead) {items.push((item.line,GutterAction::RunCode(path.clone(),item.action,item.symbol)));}
        if native {
            for function in code_actions::lisp_functions(doc.ed.text(),&doc.ed.parsed()) {items.push((function.line,GutterAction::RunLispFunction(path.clone(),function)));}
        }
        for task in tasks {for (line,name) in code_actions::tasks(&task,doc.ed.text()) {items.push((line,GutterAction::RunTask(path.clone(),task.clone(),name)));}}
        items.sort_by_key(|(line,_)|*line);
        doc.runnable_cache=Some(Cache {revision,key,items:items.clone()});items
    }
    pub fn code_run_icons(&mut self,idx:usize,specific:bool)->Vec<GutterIcon> {
        let mut icons=Vec::new();
        for (line,action) in self.runnable_items(idx) {
            if specific&&matches!(action,GutterAction::RunFile(_)|GutterAction::RunLanguage(..)) {continue}
            if icons.iter().any(|icon:&GutterIcon|icon.line==line) {continue}
            icons.push(GutterIcon {line,icon:Icon::Play,color:self.pal.ok,tip:label(&action)});
        }
        icons
    }
    pub fn code_run_actions(&mut self,idx:usize,line:usize)->Vec<(Icon,String,GutterAction)> {
        self.runnable_items(idx).into_iter().filter(|(at,_)|*at==line).map(|(_,action)|(Icon::Play,label(&action),action)).collect()
    }
    fn ready_to_run_code(&mut self,path:&Path)->Option<usize> {
        if !self.open_file(path) {return None}
        self.save_all();
        if self.docs.iter().any(|doc|doc.path.is_some()&&doc.ed.is_modified()) {self.toast(tr("Save modified files before running this action"),Sev::Weak);return None}
        self.docs.iter().position(|doc|doc.path.as_deref()==Some(path))
    }
    pub fn run_code_action(&mut self,path:&Path,def:&CodeActionDef,symbol:&str) {
        let Some(idx)=self.ready_to_run_code(path)else{return};
        let wanted=GutterAction::RunCode(path.into(),def.clone(),symbol.into());
        if !self.runnable_items(idx).iter().any(|(_,action)|*action==wanted) {self.toast(tr("This run target changed or its module was disabled"),Sev::Weak);return}
        let root=self.project_root().map(Path::to_path_buf).or_else(||path.parent().map(Path::to_path_buf)).unwrap_or_default();
        let target=self.wsl_target();let dir=path.parent().unwrap_or(Path::new("."));
        let Some(line)=def.command_line(symbol,&shell_path(path,target.as_ref()),&shell_path(dir,target.as_ref()),target.is_none()&&cfg!(windows))else{return};
        let title=label(&wanted);let launch=shell_launch(&line,&root,target.as_ref(),&[]);
        if self.start_run(format!("code:{}:{}:{symbol}",path.display(),def.name),title.clone(),launch,&title,root,target) {self.run_request=Some(RunRequest::CodeAction(def.clone(),path.into(),symbol.into()));}
    }
    pub fn run_lisp_function(&mut self,path:&Path,function:&LispFunction) {
        let Some(idx)=self.ready_to_run_code(path)else{return};
        if !self.lisp_enabled()||!code_actions::lisp_functions(self.docs[idx].ed.text(),&self.docs[idx].ed.parsed()).iter().any(|item|item.name==function.name&&item.package==function.package) {self.toast(tr("This run target changed or its module was disabled"),Sev::Weak);return}
        let Some(imp)=self.lisp.clone().or_else(||self.resolve_lisp())else{self.toast(tr("SBCL not found - set the path in Settings"),Sev::Error);return};
        let root=self.project_root().map(Path::to_path_buf).or_else(||path.parent().map(Path::to_path_buf)).unwrap_or_default();
        let call=format!("(let ((*read-eval* nil) (*package* (or (find-package {}) (error \"Function package is not loaded\")))) (format t \"~&~S~%\" (funcall (read-from-string {}))))",runner::lisp_string(&function.package),runner::lisp_string(&function.name));
        let system=match &self.run_config {crate::app_run::RunConfig::Load(system)|crate::app_run::RunConfig::Test(system)=>Some(system.clone()),_=>self.asd_systems().into_iter().find(|name|!name.ends_with("/test"))};
        let setup=format!("(asdf:initialize-source-registry (list :source-registry (list :tree {}) :inherit-configuration))",runner::lisp_string(&imp.lisp_path(&root)));
        let load=system.as_ref().map(|system|format!("(asdf:load-system {})",runner::lisp_string(system)));
        let mut args=vec![Arg::Text("--noinform"),Arg::Text("--non-interactive"),Arg::Text("--no-userinit"),Arg::Text("--no-sysinit")];
        if let Some(load)=&load {args.extend([Arg::Text("--eval"),Arg::Text("(require :asdf)"),Arg::Text("--eval"),Arg::Text(&setup),Arg::Text("--eval"),Arg::Text(load)]);}
        args.extend([Arg::Text("--load"),Arg::Path(path),Arg::Text("--eval"),Arg::Text(&call)]);
        let title=i18n::fmt("Run function '{}'",&[&function.name]);let launch=imp.launch(Some(&root),&args);let target=imp.wsl.clone();
        if self.start_run(format!("function:{}:{}:{}",path.display(),function.package,function.name),title.clone(),launch,&title,root,target) {self.run_request=Some(RunRequest::LispFunction(path.into(),function.clone()));}
    }
}
