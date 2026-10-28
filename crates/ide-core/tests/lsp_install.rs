//! The install recipes of the registry's modules, run for real by the installer, and every server installed started:
//! downloads checked, `npm ci` from the lockfiles, folders and stamps, PATH. Runs only when asked, since it downloads:
//!
//! - `LIGHT_TEST_INSTALL`: a folder of this computer to install into (and to keep the downloads in);
//! - `LIGHT_TEST_INSTALL_WSL` and `LIGHT_TEST_INSTALL_WSL_DIR`: a WSL distribution and a folder in it, to install
//!   there as well (the downloads stay in `LIGHT_TEST_INSTALL`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ide_core::lsp::install::{self, Place};
use ide_core::lsp::{Client, Output, Session};
use ide_core::modules::{LspDef, Manifest};

fn registry() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../registry/modules")
}

/// Every server of the registry Light installs: (module folder, server).
fn servers() -> Vec<(PathBuf, LspDef)> {
    let mut out = vec![];
    for entry in std::fs::read_dir(registry()).unwrap() {
        let dir = entry.unwrap().path();
        let m = Manifest::parse(&std::fs::read_to_string(dir.join("module.json")).unwrap()).unwrap();
        out.extend(m.lsp.into_iter().filter(|s| s.install.is_some()).map(|s| (dir.clone(), s)));
    }
    out
}

/// Start `cmd` as a language server and wait for it to say what it can do.
fn answers(cmd: Command, root: &str, what: &str) {
    let client = Client::spawn(cmd, None).unwrap_or_else(|e| panic!("{what}: {e}"));
    let mut s = Session::start(client, root, serde_json::Value::Null);
    let start = Instant::now();
    loop {
        if s.poll().iter().any(|o| matches!(o, Output::Ready)) {
            break;
        }
        assert!(s.state != ide_core::lsp::State::Exited && start.elapsed() < Duration::from_secs(60), "{what} does not start: {:?}", s.client().log());
        std::thread::sleep(Duration::from_millis(20));
    }
    s.stop(Duration::from_secs(5));
    s.client_mut().kill();
}

fn log(what: &str) -> impl FnMut(&str) + '_ {
    move |l: &str| eprintln!("[{what}] {l}")
}

#[test]
fn every_recipe_installs_and_its_server_starts_here() {
    let Ok(dir) = std::env::var("LIGHT_TEST_INSTALL") else {
        eprintln!("LIGHT_TEST_INSTALL not set: skipped");
        return;
    };
    let dir = PathBuf::from(dir);
    let platform = install::host_platform().unwrap();
    let place = Place::Host(dir.join("servers"));
    for (module, def) in servers() {
        if !def.install.as_ref().unwrap().available_for(platform) {
            eprintln!("{}: nothing to install on {platform}", def.name);
            continue;
        }
        let units = install::plan(&def, &module, platform).unwrap();
        install::run(&units, &place, &dir.join("downloads"), &mut log(&def.name)).unwrap_or_else(|e| panic!("{e}"));
        let paths = install::host_paths(&def, &dir.join("servers"), platform);
        assert!(!paths.is_empty(), "{} is not installed", def.name);
        let path = std::env::join_paths(paths.iter().cloned().chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap();
        let program = ide_core::cmdscript::find_program(def.program(), Some(&path)).unwrap_or_else(|| panic!("{} not found in {paths:?}", def.program()));
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/D", "/C"]).arg(def.command_line(true));
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", def.command_line(false)]);
            c
        };
        cmd.env("PATH", &path).current_dir(&dir);
        eprintln!("{}: {} starts", def.name, program.display());
        answers(cmd, &dir.to_string_lossy().replace('\\', "/"), &def.name);
        // a second run leaves it as it is
        let mut again = vec![];
        install::run(&units, &place, &dir.join("downloads"), &mut |l| again.push(l.to_string())).unwrap();
        assert!(again.iter().all(|l| l.contains("installed already")), "{again:?}");
    }
}

#[test]
fn every_recipe_installs_and_its_server_starts_in_wsl() {
    let (Ok(downloads), Ok(distro), Ok(wsl_dir)) = (std::env::var("LIGHT_TEST_INSTALL"), std::env::var("LIGHT_TEST_INSTALL_WSL"), std::env::var("LIGHT_TEST_INSTALL_WSL_DIR")) else {
        eprintln!("LIGHT_TEST_INSTALL / LIGHT_TEST_INSTALL_WSL / LIGHT_TEST_INSTALL_WSL_DIR not set: skipped");
        return;
    };
    let platform = install::wsl_platform().unwrap();
    let place = Place::Wsl { distro: distro.clone(), user: None, dir: wsl_dir.clone(), mount_root: "/mnt/".into() };
    for (module, def) in servers() {
        if !def.install.as_ref().unwrap().available_for(platform) {
            continue;
        }
        let units = install::plan(&def, &module, platform).unwrap();
        install::run(&units, &place, &Path::new(&downloads).join("downloads"), &mut log(&def.name)).unwrap_or_else(|e| panic!("{e}"));
        let line = install::wsl_line(&def, &wsl_dir, platform, def.command_line(false));
        let mut cmd = Command::new("wsl.exe");
        cmd.args(["-d", &distro, "--cd", &wsl_dir, "-e", "sh", "-c", &line]).env("WSL_UTF8", "1");
        eprintln!("{}: {line}", def.name);
        answers(cmd, &wsl_dir, &def.name);
    }
}
