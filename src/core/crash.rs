//! When the program crashes: a report of what happened is written to disk, and the program ends. The next time it
//! starts, it says where the report is. Crashes aren't recovered from: a crash in the middle of an experiment
//! likely means stopping it anyway, and the report is what's needed to fix it.
//!
//! Panics are caught by a panic hook, wherever they happen. On Windows, so are crashes in native code (the board's
//! and camera's libraries, say), by an unhandled exception filter, which also says which library it happened in.
//!
//! A report says which thread crashed, and which pass and node it was running, if it was a worker; the message, or
//! the exception; a backtrace; and the last of the log.

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::core::console;

/// Lines of the log a report ends with.
const LOG_LINES: usize = 300;
/// Names the latest report, until the program next starts and says where it is.
const LATEST: &str = "latest-crash.txt";

/// Where reports go: beside the app's saved preferences.
pub fn folder() -> Option<PathBuf> {
    eframe::storage_dir("ephys-rs").map(|d| d.join("crashes"))
}

/// Catches crashes from now on, to report them.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let message = info.payload_as_str().unwrap_or("(no message)");
        let location = info.location().map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
        report(&format!("Panicked{location}: {message}"));
        // Ending (with panics' usual code), not aborting, so the system doesn't say so on top.
        std::process::exit(101);
    }));
    #[cfg(windows)]
    native::install();
}

/// To try the reports: with `EPHYS_RS_TEST_CRASH` set to `panic`, or (to crash in native code) `native`, crashes
/// a couple of seconds after starting, on a thread of its own.
pub fn test_if_asked() {
    let Ok(kind) = std::env::var("EPHYS_RS_TEST_CRASH") else { return };
    let _ = std::thread::Builder::new().name("test crash".into()).spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        log::warn!("Crashing on purpose ({kind}), as EPHYS_RS_TEST_CRASH says");
        if kind == "native" {
            // Safety: none: it's meant to crash.
            unsafe { std::ptr::write_volatile(std::ptr::null_mut::<u32>(), 1) };
        }
        panic!("a test crash, as EPHYS_RS_TEST_CRASH says");
    });
}

/// The latest report, if the program crashed since it last started; forgetting it, so it's said once.
pub fn take_latest() -> Option<PathBuf> {
    let marker = folder()?.join(LATEST);
    let path = std::fs::read_to_string(&marker).ok()?;
    let _ = std::fs::remove_file(&marker);
    Some(PathBuf::from(path.trim()))
}

/// A file's URL, to open it: e.g. file:///C:/Users/... on Windows.
pub fn file_url(path: &std::path::Path) -> String {
    let path = path.display().to_string().replace('\\', "/").replace(' ', "%20");
    if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") }
}

/// Writes a report of a crash (`what` happened), and says where on stderr.
fn report(what: &str) {
    let thread = std::thread::current();
    let mut text = String::new();
    let _ = writeln!(text, "{} crashed, {}", crate::core::full_version(), chrono::Local::now().to_rfc2822());
    let _ = writeln!(text, "{} {}\n", std::env::consts::OS, std::env::consts::ARCH);
    let _ = writeln!(text, "{what}\n");
    let _ = writeln!(text, "Thread: {}", thread.name().unwrap_or("(unnamed)"));
    if let Some(pass) = console::current_pass() {
        let _ = write!(text, "Running pass {} of epoch {}", pass.sequence, pass.epoch);
        if let Some(node) = pass.node {
            let _ = write!(text, ", node {}", node.0);
        }
        if let Some(node_type) = pass.node_type {
            let _ = write!(text, " (a {node_type})");
        }
        text.push('\n');
    }
    let _ = writeln!(text, "\nBacktrace:\n{}", std::backtrace::Backtrace::force_capture());
    let _ = writeln!(text, "The log's last lines:");
    for line in console::recent(LOG_LINES) {
        let _ = writeln!(text, "{line}");
    }

    eprintln!("\n{what}");
    let Some(folder) = folder() else { return eprintln!("{text}") };
    let path = folder.join(format!("crash-{}.txt", chrono::Local::now().format("%Y-%m-%d-%H%M%S")));
    let written = std::fs::create_dir_all(&folder)
        .and_then(|()| std::fs::write(&path, &text))
        .and_then(|()| std::fs::write(folder.join(LATEST), path.display().to_string()));
    match written {
        Ok(()) => eprintln!("A report of the crash is in {}", path.display()),
        Err(e) => eprintln!("Couldn't write a report of the crash to {} ({e}):\n{text}", path.display()),
    }
}

/// Crashes in native code on Windows: access violations and the like, which aren't panics.
#[cfg(windows)]
mod native {
    use std::ffi::c_void;

    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        record: *mut ExceptionRecord,
        address: *mut c_void,
        parameters: u32,
        information: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        record: *const ExceptionRecord,
        context: *const c_void,
    }

    type Filter = unsafe extern "system" fn(*const ExceptionPointers) -> i32;

    const EXECUTE_HANDLER: i32 = 1;
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetUnhandledExceptionFilter(filter: Option<Filter>) -> Option<Filter>;
        fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
    }

    pub fn install() {
        // Safety: `filter` is a valid filter for the life of the program.
        unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    }

    /// The library (or program) `address` is in.
    fn module(address: *const c_void) -> Option<String> {
        let mut module = std::ptr::null_mut();
        let mut name = [0u16; 1024];
        // Safety: the buffers are as long as said.
        unsafe {
            if GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, address, &mut module) == 0 {
                return None;
            }
            let len = GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) as usize;
            (len > 0).then(|| String::from_utf16_lossy(&name[..len]))
        }
    }

    unsafe extern "system" fn filter(pointers: *const ExceptionPointers) -> i32 {
        // Safety: Windows passes a valid exception.
        let (code, address) = match unsafe { pointers.as_ref().and_then(|p| p.record.as_ref()) } {
            Some(record) => (record.code, record.address),
            None => (0, std::ptr::null_mut()),
        };
        let kind = match code {
            0xC000_0005 => "an access violation",
            0xC000_00FD => "a stack overflow",
            0xC000_001D => "an illegal instruction",
            0xC000_0094 => "an integer division by zero",
            0xC000_0409 => "a stack buffer overrun",
            0xC000_0374 => "heap corruption",
            _ => "an exception",
        };
        let place = module(address).map_or_else(String::new, |m| format!(", in {m}"));
        super::report(&format!("Crashed with {kind} (exception {code:#010X}) at {address:?}{place}"));
        EXECUTE_HANDLER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_of_files() {
        let mac = std::path::Path::new("/Users/me/Library/Application Support/ephys-rs/crashes/crash-1.txt");
        assert_eq!(file_url(mac), "file:///Users/me/Library/Application%20Support/ephys-rs/crashes/crash-1.txt");
        let windows = std::path::Path::new(r"C:\Users\me\AppData\Roaming\ephys-rs\data\crashes\crash-1.txt");
        assert_eq!(file_url(windows), "file:///C:/Users/me/AppData/Roaming/ephys-rs/data/crashes/crash-1.txt");
    }
}
