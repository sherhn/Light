//! Locate runnable declarations; the UI caches results by document and module revisions.
use crate::modules::{CodeActionDef,TaskDef};
use lisp_syntax::{Dialect,TokenKind,parse::Parsed};

#[derive(Clone,Debug,PartialEq)]
pub struct Item {pub action:CodeActionDef,pub symbol:String,pub line:usize}
pub fn compile(pattern:&str)->Result<regex::Regex,String> {
    if pattern.is_empty()||pattern.len()>16*1024 {return Err("A code action needs a nonempty pattern shorter than 16 KB".into())}
    regex::RegexBuilder::new(pattern).multi_line(true).size_limit(1024*1024).build().map_err(|e|format!("Code action pattern: {e}"))
}
/// A capture is data, never a shell expression. Dots, dashes and colons allow build target names.
pub fn safe_symbol(symbol:&str)->bool {symbol.len()<=256&&symbol.chars().all(|c|c.is_alphanumeric()||matches!(c,'_'|'.'|'-'|':'))}
fn indentation(line:&str)->usize {line.len()-line.trim_start_matches([' ','\t']).len()}
pub fn find(defs:&[CodeActionDef],text:&str,dead:&[(usize,usize)])->Vec<Item> {
    let mut result=Vec::new();let starts:Vec<_>=std::iter::once(0).chain(text.match_indices('\n').map(|(i,_)|i+1)).collect();
    for action in defs {
        let Ok(pattern)=compile(&action.pattern)else{continue};
        let contexts=if action.context.is_empty() {None}else{match compile(&action.context) {Ok(pattern)=>Some(pattern.find_iter(text).map(|region| {
            let indent=region.as_str().lines().skip(1).filter(|line|!line.trim().is_empty()&&!line.trim_start().starts_with('#')).map(indentation).min();
            (region.start(),region.end(),indent)
        }).collect::<Vec<_>>()),Err(_)=>continue}};
        for capture in pattern.captures_iter(text) {
            let Some(whole)=capture.get(0)else{continue};
            let symbol=capture.name("symbol");let at=symbol.map_or_else(||whole.start()+whole.as_str().len()-whole.as_str().trim_start().len(),|m|m.start());
            if dead.iter().any(|&(start,end)|start<=at&&at<end) {continue}
            let line_start=text[..at].rfind('\n').map_or(0,|at|at+1);
            if contexts.as_ref().is_some_and(|regions|!regions.iter().any(|&(start,end,indent)|start<=at&&at<end&&(!action.direct_children||indent==Some(indentation(&text[line_start..at]))))) {continue}
            let symbol=symbol.map_or("",|m|m.as_str());if !safe_symbol(symbol) || symbol.is_empty()&&(action.command.contains("{symbol}")||action.windows.contains("{symbol}")) {continue}
            result.push(Item {action:action.clone(),symbol:symbol.into(),line:starts.partition_point(|&start|start<=at).saturating_sub(1)});
        }
    }
    result.sort_by_key(|item|item.line);result
}
pub fn tasks(def:&TaskDef,text:&str)->Vec<(usize,String)> {
    let names=def.tasks_in(text);let mut locations=Vec::new();
    if !def.pattern.is_empty() && let Ok(pattern)=regex::RegexBuilder::new(&def.pattern).multi_line(true).build() {
        for capture in pattern.captures_iter(text) {
            if let Some(name)=capture.name("name") && names.iter().any(|n|n==name.as_str()) {locations.push((text[..name.start()].bytes().filter(|&c|c==b'\n').count(),name.as_str().into()));}
        }
    }else if !def.json.is_empty() {
        let section=text.find(&serde_json::to_string(&def.json).unwrap_or_default()).unwrap_or(0);
        for name in names.iter().filter(|name|!def.fixed.iter().any(|fixed|fixed.name==**name)) {
            let quoted=serde_json::to_string(name).unwrap_or_default();
            if let Some(at)=text[section..].find(&quoted) {locations.push((text[..section+at].bytes().filter(|&c|c==b'\n').count(),name.clone()));}
        }
    }
    for fixed in &def.fixed {locations.push((0,fixed.name.clone()));}
    locations.sort_by_key(|(line,_)|*line);locations
}

