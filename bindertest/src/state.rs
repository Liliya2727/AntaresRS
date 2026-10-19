// ---------------- profile state + state.toml (state store, like the config store in encore) ----------------
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::log::{debug, error, info};

const PROF_FILE: &str = "/data/adb/.libBinding/cur_prof";
const ACTIVE_FILE: &str = "/data/adb/.libBinding/active_app";
const STATUS_FILE: &str = "/data/adb/.libBinding/state.toml";

pub(crate) struct St {
    pub(crate) list: BTreeSet<String>,                    // packages in the applist
    pub(crate) active: Option<(String, BTreeSet<i32>)>,   // (package, live pids) of the active app
    last: String,                                         // last state written to file
}
static ST: Mutex<St> = Mutex::new(St { list: BTreeSet::new(), active: None, last: String::new() });
pub(crate) fn st() -> MutexGuard<'static, St> { ST.lock().unwrap_or_else(|e| e.into_inner()) }

pub(crate) fn is_app(uid: i32) -> bool { (10000..20000).contains(&(uid % 100000)) }

// Only used in scan_foreground (start / applist change), not on the hot path.
fn pkg_of_pid(pid: i32) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let name = String::from_utf8_lossy(raw.split(|&b| b == 0).next()?).into_owned();
    (!name.is_empty() && !name.contains(':') && name.contains('.')).then_some(name)
}

fn write_atomic(path: &str, data: &str) -> bool {
    // Unique tmp name per call — with a fixed name, two threads writing at the same
    // time could eat each other's tmp file during rename (ENOENT error).
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let tmp = format!("{path}.{}.tmp", SEQ.fetch_add(1, Ordering::Relaxed));
    match fs::write(&tmp, data).and_then(|_| fs::rename(&tmp, path)) {
        Ok(()) => true,
        Err(e) => { error!("failed to write {path}: {e}"); false }
    }
}

// ---------------- state.toml: profile + system status in a single file ----------------
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct SysStat {
    pub(crate) screen: &'static str,  // on | off | unknown
    pub(crate) saver: &'static str,   // on | off | unknown
    pub(crate) zen: &'static str,     // off | priority | none | alarms | unknown
    pub(crate) charge: &'static str,  // charging | discharging | not_charging | full | unknown
    pub(crate) level: i32,            // 0..=100, -1 = unknown
}
const UNKNOWN_STAT: SysStat = SysStat { screen: "unknown", saver: "unknown", zen: "unknown", charge: "unknown", level: -1 };
static SYS: Mutex<SysStat> = Mutex::new(UNKNOWN_STAT);
static LAST_STATE: Mutex<String> = Mutex::new(String::new());

// Write only when the contents change.
pub(crate) fn write_state(prof: &str, pkg: &str, why: &str) {
    let s = SYS.lock().unwrap_or_else(|e| e.into_inner());
    let body = format!(
        "# libBinding state (auto-generated, do not edit manually)\n\
         profile = \"{prof}\"\n\
         active_app = \"{pkg}\"\n\
         screen = \"{screen}\"\n\
         battery_saver = \"{saver}\"\n\
         zen_mode = \"{zen}\"\n\
         battery_level = {level}\n\
         charging = \"{charge}\"\n",
        screen = s.screen, saver = s.saver, zen = s.zen, level = s.level, charge = s.charge,
    );
    drop(s);
    let mut last = LAST_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if *last == body { debug!("state.toml unchanged, skipping write [{why}]"); return; }
    if write_atomic(STATUS_FILE, &body) {
        *last = body;
        info!("state.toml updated ({why})");
    }
}

// Update one status field; if it changed, rewrite state.toml.
pub(crate) fn sys_update(why: &str, f: impl FnOnce(&mut SysStat)) {
    let changed = {
        let mut g = SYS.lock().unwrap_or_else(|e| e.into_inner());
        let before = *g;
        f(&mut *g);
        before != *g
    };
    if !changed { return; }
    let (prof, pkg) = {
        let g = st();
        match &g.active {
            Some((p, _)) => ("1", p.clone()),
            None => ("2", "NULL".to_string()),
        }
    };
    write_state(&prof, &pkg, why);
}

// Profile 1 = an applist app is active, profile 2 = fallback. Write only when it changes.
pub(crate) fn apply(s: &mut St, why: &str) {
    let (prof, pkg) = match &s.active {
        Some((p, _)) => ("1", p.as_str()),
        None => ("2", "NULL"),
    };
    let key = format!("{prof} {pkg}");
    if key != s.last {
        // active_app first, so a reader of cur_prof always gets a ready pair
        if write_atomic(ACTIVE_FILE, pkg) && write_atomic(PROF_FILE, prof) {
            let from = if s.last.is_empty() { "(none)" } else { s.last.as_str() };
            info!("profile: [{from}] -> [{key}] ({why})");
            s.last = key;
        }
    } else {
        debug!("state unchanged ({key}), skipping write [{why}]");
    }
    write_state(prof, pkg, why);
}

// App from the applist currently on screen (oom_score_adj 0). Only called on start / applist change.
fn scan_foreground(list: &BTreeSet<String>) -> Option<(i32, String)> {
    let rd = fs::read_dir("/proc").ok()?;
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        let Ok(md) = e.metadata() else { continue };
        if !is_app(md.uid() as i32) { continue; }
        let Some(pkg) = pkg_of_pid(pid) else { continue };
        if !list.contains(&pkg) { continue; }
        let top = fs::read_to_string(format!("/proc/{pid}/oom_score_adj"))
            .map(|v| v.trim() == "0").unwrap_or(false);
        if top { return Some((pid, pkg)); }
    }
    None
}

pub(crate) fn adopt(why: &str) {
    let mut g = st();
    let s = &mut *g;
    if s.active.is_none() {
        s.active = scan_foreground(&s.list).map(|(pid, pkg)| (pkg, BTreeSet::from([pid])));
        match &s.active {
            Some((pkg, pids)) => info!("initial scan: {pkg} (pid {pids:?}) is in the foreground"),
            None => debug!("initial scan: no applist app in the foreground"),
        }
    }
    apply(s, why);
}
