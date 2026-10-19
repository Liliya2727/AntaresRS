// ---------------- logging ----------------
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

pub(crate) const LOG_FILE: &str = "/data/adb/.libBinding/binder.log";
const DEBUG_FLAG: &str = "/data/adb/.libBinding/debug";
const LOG_MAX: u64 = 256 * 1024;

#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum Lv { Debug = 0, Info = 1, Warn = 2, Error = 3 }

struct Logger { file: File, size: u64 }
static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);
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

fn open_log() -> Option<Logger> {
    if fs::metadata(LOG_FILE).map(|m| m.len()).unwrap_or(0) > LOG_MAX {
        let _ = fs::rename(LOG_FILE, format!("{LOG_FILE}.old"));
    }
    let file = OpenOptions::new().create(true).append(true).open(LOG_FILE).ok()?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    Some(Logger { file, size })
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
    *LOGGER.lock().unwrap_or_else(|e| e.into_inner()) = open_log();
}

pub(crate) fn log_write(lv: Lv, msg: &str) {
    if (lv as u8) < LEVEL.load(Ordering::Relaxed) { return; }
    let tag = match lv { Lv::Debug => "D", Lv::Info => "I", Lv::Warn => "W", Lv::Error => "E" };
    let line = format!("{} {} libBinding: {}\n", ts(), tag, msg);
    let _ = std::io::stderr().write_all(line.as_bytes());
    let mut g = LOGGER.lock().unwrap_or_else(|e| e.into_inner());
    let rotate = match g.as_mut() {
        Some(l) => {
            let _ = l.file.write_all(line.as_bytes());
            l.size += line.len() as u64;
            l.size > LOG_MAX
        }
        None => false,
    };
    if rotate {
        *g = None; // close the old file
        let _ = fs::rename(LOG_FILE, format!("{LOG_FILE}.old"));
        *g = open_log();
    }
}
