// ---------------- logging ----------------
//
// Shared log shape for every AntaresRS binary (archdaemon, binutils,
// binprofiles, binpreferenced):
//
//   YYYY-MM-DD HH:MM:SS.mmm LEVEL [component] message
//
// Main log : /data/adb/.config/AntaresRS/AZenith.log
// DEBUG log: /data/adb/.config/AntaresRS/AZenithVerbose.log
//
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::raw::{c_char, c_int};
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicU8, Ordering};

pub(crate) const LOG_DIR: &str = "/data/adb/.config/AntaresRS";
pub(crate) const LOG_FILE: &str = "/data/adb/.config/AntaresRS/AZenith.log";
pub(crate) const VERBOSE_FILE: &str = "/data/adb/.config/AntaresRS/AZenithVerbose.log";
const DEBUG_FLAG: &str = "/data/adb/.config/AntaresRS/debug";

/// Component tag bracketed in every line.
const COMPONENT: &str = "daemon";

/// Rotate once a file crosses this size, keeping LOG_KEEP_FILES generations
/// (<file>.1 .. <file>.N) — same policy as the companion binaries.
const LOG_MAX_BYTES: u64 = 512 * 1024;
const LOG_KEEP_FILES: usize = 2;

const ANDROID_LOG_DEBUG: c_int = 3;
const ANDROID_LOG_INFO: c_int = 4;
const ANDROID_LOG_WARN: c_int = 5;
const ANDROID_LOG_ERROR: c_int = 6;

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_print(prio: c_int, tag: *const c_char, fmt: *const c_char, ...) -> c_int;
}

#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum Lv { Debug = 0, Info = 1, Warn = 2, Error = 3 }

static LEVEL: AtomicU8 = AtomicU8::new(Lv::Info as u8);

pub(crate) fn level_name() -> &'static str {
    ["debug", "info", "warn", "error"][LEVEL.load(Ordering::Relaxed) as usize]
}

macro_rules! debug { ($($a:tt)*) => { $crate::log::log_write($crate::log::Lv::Debug, &format!($($a)*)) }; }
macro_rules! info  { ($($a:tt)*) => { $crate::log::log_write($crate::log::Lv::Info,  &format!($($a)*)) }; }
// different internal name: a plain `use warn` re-export clashes with the built-in `#[warn]` attribute
macro_rules! warn_internal { ($($a:tt)*) => { $crate::log::log_write($crate::log::Lv::Warn,  &format!($($a)*)) }; }
macro_rules! error { ($($a:tt)*) => { $crate::log::log_write($crate::log::Lv::Error, &format!($($a)*)) }; }
pub(crate) use debug;
pub(crate) use error;
pub(crate) use info;
pub(crate) use warn_internal as warn;

fn ts() -> String {
    unsafe {
        let mut t: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t.tv_sec, &mut tm);
        let mut buf = [0u8; 32];
        let n = libc::strftime(buf.as_mut_ptr() as *mut c_char, buf.len(), c"%Y-%m-%d %H:%M:%S".as_ptr(), &tm);
        format!("{}.{:03}", String::from_utf8_lossy(&buf[..n]), t.tv_nsec / 1_000_000)
    }
}

fn prio(lv: Lv) -> c_int {
    match lv {
        Lv::Debug => ANDROID_LOG_DEBUG,
        Lv::Info => ANDROID_LOG_INFO,
        Lv::Warn => ANDROID_LOG_WARN,
        Lv::Error => ANDROID_LOG_ERROR,
    }
}

/// Mirror one line to logcat. The body goes through `%s` so a literal `%`
/// inside a message can never be treated as a format specifier.
fn logcat(lv: Lv, message: &str) {
    let Ok(tag) = CString::new(COMPONENT) else { return };
    let Ok(msg) = CString::new(message.replace('\0', "")) else { return };
    let fmt = c"%s";
    unsafe {
        (__android_log_print)(prio(lv), tag.as_ptr(), fmt.as_ptr(), msg.as_ptr());
    }
}

/// Shift <path> -> <path>.1 -> <path>.N once it exceeds LOG_MAX_BYTES. On any
/// failure we just keep appending; logging must never take the daemon down.
fn rotate_if_needed(path: &str) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() < LOG_MAX_BYTES {
        return;
    }

    for i in (1..=LOG_KEEP_FILES).rev() {
        let src = if i == 1 { path.to_string() } else { format!("{}.{}", path, i - 1) };
        let dst = format!("{}.{}", path, i);

        let _ = fs::remove_file(&dst);
        let _ = fs::rename(&src, &dst);
    }
}

/// Append one line, serialized against concurrent writers (binder thread
/// pool, profiler worker, companion binaries) with an exclusive `flock`.
fn append(path: &str, line: &str) {
    rotate_if_needed(path);

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else { return };
    let fd = file.as_raw_fd();
    unsafe { libc::flock(fd, libc::LOCK_EX) };
    let _ = file.write_all(line.as_bytes());
    unsafe { libc::flock(fd, libc::LOCK_UN) };
}

pub(crate) fn log_init() {
    let lv = match std::env::var("LIBBINDING_LOG").ok().as_deref() {
        Some("debug") => Lv::Debug,
        Some("info") => Lv::Info,
        Some("warn") => Lv::Warn,
        Some("error") => Lv::Error,
        _ if fs::metadata(DEBUG_FLAG).is_ok() => Lv::Debug,
        _ => Lv::Info,
    };
    LEVEL.store(lv as u8, Ordering::Relaxed);
}

pub(crate) fn log_write(lv: Lv, msg: &str) {
    if (lv as u8) < LEVEL.load(Ordering::Relaxed) { return; }
    let level = match lv { Lv::Debug => "DEBUG", Lv::Info => "INFO", Lv::Warn => "WARN", Lv::Error => "ERROR" };
    let _ = fs::create_dir_all(LOG_DIR);
    let line = format!("{} {} [{}] {}\n", ts(), level, COMPONENT, msg);
    // DEBUG detail goes to the verbose log so the main log stays readable.
    append(if matches!(lv, Lv::Debug) { VERBOSE_FILE } else { LOG_FILE }, &line);
    logcat(lv, &format!("[{COMPONENT}] {msg}"));
}

/// Drop every generation of both log files (`--clearlogs`).
pub(crate) fn clear_logs() {
    for path in [LOG_FILE, VERBOSE_FILE] {
        let _ = fs::remove_file(path);
        for i in 1..=LOG_KEEP_FILES {
            let _ = fs::remove_file(format!("{path}.{i}"));
        }
    }
}
