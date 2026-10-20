// ---------------- gamelist.txt (GameRegistry) + inotify (InotifyWatcher) ----------------
use std::collections::BTreeSet;
use std::ffi::c_void;
use std::fs;
use std::thread;

use crate::common::log::{comp, debug, error, info, warn};
use crate::daemon::state::{adopt, st};

pub(crate) const LIST_DIR: &str = "/data/adb/.config/AntaresRS/gamelist";
const LIST_NAME: &str = "gamelist.txt";
const LIST_FILE: &str = "/data/adb/.config/AntaresRS/gamelist/gamelist.txt";

// One package name per line. Blank lines, '#' comments and anything after whitespace
// are ignored, so "com.foo # shooter" and "com.foo" both give "com.foo".
fn parse_list(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .map(|l| l.split_whitespace().next().unwrap_or(""))
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

pub(crate) fn reload_list() {
    let text = match fs::read_to_string(LIST_FILE) {
        Ok(t) => t,
        Err(e) => { warn!(comp::APPLIST, "gamelist unreadable ({LIST_FILE}): {e}"); return; }
    };
    let new = parse_list(&text);
    let total = new.len();
    let (added, removed, dropped) = {
        let mut g = st();
        let s = &mut *g;
        let added: Vec<String> = new.difference(&s.list).cloned().collect();
        let removed: Vec<String> = s.list.difference(&new).cloned().collect();
        s.list = new;
        // active app was removed from the gamelist -> fallback
        let mut dropped = None;
        if matches!(&s.active, Some((p, _)) if !s.list.contains(p)) {
            dropped = s.active.take().map(|(p, _)| p);
        }
        (added, removed, dropped)
    };
    info!(comp::APPLIST, "gamelist loaded: {total} packages (+{} -{})", added.len(), removed.len());
    if !added.is_empty() { info!(comp::APPLIST, "gamelist added: {}", added.join(", ")); }
    if !removed.is_empty() { info!(comp::APPLIST, "gamelist removed: {}", removed.join(", ")); }
    if let Some(p) = dropped { info!(comp::APPLIST, "active app {p} removed from gamelist, falling back"); }
    debug!(comp::APPLIST, "full gamelist: {}", st().list.iter().cloned().collect::<Vec<_>>().join(", "));
    adopt("gamelist changed");
}

// inotify on the directory (editors often replace files via rename). The thread sleeps in read().
pub(crate) fn watch_list() {
    thread::spawn(|| unsafe {
        let fd = libc::inotify_init1(libc::IN_CLOEXEC);
        if fd < 0 { error!(comp::APPLIST, "inotify_init1 failed"); return; }
        let dir = std::ffi::CString::new(LIST_DIR).unwrap();
        let wd = libc::inotify_add_watch(fd, dir.as_ptr(), libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO);
        if wd < 0 { error!(comp::APPLIST, "inotify_add_watch failed: {LIST_DIR}"); return; }
        info!(comp::APPLIST, "watching for changes to {LIST_DIR}/{LIST_NAME}");

        let mut buf = [0u8; 4096];
        loop {
            let r = libc::read(fd, buf.as_mut_ptr() as *mut c_void, buf.len());
            if r < 0 && *libc::__errno() == libc::EINTR { continue; }
            if r <= 0 { error!(comp::APPLIST, "inotify read stopped (r={r})"); return; }
            let total = r as usize;
            let (mut off, mut hit) = (0usize, false);
            while off + 16 <= total {
                let len = u32::from_ne_bytes(buf[off + 12..off + 16].try_into().unwrap()) as usize;
                if off + 16 + len > total { break; }
                let name = buf[off + 16..off + 16 + len].split(|&b| b == 0).next().unwrap_or(&[]);
                if name == LIST_NAME.as_bytes() { hit = true; }
                off += 16 + len;
            }
            if hit {
                debug!(comp::APPLIST, "inotify: {LIST_NAME} changed");
                reload_list();
            }
        }
    });
}
