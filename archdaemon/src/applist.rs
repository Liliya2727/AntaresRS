// ---------------- applist.json (GameRegistry) + inotify (InotifyWatcher) ----------------
use std::collections::BTreeSet;
use std::ffi::c_void;
use std::fs;
use std::thread;

use crate::log::{debug, error, info, warn};
use crate::state::{adopt, st};

pub(crate) const LIST_DIR: &str = "/data/adb/.config/AZenith/gamelist";
const LIST_NAME: &str = "azenithApplist.json";
const LIST_FILE: &str = "/data/adb/.config/AZenith/gamelist/azenithApplist.json";

// Take only top-level keys (string before ':' at depth 1) without serde.
fn parse_keys(text: &str) -> Option<BTreeSet<String>> {
    let b = text.as_bytes();
    let mut set = BTreeSet::new();
    let (mut i, mut depth, mut opened) = (0usize, 0i32, false);
    while i < b.len() {
        match b[i] {
            b'{' | b'[' => {
                if !opened && b[i] != b'{' { return None; }
                opened = true;
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                if depth < 0 { return None; }
                i += 1;
            }
            b'"' => {
                i += 1;
                let mut buf = Vec::new();
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                        if i >= b.len() { return None; }
                    }
                    buf.push(b[i]);
                    i += 1;
                }
                if i >= b.len() { return None; }
                i += 1; // closing quote
                if depth == 1 {
                    let mut j = i;
                    while j < b.len() && b[j].is_ascii_whitespace() { j += 1; }
                    if b.get(j) == Some(&b':') {
                        set.insert(String::from_utf8_lossy(&buf).into_owned());
                    }
                }
            }
            _ => i += 1,
        }
    }
    (opened && depth == 0).then_some(set)
}

pub(crate) fn reload_list() {
    let text = match fs::read_to_string(LIST_FILE) {
        Ok(t) => t,
        Err(e) => { warn!("applist unreadable ({LIST_FILE}): {e}"); return; }
    };
    let Some(new) = parse_keys(&text) else {
        warn!("applist.json invalid ({} bytes), keeping the old list", text.len());
        return;
    };
    let total = new.len();
    let (added, removed, dropped) = {
        let mut g = st();
        let s = &mut *g;
        let added: Vec<String> = new.difference(&s.list).cloned().collect();
        let removed: Vec<String> = s.list.difference(&new).cloned().collect();
        s.list = new;
        // active app was removed from the applist -> fallback
        let mut dropped = None;
        if matches!(&s.active, Some((p, _)) if !s.list.contains(p)) {
            dropped = s.active.take().map(|(p, _)| p);
        }
        (added, removed, dropped)
    };
    info!("applist loaded: {total} packages (+{} -{})", added.len(), removed.len());
    if !added.is_empty() { info!("applist added: {}", added.join(", ")); }
    if !removed.is_empty() { info!("applist removed: {}", removed.join(", ")); }
    if let Some(p) = dropped { info!("active app {p} removed from applist, falling back"); }
    debug!("full applist: {}", st().list.iter().cloned().collect::<Vec<_>>().join(", "));
    adopt("applist changed");
}

// inotify on the directory (editors often replace files via rename). The thread sleeps in read().
pub(crate) fn watch_list() {
    thread::spawn(|| unsafe {
        let fd = libc::inotify_init1(libc::IN_CLOEXEC);
        if fd < 0 { error!("inotify_init1 failed"); return; }
        let dir = std::ffi::CString::new(LIST_DIR).unwrap();
        let wd = libc::inotify_add_watch(fd, dir.as_ptr(), libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO);
        if wd < 0 { error!("inotify_add_watch failed: {LIST_DIR}"); return; }
        info!("watching for changes to {LIST_DIR}/{LIST_NAME}");

        let mut buf = [0u8; 4096];
        loop {
            let r = libc::read(fd, buf.as_mut_ptr() as *mut c_void, buf.len());
            if r < 0 && *libc::__errno() == libc::EINTR { continue; }
            if r <= 0 { error!("inotify read stopped (r={r})"); return; }
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
                debug!("inotify: {LIST_NAME} changed");
                reload_list();
            }
        }
    });
}
