//! Verified, cancellable copies from WSL to a new name on the Windows filesystem.
//! The source is never deleted. Existing destinations are never replaced.
use std::{collections::HashSet,fs::{self,File,OpenOptions},io::{Read,Write},path::{Path,PathBuf},sync::atomic::{AtomicBool,AtomicU64,Ordering}};
use sha2::{Digest,Sha256};

#[derive(Clone,Debug,Default)]
pub struct Progress {pub files:usize,pub bytes:u64,pub total:u64,pub verifying:bool}
#[derive(Clone,Debug)]
pub struct Report {pub destination:PathBuf,pub files:usize,pub bytes:u64}
#[derive(Clone,Debug,PartialEq,Eq)]
struct Entry {relative:PathBuf,directory:bool,len:u64}

fn checked(cancel:&AtomicBool)->Result<(),String> {if cancel.load(Ordering::Relaxed) {Err("Copy cancelled; the source was preserved".into())}else{Ok(())}}
fn error(path:&Path,e:impl std::fmt::Display)->String {format!("{}: {e}",path.display())}
fn at(root:&Path,relative:&Path)->PathBuf {if relative.as_os_str().is_empty() {root.into()}else{root.join(relative)}}
fn valid_name(name:&str)->bool {
    let stem=name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !name.is_empty() && !name.ends_with(['.',' ']) && !name.chars().any(|c|c<' '||"<>:\"/\\|?*".contains(c))
        && !matches!(stem.as_str(),"CON"|"PRN"|"AUX"|"NUL"|"CLOCK$")
        && !(stem.len()==4 && (stem.starts_with("COM")||stem.starts_with("LPT")) && matches!(stem.as_bytes()[3],b'1'..=b'9'))
}
fn listing(source:&Path,cancel:&AtomicBool)->Result<Vec<Entry>,String> {
    let mut entries=Vec::new();let mut pending=vec![PathBuf::new()];let mut names=HashSet::new();
    while let Some(relative)=pending.pop() {
        checked(cancel)?;let path=at(source,&relative);
        let meta=fs::symlink_metadata(&path).map_err(|e|error(&path,e))?;
        if meta.file_type().is_symlink() || (!meta.is_file()&&!meta.is_dir()) {return Err(error(&path,"Links and special files cannot be copied to Windows"))}
        if !relative.as_os_str().is_empty() {
            let name=relative.file_name().and_then(|s|s.to_str()).ok_or_else(||error(&path,"Invalid Unicode filename"))?;
            if !valid_name(name) {return Err(error(&path,"This filename is not supported by Windows"))}
            if !names.insert(relative.to_string_lossy().to_lowercase()) {return Err(error(&path,"Names differ only by case; Windows cannot store both"))}
        }
        if entries.len()>=100_000 {return Err("More than 100,000 entries; choose a smaller folder".into())}
        entries.push(Entry {relative:relative.clone(),directory:meta.is_dir(),len:if meta.is_file(){meta.len()}else{0}});
        if meta.is_dir() {
            for child in fs::read_dir(&path).map_err(|e|error(&path,e))? {
                let child=child.map_err(|e|error(&path,e))?;pending.push(relative.join(child.file_name()));
            }
        }
    }
    entries.sort_by(|a,b|a.relative.cmp(&b.relative));Ok(entries)
}
fn hash(path:&Path,cancel:&AtomicBool)->Result<[u8;32],String> {
    let mut file=File::open(path).map_err(|e|error(path,e))?;let mut digest=Sha256::new();let mut buffer=vec![0;1024*1024];
    loop {checked(cancel)?;let n=file.read(&mut buffer).map_err(|e|error(path,e))?;if n==0 {break}digest.update(&buffer[..n]);}
    Ok(digest.finalize().into())
}
struct Stage {path:PathBuf,parent:PathBuf}
impl Stage {
    fn new(parent:&Path)->Result<Self,String> {
        static NEXT:AtomicU64=AtomicU64::new(0);
        for _ in 0..100 {
            let stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let path=parent.join(format!(".light-transfer-{}-{stamp}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
            match fs::create_dir(&path) {Ok(())=>return Ok(Self {path,parent:parent.into()}),Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>{},Err(e)=>return Err(error(&path,e))}
        }
        Err("Cannot reserve a temporary copy folder".into())
    }
}
impl Drop for Stage {fn drop(&mut self) {
    // Only this operation's exclusively created staging folder is ever removed.
    if self.path.parent()==Some(self.parent.as_path()) && self.path.file_name().is_some_and(|s|s.to_string_lossy().starts_with(".light-transfer-")) {
        let _=fs::remove_dir_all(&self.path);
    }
}}

#[cfg(windows)]
fn publish(source:&Path,destination:&Path)->Result<(),String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name="kernel32")]
    unsafe extern "system" {fn MoveFileExW(existing:*const u16,new:*const u16,flags:u32)->i32;}
    let source:Vec<u16>=source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target:Vec<u16>=destination.as_os_str().encode_wide().chain(Some(0)).collect();
    // No REPLACE_EXISTING flag: even a destination created during the copy wins.
    if unsafe {MoveFileExW(source.as_ptr(),target.as_ptr(),0)}==0 {return Err(error(destination,std::io::Error::last_os_error()))}Ok(())
}
#[cfg(not(windows))]
fn publish(source:&Path,destination:&Path)->Result<(),String> {
    // This feature is exposed only on Windows. Regular-file tests remain portable.
    fs::hard_link(source,destination).map_err(|e|error(destination,e))?;Ok(())
}

