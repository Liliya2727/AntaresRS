// ---------------- profiler (profile switching, mirrors jni/src/profile/* in AZenith) ----------------
// run_profiler() in AZenith writes API/current_profile + API/gameinfo and then runs
// `sys.azenith-profilesettings <mode>`; the mode decision in the C daemon lives in the main
// loop. Here the same decision is made from binder events: a gamelist app in the foreground
// (IProcessObserver) -> performance, battery saver -> eco, everything else -> balanced,
// with the 10s screen-off grace period of the C loop.
use std::collections::BTreeSet;
use std::fs;
use std::os::raw::c_char;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::log::{debug, error, info, warn};
use crate::state::{saver, screen, write_atomic};

pub(crate) const API_DIR: &str = "/data/adb/.config/AntaresRS/API";
const PROFILE_FILE: &str = "/data/adb/.config/AntaresRS/API/current_profile";
const GAMEINFO_FILE: &str = "/data/adb/.config/AntaresRS/API/gameinfo";
const CUR_PROF: &str = "/data/adb/.config/AntaresRS/cur_prof";
const PROFILESETTINGS: &str = "sys.azenith-profilesettings";

// ProfileMode in AZenith.h
pub(crate) const MODE_PERF: u8 = 1;
pub(crate) const MODE_BALANCED: u8 = 2;
pub(crate) const MODE_ECO: u8 = 3;

const SCREEN_OFF_GRACE: Duration = Duration::from_secs(10);

static CUR: Mutex<u8> = Mutex::new(0); // 0 = nothing applied yet
static GRACE: Mutex<Option<Instant>> = Mutex::new(None); // screen-off grace period
static TX: Mutex<Option<mpsc::Sender<u8>>> = Mutex::new(None);

// Worker that runs sys.azenith-profilesettings. Exec'ing from the caller thread would
// block (and hold the state lock) for the whole run, so it is queued instead.
pub(crate) fn init() {
    let (tx, rx) = mpsc::channel::<u8>();
    *TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    thread::spawn(move || {
        let mut last = 0u8;
        while let Ok(mode) = rx.recv() {
            if mode == last { continue; }
            last = mode;
            run_profilesettings(mode);
        }
    });
}

fn queue_profilesettings(mode: u8) {
    let tx = TX.lock().unwrap_or_else(|e| e.into_inner()).as_ref().cloned();
    match tx {
        Some(t) => { let _ = t.send(mode); }
        None => run_profilesettings(mode), // init() not called yet
    }
}

pub(crate) fn mode_name(m: u8) -> &'static str {
    match m {
        0 => "none",
        MODE_PERF => "performance",
        MODE_BALANCED => "balanced",
        MODE_ECO => "eco",
        _ => "unknown",
    }
}

// Name of the profile currently applied (for state.toml).
pub(crate) fn cur_name() -> &'static str {
    mode_name(*CUR.lock().unwrap_or_else(|e| e.into_inner()))
}

fn time_str() -> String {
    unsafe {
        let mut t: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t.tv_sec, &mut tm);
        let mut buf = [0u8; 32];
        let n = libc::strftime(buf.as_mut_ptr() as *mut c_char, buf.len(), c"%H:%M:%S".as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

fn uid_of(pid: i32) -> i32 {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

// gameinfo: "<pkg> <pid> <uid>\nTime: HH:MM:SS\n" or "NULL 0 0\nTime: HH:MM:SS\n"
fn write_gameinfo(active: &Option<(String, BTreeSet<i32>)>) {
    let t = time_str();
    let body = match active {
        Some((pkg, pids)) => {
            let pid = pids.iter().next().copied().unwrap_or(0);
            format!("{pkg} {pid} {}\nTime: {t}\n", uid_of(pid))
        }
        None => format!("NULL 0 0\nTime: {t}\n"),
    };
    write_atomic(GAMEINFO_FILE, &body);
}

// (void)systemv("sys.azenith-profilesettings %d", profile);
fn run_profilesettings(mode: u8) {
    let arg = mode.to_string();
    let spawn = |bin: &str| {
        Command::new(bin)
            .arg(&arg)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status()
    };
    let mut status = spawn(PROFILESETTINGS);
    // the module may not be on PATH (eg. started from a different shell)
    if matches!(status, Err(ref e) if e.kind() == std::io::ErrorKind::NotFound) {
        for bin in [format!("/system/bin/{PROFILESETTINGS}"),
                    format!("/data/adb/modules/AZenith/system/bin/{PROFILESETTINGS}")] {
            match spawn(&bin) {
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                other => { status = other; break; }
            }
        }
    }
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => warn!("{PROFILESETTINGS} {mode} exited with {s}"),
        Err(e) => error!("failed to run {PROFILESETTINGS} {mode}: {e}"),
    }
}

fn apply(mode: u8, active: &Option<(String, BTreeSet<i32>)>, why: &str) {
    let from = {
        let mut c = CUR.lock().unwrap_or_else(|e| e.into_inner());
        let f = *c;
        *c = mode;
        f
    };
    info!("profile: [{}] -> [{}] ({why})", mode_name(from), mode_name(mode));
    write_gameinfo(active);
    write_atomic(PROFILE_FILE, &format!("{mode}\n"));
    write_atomic(CUR_PROF, &format!("{mode}\n"));
    queue_profilesettings(mode);
}

// Decide the profile from the current state. Called after every foreground/died/status change.
pub(crate) fn checkup(active: &Option<(String, BTreeSet<i32>)>, why: &str) {
    let cur = *CUR.lock().unwrap_or_else(|e| e.into_inner());
    let screen_off = screen() == "off";
    let saver_on = saver() == "on";

    let want = if active.is_some() && !screen_off {
        MODE_PERF
    } else if saver_on {
        MODE_ECO
    } else {
        MODE_BALANCED
    };

    // Screen off while a game is running: keep performance for 10s (grace period).
    let mut grace = GRACE.lock().unwrap_or_else(|e| e.into_inner());
    if cur == MODE_PERF && want != MODE_PERF && active.is_some() && screen_off {
        match *grace {
            None => {
                *grace = Some(Instant::now());
                info!("screen off grace period started (timeout=10s, keep performance)");
            }
            Some(t) => debug!("grace period active {:.0}s ({why})", t.elapsed().as_secs_f32()),
        }
        return;
    }
    *grace = None;

    if want == cur {
        debug!("profile unchanged [{}] ({why})", mode_name(want));
        return;
    }
    apply(want, active, why);
}

// Called from the status poll cycle: drops performance once the grace period expires.
pub(crate) fn tick() {
    let expired = {
        let g = GRACE.lock().unwrap_or_else(|e| e.into_inner());
        matches!(*g, Some(t) if t.elapsed() >= SCREEN_OFF_GRACE)
    };
    if !expired { return; }
    info!("grace period expired, dropping performance");
    *GRACE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let active = crate::state::st().active.clone();
    checkup(&active, "grace period expired");
}
