//! A pseudo console: a program that believes it runs in a terminal window. What it draws on its screen comes back as
//! VT sequences, and the keys typed are written to it as bytes. On Windows this is ConPTY (`CreatePseudoConsole`), the
//! way Windows Terminal hosts WSL: `wsl.exe` inside it gives the Linux shell a real terminal (colours, prompts,
//! full-screen programs, Ctrl+C, the window's size).

// elsewhere there is no pseudo console to make (a Linux shell is a project of Windows): the types stay for the code that
// is the same everywhere, and what only the console of Windows uses is not compiled
#![cfg_attr(not(windows), allow(dead_code))]

use std::ffi::OsStr;
use std::path::Path;
#[cfg(windows)]
use std::sync::mpsc::{self, Receiver, Sender};

/// What a pseudo console reports.
pub enum PtyEvent {
    /// Bytes the program wrote to its screen.
    Output(Vec<u8>),
    /// The program ended, with its exit code when it is known.
    Exited(Option<u32>),
}

/// The command line of a program for `CreateProcessW`: each argument quoted the way `CommandLineToArgvW` reads it back.
pub fn command_line(program: &OsStr, args: &[String]) -> String {
    fn quote(arg: &str) -> String {
        if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
            return arg.to_string();
        }
        let mut out = String::from("\"");
        let mut backslashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    // the backslashes before a quote are doubled, and the quote itself is escaped
                    out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    out.push('"');
                    backslashes = 0;
                }
                c => {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    out.push(c);
                    backslashes = 0;
                }
            }
        }
        // backslashes before the closing quote are doubled too
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
        out.push('"');
        out
    }
    let mut line = quote(&program.to_string_lossy());
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    line
}

#[cfg(windows)]
pub use windows::Pty;

#[cfg(not(windows))]
pub struct Pty;