pub fn copy_verified(source:&Path,parent:&Path,cancel:&AtomicBool,mut progress:impl FnMut(Progress))->Result<Report,String> {
    checked(cancel)?;
    let name=source.file_name().and_then(|s|s.to_str()).filter(|name|valid_name(name)).ok_or("Choose a file or folder with a Windows-compatible name")?.to_string();
    if fs::symlink_metadata(source).map_err(|e|error(source,e))?.file_type().is_symlink() {return Err("Choose the original file or folder, not a link".into())}
    let source=source.canonicalize().map_err(|e|error(source,e))?;
    let parent=parent.canonicalize().map_err(|e|error(parent,e))?;
    if !parent.is_dir() || parent.starts_with(&source) {return Err("The destination must be a folder outside the source".into())}
    let destination=parent.join(&name);
    if fs::symlink_metadata(&destination).is_ok() {return Err(error(&destination,"Already exists; choose another destination folder"))}
    let plan=listing(&source,cancel)?;
    let mut status=Progress {total:plan.iter().map(|e|e.len).sum(),..Default::default()};progress(status.clone());
    let stage=Stage::new(&parent)?;let staged=stage.path.join(&name);let mut hashes=Vec::new();let mut buffer=vec![0;1024*1024];
    for entry in &plan {
        checked(cancel)?;let from=at(&source,&entry.relative);let to=at(&staged,&entry.relative);
        if entry.directory {fs::create_dir(&to).map_err(|e|error(&to,e))?;continue}
        let mut input=File::open(&from).map_err(|e|error(&from,e))?;
        let mut output=OpenOptions::new().write(true).create_new(true).open(&to).map_err(|e|error(&to,e))?;
        let mut digest=Sha256::new();let mut size=0;
        loop {
            checked(cancel)?;let n=input.read(&mut buffer).map_err(|e|error(&from,e))?;if n==0 {break}
            output.write_all(&buffer[..n]).map_err(|e|error(&to,e))?;digest.update(&buffer[..n]);size+=n as u64;status.bytes+=n as u64;progress(status.clone());
        }
        if size!=entry.len {return Err(error(&from,"Source changed during copying; retry when it is no longer being edited"))}
        output.sync_all().map_err(|e|error(&to,e))?;
        hashes.push((entry.relative.clone(),<[u8;32]>::from(digest.finalize())));status.files+=1;progress(status.clone());
    }
    status.verifying=true;progress(status.clone());
    if listing(&source,cancel)?!=plan {return Err("Source folder changed during copying; retry when it is no longer being edited".into())}
    for (relative,expected) in hashes {
        if hash(&at(&source,&relative),cancel)?!=expected || hash(&at(&staged,&relative),cancel)?!=expected {return Err(error(&at(&source,&relative),"Verification failed; the source was preserved"))}
    }
    checked(cancel)?;publish(&staged,&destination)?;
    Ok(Report {destination,files:status.files,bytes:status.bytes})
}