#[derive(Clone,Debug,PartialEq)]
pub struct LispFunction {pub name:String,pub package:String,pub line:usize}
pub fn lisp_functions(text:&str,parsed:&Parsed)->Vec<LispFunction> {
    if parsed.dialect!=Dialect::CommonLisp {return vec![]}
    let mut package=Some("CL-USER".to_string());let mut result=Vec::new();
    for datum in parsed.top_level() {
        if parsed.tokens[datum.first].kind!=TokenKind::Open {continue}
        let children=parsed.children(datum.first);let Some(operator)=children.first()else{continue};
        let op=text[parsed.start_of(*operator)..parsed.end_of(*operator)].to_ascii_lowercase();
        let op=op.strip_prefix("cl:").or_else(||op.strip_prefix("common-lisp:")).unwrap_or(&op);
        if op=="in-package" {
            package=children.get(1).and_then(|name| {
                let value=&text[parsed.start_of(*name)..parsed.end_of(*name)];
                if value.starts_with('"')&&value.ends_with('"')&&!value.contains('\\') {Some(value[1..value.len()-1].into())}
                else if value.trim_start_matches(['#',':']).chars().all(|c|c.is_alphanumeric()||matches!(c,'-'|'_'|'/')) {Some(value.trim_start_matches(['#',':']).to_uppercase())}else{None}
            });
        }
        if op!="defun"||children.len()<3 {continue}
        let name=children[1];let args=children[2];
        if parsed.tokens[name.first].kind!=TokenKind::Symbol || parsed.tokens[args.first].kind!=TokenKind::Open || parsed.partner[args.first].is_none() || !parsed.children(args.first).is_empty() {continue}
        if let Some(package)=&package {result.push(LispFunction {name:text[parsed.start_of(name)..parsed.end_of(name)].into(),package:package.clone(),line:text[..parsed.start_of(datum)].bytes().filter(|&c|c==b'\n').count()});}
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declaration_matches_skip_strings_comments_and_unsafe_captures() {
        let action=CodeActionDef {language:"python".into(),name:"Run {symbol}".into(),pattern:r"^def (?P<symbol>[^ (]+)\(\):".into(),command:"python {symbol}".into(),..Default::default()};
        let text="def fake():\ndef real():\ndef bad$():\n";
        let found=find(std::slice::from_ref(&action),text,&[(0,11)]);assert_eq!(found.len(),1);assert_eq!((&found[0].symbol,found[0].line),(&"real".to_string(),1));
        assert!(action.command_line("x$(touch)","file","dir",false).is_none());
        assert!(compile("").is_err());assert!(compile("[").is_err());
    }
    #[test]
    fn lisp_functions_respect_reader_prefixes_packages_and_arguments() {
        let text="(defpackage :demo (:use :cl))\n(in-package :demo)\n(defun main () :ok)\n(defun needs (x) x)\n'(defun quoted () :no)\n; (defun comment () :no)\n\"(defun string () :no)\"\n(in-package \"case-Sensitive\")\n(defun second () :ok)";
        let found=lisp_functions(text,&lisp_syntax::parse::parse(text,Dialect::CommonLisp));
        assert_eq!(found.iter().map(|f|(f.name.as_str(),f.package.as_str(),f.line)).collect::<Vec<_>>(),[("main","DEMO",2),("second","case-Sensitive",8)]);
        assert!(lisp_functions(text,&lisp_syntax::parse::parse(text,Dialect::Scheme)).is_empty());
    }
    #[test]
    fn task_locations_are_the_actual_make_and_npm_declarations() {
        let make=TaskDef {pattern:r"(?m)^(?P<name>[A-Za-z_][\w-]*):".into(),..Default::default()};
        assert_eq!(tasks(&make,"# comment\nhello:\n\techo hi\nclean:\n"),[(1,"hello".into()),(3,"clean".into())]);
        let npm=TaskDef {json:"scripts".into(),..Default::default()};
        assert_eq!(tasks(&npm,"{\n\"name\":\"build\",\n\"scripts\":{\n\"build\":\"echo hi\",\n\"test\":\"echo test\"}}"),[(3,"build".into()),(4,"test".into())]);
    }
    #[test]
    fn compose_actions_are_limited_to_services() {
        let action=CodeActionDef {pattern:r"^[ \t]+(?P<symbol>[A-Za-z_]\w*):".into(),context:r"^services:[^\n]*(?:\n(?:[ \t#\r][^\n]*)?)*".into(),direct_children:true,..Default::default()};
        for width in [2,4] {for newline in ["\n","\r\n"] {
            let text=format!("services:\n{pad}web:\n{pad}{pad}image: busybox\n# comment\n\n{pad}worker:\n{pad}{pad}image: busybox\nvolumes:\n{pad}cache:\nnetworks:\n{pad}private:\n",pad=" ".repeat(width)).replace('\n',newline);
            let found=find(std::slice::from_ref(&action),&text,&[]);
            assert_eq!(found.iter().map(|item|item.symbol.as_str()).collect::<Vec<_>>(),["web","worker"]);
        }}
    }
}
