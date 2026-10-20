// ---------------- logging ----------------
//
// One writer for the whole service binary. The daemon, the binder callbacks
// and every companion command (`sys.azenith-service profilesettings ...`,
// `... utilityconf ...`, `... preferencedtweaks ...`, `... preload ...`) all
// append to the same two files through this module:
//
//   YYYY-MM-DD HH:MM:SS.mmm LEVEL [component] message
//
// Main log : /data/adb/.config/AntaresRS/AZenith.log
// DEBUG log: /data/adb/.config/AntaresRS/AZenithVerbose.log
//
// `component` is a dotted path naming the owner of the message. The first
// segment is the part of the module that emitted it (`daemon`, `cmd`,
// `common`), the rest narrows it to a subsystem:
//
//   [daemon.profiler]   [daemon.status]   [cmd.settings.cpu]
//   [cmd.utility.shell] [cmd.prefs.walt]  [cmd.preload]
//
// Use the constants in [`comp`] rather than a literal, so a message can never
// drift into an ad-hoc tag.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::raw::c_char;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::common::android_ffi;

pub(crate) const LOG_DIR: &str = "/data/adb/.config/AntaresRS";
pub(crate) const LOG_FILE: &str = "/data/adb/.config/AntaresRS/AZenith.log";
pub(crate) const VERBOSE_FILE: &str = "/data/adb/.config/AntaresRS/AZenithVerbose.log";
const DEBUG_FLAG: &str = "/data/adb/.config/AntaresRS/debug";

/// logcat tag for every line this binary prints.
const LOGCAT_TAG: &str = "AZenith";

/// Rotate once a file crosses this size, keeping LOG_KEEP_FILES generations
/// (<file>.1 .. <file>.N).
const LOG_MAX_BYTES: u64 = 512 * 1024;
const LOG_KEEP_FILES: usize = 2;

#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum Lv { Debug = 0, Info = 1, Warn = 2, Error = 3 }

static LEVEL: AtomicU8 = AtomicU8::new(Lv::Info as u8);

pub(crate) fn level_name() -> &'static str {
    ["debug", "info", "warn", "error"][LEVEL.load(Ordering::Relaxed) as usize]
}

/// Component tags, grouped by the module that owns them.
pub(crate) mod comp {
    // daemon: the binder engine itself.
    pub(crate) const DAEMON: &str = "daemon";
    pub(crate) const NDK: &str = "daemon.ndk";
    pub(crate) const CODES: &str = "daemon.codes";
    pub(crate) const STATUS: &str = "daemon.status";
    pub(crate) const OBSERVER: &str = "daemon.observer";
    pub(crate) const APPLIST: &str = "daemon.applist";
    pub(crate) const STATE: &str = "daemon.state";
    pub(crate) const PROFILER: &str = "daemon.profiler";

    // cmd.<command>.<subsystem>: the companion commands.
    pub(crate) const CMD: &str = "cmd";
    pub(crate) const PRELOAD: &str = "cmd.preload";
    pub(crate) const SETTINGS_CPU: &str = "cmd.settings.cpu";
    pub(crate) const SETTINGS_GPU: &str = "cmd.settings.gpu";
    pub(crate) const SETTINGS_IO: &str = "cmd.settings.io";
    pub(crate) const SETTINGS_RENDERER: &str = "cmd.settings.renderer";
    pub(crate) const SETTINGS_SYSFS: &str = "cmd.settings.sysfs";
    pub(crate) const SETTINGS_PROP: &str = "cmd.settings.prop";
    pub(crate) const SETTINGS_APPS: &str = "cmd.settings.apps";
    pub(crate) const SETTINGS_PROFILE: &str = "cmd.settings.profile";
    pub(crate) const UTILITY_CPU: &str = "cmd.utility.cpu";
    pub(crate) const UTILITY_GPU: &str = "cmd.utility.gpu";
    pub(crate) const UTILITY_IO: &str = "cmd.utility.io";
    pub(crate) const UTILITY_THERMAL: &str = "cmd.utility.thermal";
    pub(crate) const UTILITY_FSTRIM: &str = "cmd.utility.fstrim";
    pub(crate) const UTILITY_DND: &str = "cmd.utility.dnd";
    pub(crate) const UTILITY_REFRESH: &str = "cmd.utility.refresh";
    pub(crate) const UTILITY_RENDERER: &str = "cmd.utility.renderer";
    pub(crate) const UTILITY_SHELL: &str = "cmd.utility.shell";
    pub(crate) const PREFS_JIT: &str = "cmd.prefs.jit";
    pub(crate) const PREFS_SCHED: &str = "cmd.prefs.sched";
    pub(crate) const PREFS_WALT: &str = "cmd.prefs.walt";
    pub(crate) const PREFS_FPSGO: &str = "cmd.prefs.fpsgo";
    pub(crate) const PREFS_GPU: &str = "cmd.prefs.gpu";
    pub(crate) const PREFS_SF: &str = "cmd.prefs.sf";
    pub(crate) const PREFS_THERMAL: &str = "cmd.prefs.thermal";
    pub(crate) const PREFS_TRACE: &str = "cmd.prefs.trace";
    pub(crate) const PREFS_LOGD: &str = "cmd.prefs.logd";
    pub(crate) const PREFS_WRITE: &str = "cmd.prefs.write";