#[cfg(not(windows))]
impl Pty {
    pub fn spawn(_program: &OsStr, _args: &[String], _cwd: Option<&Path>, _env: &[(String, String)], _cols: u16, _rows: u16, _wake: impl Fn() + Send + 'static) -> std::io::Result<Pty> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "a pseudo console is only made on Windows"))
    }
    pub fn write(&self, _bytes: &[u8]) {}
    pub fn resize(&self, _cols: u16, _rows: u16) {}
    pub fn try_recv(&self) -> Option<PtyEvent> {
        None
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::thread::JoinHandle;

    type Handle = isize;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Coord {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    struct StartupInfoW {
        cb: u32,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: u32,
        y: u32,
        x_size: u32,
        y_size: u32,
        x_count_chars: u32,
        y_count_chars: u32,
        fill_attribute: u32,
        flags: u32,
        show_window: u16,
        reserved2_len: u16,
        reserved2: *mut u8,
        std_input: Handle,
        std_output: Handle,
        std_error: Handle,
    }

    #[repr(C)]
    struct StartupInfoExW {
        info: StartupInfoW,
        attributes: *mut c_void,
    }

    #[repr(C)]
    struct ProcessInformation {
        process: Handle,
        thread: Handle,
        process_id: u32,
        thread_id: u32,
    }

    const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;
    const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
    const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
    const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
    const INVALID_HANDLE_VALUE: Handle = -1;
    const INFINITE: u32 = 0xFFFF_FFFF;
    const STILL_ACTIVE: u32 = 259;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreatePipe(read: *mut Handle, write: *mut Handle, attributes: *const c_void, size: u32) -> i32;
        fn CreatePseudoConsole(size: Coord, input: Handle, output: Handle, flags: u32, console: *mut Handle) -> i32;
        fn ResizePseudoConsole(console: Handle, size: Coord) -> i32;
        fn ClosePseudoConsole(console: Handle);
        fn InitializeProcThreadAttributeList(list: *mut c_void, count: u32, flags: u32, size: *mut usize) -> i32;
        fn UpdateProcThreadAttribute(list: *mut c_void, flags: u32, attribute: usize, value: *const c_void, size: usize, previous: *mut c_void, returned: *mut usize) -> i32;
        fn DeleteProcThreadAttributeList(list: *mut c_void);
        fn CreateProcessW(application: *const u16, command_line: *mut u16, process_attributes: *const c_void, thread_attributes: *const c_void, inherit_handles: i32, flags: u32, environment: *const c_void, cwd: *const u16, startup: *const StartupInfoExW, info: *mut ProcessInformation) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn ReadFile(file: Handle, buffer: *mut u8, len: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
        fn WriteFile(file: Handle, buffer: *const u8, len: u32, written: *mut u32, overlapped: *mut c_void) -> i32;
        fn TerminateProcess(process: Handle, code: u32) -> i32;
        fn WaitForSingleObject(handle: Handle, ms: u32) -> u32;
        fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
    }

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    fn last_error(what: &str) -> std::io::Error {
        let e = std::io::Error::last_os_error();
        std::io::Error::new(e.kind(), format!("{what}: {e}"))
    }

    /// A program running in a pseudo console. Dropping it ends the program.
    pub struct Pty {
        console: Handle,
        process: Handle,
        events: Receiver<PtyEvent>,
        input: Option<Sender<Vec<u8>>>,
        threads: Vec<JoinHandle<()>>,
    }

    // the handles are plain numbers the system owns; the threads that use them end before they are closed
    unsafe impl Send for Pty {}

    impl Pty {
        /// Start `program` with `args` in a pseudo console of `cols` x `rows` cells. `wake` is called (from another
        /// thread) whenever there is something to read.
        pub fn spawn(program: &OsStr, args: &[String], cwd: Option<&Path>, env: &[(String, String)], cols: u16, rows: u16, wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Pty> {
            let size = Coord { x: cols.max(2) as i16, y: rows.max(2) as i16 };
            // SAFETY: plain Win32 calls; every out-pointer points to a local of the right type, and each handle made here
            // is closed exactly once: the pipe ends given to the console right after it has its own copies, the others
            // by the thread that uses them or by `Drop`
            unsafe {
                let (mut in_read, mut in_write, mut out_read, mut out_write) = (0, 0, 0, 0);
                if CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) == 0 {
                    return Err(last_error("CreatePipe"));
                }
                if CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0) == 0 {
                    CloseHandle(in_read);
                    CloseHandle(in_write);
                    return Err(last_error("CreatePipe"));
                }
                let mut console = 0;
                let hr = CreatePseudoConsole(size, in_read, out_write, 0, &mut console);
                CloseHandle(in_read);
                CloseHandle(out_write);
                if hr != 0 {
                    CloseHandle(in_write);
                    CloseHandle(out_read);
                    return Err(std::io::Error::other(format!("CreatePseudoConsole failed (0x{hr:08x})")));
                }
                let fail = |what: &str| {
                    let e = last_error(what);
                    ClosePseudoConsole(console);
                    CloseHandle(in_write);
                    CloseHandle(out_read);
                    e
                };
                // the attribute that puts the program into the console
                let mut list_size = 0usize;
                InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut list_size);
                let mut list = vec![0u8; list_size];
                let list_ptr = list.as_mut_ptr() as *mut c_void;
                if InitializeProcThreadAttributeList(list_ptr, 1, 0, &mut list_size) == 0 {
                    return Err(fail("InitializeProcThreadAttributeList"));
                }
                if UpdateProcThreadAttribute(list_ptr, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, console as *const c_void, std::mem::size_of::<Handle>(), std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
                    DeleteProcThreadAttributeList(list_ptr);
                    return Err(fail("UpdateProcThreadAttribute"));
                }
                let mut startup: StartupInfoExW = std::mem::zeroed();
                startup.info.cb = std::mem::size_of::<StartupInfoExW>() as u32;
                // no standard handles of this process (the tests redirect them): the console is the only one
                startup.info.flags = STARTF_USESTDHANDLES;
                startup.info.std_input = INVALID_HANDLE_VALUE;
                startup.info.std_output = INVALID_HANDLE_VALUE;
                startup.info.std_error = INVALID_HANDLE_VALUE;
                startup.attributes = list_ptr;
                let mut line = wide(OsStr::new(&command_line(program, args)));
                let cwd = cwd.map(|d| wide(d.as_os_str()));
                // this process's environment and the variables given
                let mut vars: Vec<(String, String)> = std::env::vars().filter(|(k, _)| !env.iter().any(|(e, _)| e.eq_ignore_ascii_case(k))).collect();
                vars.extend(env.iter().cloned());
                vars.sort_by_key(|(k, _)| k.to_uppercase());
                let mut block: Vec<u16> = Vec::new();
                for (k, v) in &vars {
                    block.extend(OsStr::new(&format!("{k}={v}")).encode_wide());
                    block.push(0);
                }
                block.push(0);
                let mut info: ProcessInformation = std::mem::zeroed();
                let ok = CreateProcessW(std::ptr::null(), line.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0, EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT, block.as_ptr() as *const c_void, cwd.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()), &startup, &mut info);
                DeleteProcThreadAttributeList(list_ptr);
                if ok == 0 {
                    return Err(fail("CreateProcessW"));
                }
                CloseHandle(info.thread);
                let process = info.process;

                let (tx, events) = mpsc::channel();
                let wake = std::sync::Arc::new(wake);
                let mut threads = Vec::new();
                // the screen output, as it comes
                {
                    let (tx, wake) = (tx.clone(), wake.clone());
                    threads.push(std::thread::spawn(move || {
                        let mut buf = vec![0u8; 16 * 1024];
                        loop {
                            let mut read = 0u32;
                            if ReadFile(out_read, buf.as_mut_ptr(), buf.len() as u32, &mut read, std::ptr::null_mut()) == 0 || read == 0 {
                                break;
                            }
                            if tx.send(PtyEvent::Output(buf[..read as usize].to_vec())).is_err() {
                                break;
                            }
                            wake();
                        }
                        CloseHandle(out_read);
                    }));
                }
                // the keys, written in order without holding up the window
                let (input, keys) = mpsc::channel::<Vec<u8>>();
                threads.push(std::thread::spawn(move || {
                    while let Ok(bytes) = keys.recv() {
                        let mut at = 0;
                        while at < bytes.len() {
                            let mut written = 0u32;
                            if WriteFile(in_write, bytes[at..].as_ptr(), (bytes.len() - at) as u32, &mut written, std::ptr::null_mut()) == 0 {
                                CloseHandle(in_write);
                                return;
                            }
                            at += written as usize;
                        }
                    }
                    CloseHandle(in_write);
                }));
                // the end of the program
                threads.push(std::thread::spawn(move || {
                    WaitForSingleObject(process, INFINITE);
                    let mut code = 0u32;
                    let code = (GetExitCodeProcess(process, &mut code) != 0 && code != STILL_ACTIVE).then_some(code);
                    let _ = tx.send(PtyEvent::Exited(code));
                    wake();
                }));
                Ok(Pty { console, process, events, input: Some(input), threads })
            }
        }