#[cfg(test)]
mod tests {
    use super::*;use crate::testutil::TempDir;
    #[test]
    fn verified_copy_preserves_source_and_never_overwrites_an_existing_file() {
        let dir=TempDir::new("transfer");let source=dir.path().join("notes.txt");let target=dir.path().join("windows");fs::create_dir(&target).unwrap();fs::write(&source,"Заметки проекта").unwrap();
        let report=copy_verified(&source,&target,&AtomicBool::new(false),|_|{}).unwrap();assert_eq!(report.files,1);assert_eq!(fs::read(&source).unwrap(),fs::read(&report.destination).unwrap());
        fs::write(&source,"new").unwrap();assert!(copy_verified(&source,&target,&AtomicBool::new(false),|_|{}).is_err());assert_eq!(fs::read_to_string(report.destination).unwrap(),"Заметки проекта");
    }
    #[test]
    fn cancellation_and_a_concurrently_created_destination_preserve_both_sides() {
        for cancel_copy in [true,false] {
            let dir=TempDir::new("transfer-race");let source=dir.path().join("sample.txt");let parent=dir.path().join("windows");fs::create_dir(&parent).unwrap();fs::write(&source,"original").unwrap();
            let cancel=AtomicBool::new(false);let mut injected=false;
            let result=copy_verified(&source,&parent,&cancel,|state|if state.bytes>0&&!injected {injected=true;if cancel_copy {cancel.store(true,Ordering::Relaxed)}else{fs::write(parent.join("sample.txt"),"other user's file").unwrap();}});
            assert!(result.is_err());assert_eq!(fs::read_to_string(source).unwrap(),"original");
            if !cancel_copy {assert_eq!(fs::read_to_string(parent.join("sample.txt")).unwrap(),"other user's file");}
            assert!(!fs::read_dir(parent).unwrap().any(|e|e.unwrap().file_name().to_string_lossy().starts_with(".light-transfer-")));
        }
    }
    #[test]
    fn modified_sources_are_detected_before_publication() {
        let dir=TempDir::new("transfer-modified");let source=dir.path().join("sample.txt");let parent=dir.path().join("windows");fs::create_dir(&parent).unwrap();fs::write(&source,"original").unwrap();
        let result=copy_verified(&source,&parent,&AtomicBool::new(false),|state|if state.verifying {fs::write(&source,"MODIFIED").unwrap();});
        assert!(result.is_err());assert!(!parent.join("sample.txt").exists());assert_eq!(fs::read_to_string(source).unwrap(),"MODIFIED");
    }
    #[cfg(windows)]
    #[test]
    fn directories_empty_folders_and_unicode_are_copied_and_verified() {
        let dir=TempDir::new("transfer-tree");let source=dir.path().join("Проект");let parent=dir.path().join("windows");fs::create_dir_all(source.join("empty")).unwrap();fs::create_dir_all(source.join("src")).unwrap();fs::write(source.join("src/main.lisp"),"(print :ok)").unwrap();fs::create_dir(&parent).unwrap();
        let report=copy_verified(&source,&parent,&AtomicBool::new(false),|_|{}).unwrap();assert_eq!(report.files,1);assert!(report.destination.join("empty").is_dir());assert_eq!(fs::read(source.join("src/main.lisp")).unwrap(),fs::read(report.destination.join("src/main.lisp")).unwrap());
        assert!(copy_verified(&source,&source,&AtomicBool::new(false),|_|{}).is_err());
    }
    #[test]
    fn incompatible_windows_names_are_rejected() {for name in ["CON","con.txt","COM1.log","Lpt9","file.","file ","a:b","a\\b","a?b"] {assert!(!valid_name(name),"{name}");}assert!(valid_name("Скрипт.py"));}
    #[cfg(unix)]
    #[test]
    fn links_are_rejected_without_reading_their_targets() {
        let dir=TempDir::new("transfer-link");let source=dir.path().join("source");fs::create_dir(&source).unwrap();std::os::unix::fs::symlink("/etc",source.join("outside")).unwrap();assert!(listing(&source,&AtomicBool::new(false)).is_err());
    }
}