    // common: helpers shared by the daemon and the commands.
    pub(crate) const COMMON_FS: &str = "common.fs";
    pub(crate) const COMMON_PROP: &str = "common.prop";
}

/// Daemon logging macros. Every line is tagged `[daemon.*]`; pass the tag from
/// [`comp`] so a message names the module that produced it:
///
/// ```ignore
/// use crate::common::log::{comp, info, warn};
/// info!(comp::PROFILER, "profile [{}] -> [{}]", from, to);
/// ```
macro_rules! debug { ($component:expr, $($a:tt)*) => { $crate::common::log::log_write($crate::common::log::Lv::Debug, $component, &format!($($a)*)) }; }
macro_rules! info  { ($component:expr, $($a:tt)*) => { $crate::common::log::log_write($crate::common::log::Lv::Info,  $component, &format!($($a)*)) }; }
// different internal name: a plain `use warn` re-export clashes with the built-in `#[warn]` attribute
macro_rules! warn_internal { ($component:expr, $($a:tt)*) => { $crate::common::log::log_write($crate::common::log::Lv::Warn,  $component, &format!($($a)*)) }; }
macro_rules! error { ($component:expr, $($a:tt)*) => { $crate::common::log::log_write($crate::common::log::Lv::Error, $component, &format!($($a)*)) }; }
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

fn prio(lv: Lv) -> std::os::raw::c_int {
    match lv {
        Lv::Debug => android_ffi::ANDROID_LOG_DEBUG,
        Lv::Info => android_ffi::ANDROID_LOG_INFO,
        Lv::Warn => android_ffi::ANDROID_LOG_WARN,
        Lv::Error => android_ffi::ANDROID_LOG_ERROR,
    }
}

/// Mirror one line to logcat, tagged with the component so `logcat -s AZenith`
/// still reads like a file line.
fn logcat(lv: Lv, component: &str, message: &str) {
    android_ffi::logcat(prio(lv), LOGCAT_TAG, &format!("[{component}] {message}"));
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
/// pool, profiler worker, companion processes) with an exclusive `flock`.
fn append(path: &str, line: &str) {
    rotate_if_needed(path);

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else { return };
    let fd = file.as_raw_fd();
    android_ffi::lock_fd(fd);
    let _ = file.write_all(line.as_bytes());
    android_ffi::unlock_fd(fd);
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

/// `true` when DEBUG detail should be produced at all.
///
/// Two switches mean the same thing and either one enables it: the module
/// property `persist.sys.azenith.debugmode` (what a user flips) and the
/// [`DEBUG_FLAG`] file (what `log_init` also reads). Keeping them in one
/// function is what stops the level and the gate from disagreeing — with only
/// the flag file, the level was Debug but every DEBUG line was still dropped.
pub(crate) fn debugmode() -> bool {
    fs::metadata(DEBUG_FLAG).is_ok()
        || crate::common::prop::getprop("persist.sys.azenith.debugmode") == "true"
}
/// Single funnel for every level, so the on-disk shape stays identical
/// whichever module emitted the line.
pub(crate) fn log_write(lv: Lv, component: &str, msg: &str) {
    if (lv as u8) < LEVEL.load(Ordering::Relaxed) { return; }
    // DEBUG detail never reaches the main log, so a command that is invoked
    // (or not) does not have to check the flag itself.
    if matches!(lv, Lv::Debug) && !debugmode() { return; }

    let level = match lv { Lv::Debug => "DEBUG", Lv::Info => "INFO", Lv::Warn => "WARN", Lv::Error => "ERROR" };
    let component = if component.is_empty() { comp::DAEMON } else { component };

    let _ = fs::create_dir_all(LOG_DIR);
    let line = format!("{} {} [{}] {}\n", ts(), level, component, msg);
    append(if matches!(lv, Lv::Debug) { VERBOSE_FILE } else { LOG_FILE }, &line);
    logcat(lv, component, msg);
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