        /// Type `bytes` (keys, pasted text) into the program.
        pub fn write(&self, bytes: &[u8]) {
            if let Some(input) = &self.input
                && !bytes.is_empty()
            {
                let _ = input.send(bytes.to_vec());
            }
        }

        /// The window of the program has `cols` x `rows` cells now.
        pub fn resize(&self, cols: u16, rows: u16) {
            // SAFETY: the console handle stays open until `Drop`
            unsafe {
                ResizePseudoConsole(self.console, Coord { x: cols.max(2) as i16, y: rows.max(2) as i16 });
            }
        }

        pub fn try_recv(&self) -> Option<PtyEvent> {
            self.events.try_recv().ok()
        }
    }

    impl Drop for Pty {
        fn drop(&mut self) {
            // SAFETY: the program is ended, then its console closed (that ends the reader and the writer); the waiting
            // thread returns once the program is gone, and only then is the process handle closed
            unsafe {
                TerminateProcess(self.process, 1);
                self.input = None;
                ClosePseudoConsole(self.console);
                for t in self.threads.drain(..) {
                    let _ = t.join();
                }
                CloseHandle(self.process);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_quoted_the_way_windows_reads_them_back() {
        let line = command_line(OsStr::new(r"C:\Program Files\x.exe"), &["-d".into(), "Ubuntu 24".into(), r#"say "hi""#.into(), r"C:\dir\".into(), String::new()]);
        assert_eq!(line, r#""C:\Program Files\x.exe" -d "Ubuntu 24" "say \"hi\"" C:\dir\ """#);
        assert_eq!(command_line(OsStr::new("wsl.exe"), &["--cd".into(), r"a b\".into()]), r#"wsl.exe --cd "a b\\""#);
    }

    #[cfg(windows)]
    #[test]
    fn a_program_in_a_pseudo_console_is_heard_and_can_be_typed_to() {
        use std::time::{Duration, Instant};
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        let pty = Pty::spawn(OsStr::new(&comspec), &["/Q".into(), "/K".into(), "echo pty-ready".into()], None, &[("LIGHT_PTY_TEST".into(), "from-env".into())], 80, 24, || {}).unwrap();
        let mut seen = Vec::new();
        let wait_for = |pty: &Pty, seen: &mut Vec<u8>, what: &str| {
            let start = Instant::now();
            while !String::from_utf8_lossy(seen).contains(what) {
                assert!(start.elapsed() < Duration::from_secs(20), "{what:?} never came: {:?}", String::from_utf8_lossy(seen));
                while let Some(event) = pty.try_recv() {
                    if let PtyEvent::Output(bytes) = event {
                        seen.extend(bytes);
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait_for(&pty, &mut seen, "pty-ready");
        pty.write(b"echo typed-%LIGHT_PTY_TEST%\r");
        wait_for(&pty, &mut seen, "typed-from-env");
        pty.write(b"exit 7\r");
        let start = Instant::now();
        loop {
            match pty.try_recv() {
                Some(PtyEvent::Exited(code)) => {
                    assert_eq!(code, Some(7));
                    break;
                }
                Some(_) => {}
                None => {
                    assert!(start.elapsed() < Duration::from_secs(20), "the program did not end");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}
