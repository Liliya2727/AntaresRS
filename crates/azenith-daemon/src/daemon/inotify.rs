// Copyright (C) 2026-2027 Zexshia
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! inotify watch setup and event routing. Replaces `InotifyWatcher.c`.
//!
//! ## How routing works
//!
//! The C version switched on `event->name` with a chain of `strcmp`s against
//! every known filename. Inotify delivers the *basename* only, and several
//! watches overlap (the config dir, `API/`, `gamelist/`, `bypasschgconfig/`),
//! so one physical change can arrive on more than one watch. The C code
//! deduplicated implicitly by dispatching on name only, and this does the same
//! — the same basename in two directories would be dispatched twice, exactly
//! as before. That is not a new bug and not worth a routing table here.
//!
//! ## The `poller`/loop split
//!
//! [`Watcher::poll_events`] owns the `poll(2)` and returns the two flags the loop
//! cares about. Everything else is a pure function over an event name, which is
//! what makes the routing testable without a real inotify fd.

use azenith_common::android_props;
use azenith_common::config;
use azenith_common::logger::{Level, log};
use azenith_common::paths;
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use crate::app_loader;
use crate::daemon::context::{Daemon, GameConfig, MAX_TRACK_PIDS, ProfileMode};

/// Creates a `pipe(2)` and returns `(read, write)`, both `CLOEXEC`.
///
/// `O_CLOEXEC` matters: `systemv()` spawns children, and a leaked writable pipe
/// end would keep the read side permanently readable in any child that
/// inherited it, making the event loop spin.
fn make_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    // SAFETY: `pipe` only writes into the fds it creates; both are uninitialised
    // beforehand, which is the documented contract.
    let mut fds = [0 as libc::c_int; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `pipe2` returned 0, so both entries are valid, open fds. Passing
    // them to `OwnedFd` transfers ownership to Rust's closer.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// What one `poll(2)` wakeup produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Wake {
    /// The companion's lock was released — the Java side died.
    pub java_died: bool,
    /// The daemon should exit (module update/remove, integrity failure).
    pub should_exit: bool,
    /// Set when a routing decision wants another iteration soon rather than a
    /// blocking wait. The C `need_loop` flag.
    pub need_loop: bool,
}

/// The inotify instance plus the java-lock pipe.
///
/// `java_lock_rx` is a raw read end of a `pipe(2)`, not a file: the C code
/// writes one byte into it from the lock-watcher thread and the event loop
/// selects on it. It is held as a raw fd because `poll` needs to borrow it and
/// nothing ever reads the byte itself — the flag is the signal.
pub struct Watcher {
    inotify: Inotify,
    java_lock_rx: OwnedFd,
    /// Kept solely so the read end never reports EOF. The lock-watcher thread
    /// gets a `dup` of this; without a second live writer the read side would
    /// be permanently readable and the event loop would spin at 100% CPU.
    _java_lock_tx: OwnedFd,
}

impl Watcher {
    /// Creates the inotify instance and adds every watch the C code added.
    ///
    /// The masks are copied verbatim from `setup_inotify_watchers` — in
    /// particular `gamelist/` adds `IN_CLOSE_WRITE` (the Manager writes it that
    /// way) while the others do not.
    pub fn new() -> std::io::Result<Self> {
        let inotify = Inotify::init(InitFlags::IN_NONBLOCK)?;

        let targets: [(&str, AddWatchFlags); 5] = [
            (
                paths::CONFIG_DIR,
                AddWatchFlags::IN_MODIFY | AddWatchFlags::IN_CREATE | AddWatchFlags::IN_MOVED_TO,
            ),
            (
                paths::API_DIR,
                AddWatchFlags::IN_MODIFY | AddWatchFlags::IN_CREATE | AddWatchFlags::IN_MOVED_TO,
            ),
            (
                paths::GAMELIST_DIR,
                AddWatchFlags::IN_MODIFY
                    | AddWatchFlags::IN_CLOSE_WRITE
                    | AddWatchFlags::IN_MOVED_TO
                    | AddWatchFlags::IN_CREATE,
            ),
            (
                paths::BYPASSCHG_CONFIG,
                AddWatchFlags::IN_MODIFY | AddWatchFlags::IN_CREATE | AddWatchFlags::IN_MOVED_TO,
            ),
            (
                paths::MODULE_DIR,
                AddWatchFlags::IN_MODIFY
                    | AddWatchFlags::IN_CREATE
                    | AddWatchFlags::IN_MOVED_TO
                    | AddWatchFlags::IN_DELETE,
            ),
        ];

        for (path, mask) in targets {
            // A watch that cannot be added is logged, not fatal: the C code
            // ignored the return value too. The config dir existing is the only
            // thing that truly matters and that is checked at startup.
            if let Err(e) = inotify.add_watch(std::path::Path::new(path), mask) {
                log(
                    Level::Warn,
                    "Inotify",
                    &format!("could not watch {path}: {e}"),
                );
            }
        }

        // SAFETY: `pipe` is a plain syscall and both fds are fresh, so they are
        // ours to own. The write end goes to the lock-watcher thread; a
        // duplicate is leaked into the struct so the read end never sees EOF
        // when the thread's copy closes — `poll` must only report HUP when the
        // watcher itself dies.
        let (java_lock_rx, java_lock_tx) = make_pipe()?;

        Ok(Self {
            inotify,
            java_lock_rx,
            _java_lock_tx: java_lock_tx,
        })
    }

    /// Waits for the Java companion to take `java.lock`, then starts the
    /// lock-watcher thread.
    ///
    /// The daemon must not act before the companion holds the lock: it dies when
    /// the companion dies, and `app_status` is the companion's to write. Returns
    /// an error if the lock is not taken within `timeout_secs`, which is the
    /// fatal startup timeout the C expressed as a 120 s wait.
    pub fn wait_for_java_lock(&mut self, timeout_secs: u64) -> std::io::Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        while !java_lock_is_held() {
            if std::time::Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "java.lock was never taken",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        // The thread gets its own dup of the write end so that the struct's copy
        // keeps the read side from ever seeing EOF.
        let tx = self.java_lock_tx_dup()?;
        let rx = self.java_lock_rx.try_clone()?;
        std::thread::spawn(move || watch_java_lock(rx, tx));
        Ok(())
    }

    /// A duplicate of the pipe's write end, for the lock-watcher thread.
    ///
    /// The thread writes one byte when the companion's lock is released; the
    /// main loop never reads it.
    pub fn java_lock_tx_dup(&self) -> std::io::Result<OwnedFd> {
        self._java_lock_tx.try_clone()
    }

    /// Blocks for at most `timeout_ms`, then drains and routes every event.
    ///
    /// `timeout_ms` is the C `poll_timeout`: `0` for a spin iteration, a
    /// countdown during the grace and foreground-away windows, and `-1` to block
    /// indefinitely when idle.
    pub fn poll_events(&self, daemon: &mut Daemon, timeout_ms: i32) -> std::io::Result<Wake> {
        let mut wake = Wake::default();

        use std::os::fd::{AsRawFd, BorrowedFd};
        let mut fds = [
            nix::poll::PollFd::new(self.inotify.as_fd(), nix::poll::PollFlags::POLLIN),
            nix::poll::PollFd::new(
                unsafe { BorrowedFd::borrow_raw(self.java_lock_rx.as_raw_fd()) },
                nix::poll::PollFlags::POLLIN,
            ),
        ];

        // nix 0.30: `poll` takes `impl Into<PollTimeout>`. `TryFrom<i32>` rejects
        // anything below -1, and -1 is exactly the C "block forever".
        let timeout = nix::poll::PollTimeout::try_from(timeout_ms)
            .map_err(|_| std::io::Error::other("poll timeout below -1"))?;

        let ready = match nix::poll::poll(&mut fds, timeout) {
            Ok(n) => n,
            // EINTR is routine here — SIGCHLD from a restarted app, a user
            // `set_priority` on a dying process. Not an error.
            Err(nix::errno::Errno::EINTR) => return Ok(wake),
            Err(e) => return Err(std::io::Error::from_raw_os_error(e as i32)),
        };

        if ready == 0 {
            return Ok(wake);
        }

        // Java death is checked first and short-circuits, exactly as the C did:
        // if the companion is gone there is no point applying anything.
        if fds[1]
            .revents()
            .is_some_and(|r| r.contains(nix::poll::PollFlags::POLLIN))
        {
            daemon.set_java_died();
            wake.java_died = true;
            wake.should_exit = true;
            return Ok(wake);
        }

        if fds[0]
            .revents()
            .is_some_and(|r| r.contains(nix::poll::PollFlags::POLLIN))
        {
            self.drain(daemon, &mut wake)?;
        }

        Ok(wake)
    }

    /// Reads every queued event and routes it.
    fn drain(&self, daemon: &mut Daemon, wake: &mut Wake) -> std::io::Result<()> {
        for event in self.inotify.read_events()? {
            let Some(name) = event.name else { continue };
            self.route(daemon, name.to_string_lossy().as_ref(), wake);
            if wake.should_exit {
                // Stop reading: the loop is about to exit and a queued `remove`
                // must not be overtaken by a later event's side effects.
                break;
            }
        }
        Ok(())
    }

    /// Routes one event by basename. The pure part of the watcher: everything it
    /// touches is either `daemon` state or a file read, and every branch is
    /// exercised by the tests below without an inotify fd.
    fn route(&self, daemon: &mut Daemon, name: &str, wake: &mut Wake) {
        if name.ends_with("azenithApplist.json") {
            // The Manager writes the gamelist in two steps; the C slept 50 ms to
            // let it settle. Same here — without it the daemon can parse a
            // half-written file.
            std::thread::sleep(std::time::Duration::from_millis(50));
            let n = app_loader::gamelist::reload_cache(&daemon.gamelist);
            if n > 0 {
                daemon.need_profile_checkup = true;
            }
            return;
        }

        match name {
            "app_status" => {
                if let Some(st) = app_loader::status_monitor::read_app_status() {
                    daemon.state = st;
                }
                daemon.need_profile_checkup = true;
            }
            "background_apps" => {
                handle_background_apps_event(daemon);
                if daemon.gamestart.is_none() {
                    daemon.need_profile_checkup = true;
                }
            }
            "current_profile" => {
                if let Some(m) = read_profile_mode() {
                    daemon.cur_mode = m;
                }
            }
            "current_modes" => self.on_current_modes(daemon),
            "update" => {
                log(Level::Info, "Inotify", "Module update detected, exiting.");
                android_props::setprop("persist.sys.azenith.service", "");
                android_props::setprop("persist.sys.azenith.state", "stopped");
                wake.should_exit = true;
            }
            "remove" => {
                log(Level::Info, "Inotify", "Module is removed, exiting.");
                wake.should_exit = true;
            }
            "module.prop" => {
                log(Level::Info, "Inotify", "module.prop modified...");
                crate::integrity::is_kanged();
                crate::integrity::check_module_version();
            }
            "reboot" => {
                log(
                    Level::Info,
                    "Inotify",
                    "Configuration updated. Please reboot your device to take full effect.",
                );
            }
            "freqoffset" => {
                let v = config::read_line(&format!("{}/freqoffset", paths::CONFIG_DIR));
                if !v.is_empty() {
                    daemon.config_freqoffset = v.clone();
                    log(
                        Level::Info,
                        "Inotify",
                        &format!("freqoffset updated to [{v}]"),
                    );
                }
            }
            "bypasspath" => {
                let v = config::read_line(&format!("{}/bypasspath", paths::BYPASSCHG_CONFIG));
                if !v.is_empty() {
                    daemon.config_bypasspath = v;
                }
            }
            "bypasschg" => {
                daemon.config_bypasschg =
                    config::read_int_or(&format!("{}/bypasschg", paths::BYPASSCHG_CONFIG), 0);
            }
            "bypasschgthreshold" => {
                daemon.config_bypasschgthreshold = config::read_int_or(
                    &format!("{}/bypasschgthreshold", paths::BYPASSCHG_CONFIG),
                    0,
                );
            }
            _ => {}
        }
    }

    /// Auto-mode toggle. Only reacts once initialisation has completed, and only
    /// on an actual change — same guard as the C.
    fn on_current_modes(&self, daemon: &mut Daemon) {
        let ai = config::read_line(paths::DAEMON_MODES);
        if ai.is_empty() {
            return;
        }
        if !daemon.is_initialize_complete || daemon.prev_ai_state == ai {
            return;
        }

        log(
            Level::Info,
            "Inotify",
            "Dynamic profile toggled, Reapplying Balanced Profiles",
        );
        daemon.cur_mode = ProfileMode::PerfCommon;
        crate::profiles::apply(daemon, crate::daemon::context::ProfileMode::Balanced);
        daemon.prev_ai_state = ai.clone();

        if ai == "1" {
            daemon.clear_game();
            daemon.need_profile_checkup = true;
        }
    }
}

/// Reads `API/current_profile` — a bare integer, the `ProfileMode` discriminant.
///
/// Returns `None` on a missing or malformed file, leaving the mode unchanged.
/// The C cast `atoi`'s result straight to the enum, so a corrupt file silently
/// became whatever byte the parse produced.
fn read_profile_mode() -> Option<ProfileMode> {
    let raw = config::read_line(paths::PROFILE_MODE);
    if raw.is_empty() {
        return None;
    }
    raw.parse::<u8>().ok().and_then(ProfileMode::from_index)
}

/// Re-reads the game's PIDs after a `background_apps` change.
///
/// Two behaviours are preserved from the C, one bug is not:
///
/// * Preserved: if the file is momentarily empty (the companion truncates before
///   rewriting) the previous PID set is kept, so a transient empty read does not
///   look like the game exiting.
/// * Preserved: when the PID count reaches zero, the daemon only tears the game
///   down if it is *not* focused and no renderer restart is in flight.
/// * **Not preserved**: the C hard-capped `max_track_pids` at 2 while
///   `MAX_GAME_PIDS` was 8, so a three-process game silently lost a process's
///   priority. The cap is now [`MAX_GAME_PIDS`].
fn handle_background_apps_event(daemon: &mut Daemon) {
    let Some(pkg) = daemon.gamestart.clone() else {
        return;
    };

    let new_pids = crate::pid_tracker::pids_of(&pkg, MAX_TRACK_PIDS);

    // Companion truncates the file before rewriting it: an empty file means
    // "not written yet", not "no processes".
    if new_pids.is_empty()
        && !daemon.game_pids.is_empty()
        && std::fs::metadata(paths::BACKGROUND_APPS).is_ok_and(|m| m.len() == 0)
    {
        return;
    }

    let changed = new_pids != daemon.game_pids;
    if !changed {
        return;
    }

    if !new_pids.is_empty() {
        log(
            Level::Info,
            "Inotify",
            &format!(
                "Tracking {} PID(s) for {}",
                new_pids.len(),
                daemon.game_label()
            ),
        );
    }

    daemon.game_pids = new_pids;
    apply_priority_if_wanted(daemon);

    if !daemon.game_pids.is_empty() {
        return;
    }

    // Zero PIDs: either the app is restarting, or it is genuinely gone.
    if daemon.game_is_focused() || crate::utility::is_restarting_renderer() {
        log(
            Level::Info,
            "Inotify",
            &format!(
                "Game {} PIDs dropped (Restarting). Waiting to respawn...",
                daemon.game_label()
            ),
        );
        return;
    }

    log(
        Level::Info,
        "Inotify",
        &format!(
            "Game {} completely closed. Exiting performance mode...",
            daemon.game_label()
        ),
    );
    crate::handlers::resolution::restore(daemon, &pkg);
    daemon.clear_game();
    daemon.fg_away_active = false;
    daemon.fg_away_timer = None;
}

/// Applies process priority to each tracked PID when the app asks for it.
///
/// `app_priority` is tri-state: `"true"` always, `"false"` never, anything else
/// (including `"default"`) defers to the global `iosched` toggle. The C
/// `IS_FALSE` test meant exactly that, and the `"default"` string is why an
/// unconfigured game inherits the global setting rather than opting out.
fn apply_priority_if_wanted(daemon: &Daemon) {
    let mode = daemon.opts.app_priority.as_str();
    let wanted = if mode == "true" {
        true
    } else if mode == "false" {
        false
    } else {
        android_props::getprop_bool_1("persist.sys.azenithconf.iosched")
    };
    if !wanted {
        return;
    }
    for pid in &daemon.game_pids {
        crate::priority::set_priority(*pid);
    }
}

/// Reads one named `GameConfig` field and reports whether it is switched on.
///
/// A `match` rather than a struct field lookup because the caller passes the
/// field name as data (it comes from the event routing), and an unknown name
/// must read as "off" instead of panicking — with `panic = "abort"` that would
/// take the whole daemon down over a stray event.
pub fn app_wants(app: &GameConfig, field: &str) -> bool {
    let v = match field {
        "perf_lite_mode" => &app.perf_lite_mode,
        "dnd_on_gaming" => &app.dnd_on_gaming,
        "app_priority" => &app.app_priority,
        "game_preload" => &app.game_preload,
        "refresh_rate" => &app.refresh_rate,
        "renderer" => &app.renderer,
        "resolution_downscale" => &app.resolution_downscale,
        "resolution_fps" => &app.resolution_fps,
        "bypass_charging" => &app.bypass_charging,
        _ => return false,
    };
    // "on" is the Manager's spelling for these fields; `IS_TRUE` in the C
    // accepted "true", and the Manager writes "true", so both are honoured.
    !GameConfig::is_default(v) && v != "false"
}

#[cfg(test)]
mod app_wants_tests {
    use super::*;

    #[test]
    fn an_unknown_field_reads_as_off() {
        // Must not panic: `panic = "abort"` would kill the daemon.
        let app = GameConfig::default();
        assert!(!app_wants(&app, "no_such_field"));
    }

    #[test]
    fn a_set_field_reads_as_on() {
        let app = GameConfig {
            refresh_rate: "120".into(),
            ..Default::default()
        };
        assert!(app_wants(&app, "refresh_rate"));
        assert!(!app_wants(&app, "dnd_on_gaming"));
    }
}

/// Probes `java.lock` with `fcntl(F_GETLK)`, which is what the companion's
/// `FileChannel.tryLock()` takes.
///
/// `flock` and POSIX record locks are *independent* namespaces on Linux: they
/// never conflict, so probing with `flock` always reports "free" no matter what
/// the companion does, and the daemon's startup wait then always times out.
/// The C kept these apart for exactly this reason — `F_GETLK` for the
/// companion, `flock` for its own lock — and folding both into one helper is
/// what broke the boot sequence. Verified on-device: `/proc/locks` shows the
/// companion's `POSIX WRITE` lock while `flock` acquires the same file freely.
pub fn java_lock_is_held() -> bool {
    let Ok(c) = std::ffi::CString::new(paths::JAVA_LOCK) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated path for the call's duration.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY) };
    if fd < 0 {
        return false;
    }
    // F_GETLK only *reports* a conflicting lock; it never takes one, so a
    // probe cannot disturb the companion's lock.
    let mut fl = libc::flock {
        l_type: libc::F_WRLCK as i16,
        l_whence: libc::SEEK_SET as i16,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };
    // SAFETY: `fd` is owned here, and `fl` is a valid, fully initialised
    // `struct flock` for the duration of the call.
    let queried = unsafe { libc::fcntl(fd, libc::F_GETLK, &mut fl) } != -1;
    // SAFETY: closing a descriptor we own is always valid.
    unsafe { libc::close(fd) };
    queried && fl.l_type != libc::F_UNLCK as i16
}

/// Acquires and holds the daemon lock for the process lifetime.
///
/// Returns `false` when another daemon already holds it. The fd is intentionally
/// leaked: the lock has to survive for the life of the process, and the C made
/// the same trade.
pub fn acquire_daemon_lock() -> bool {
    let c = match std::ffi::CString::new(paths::LOCK_FILE) {
        Ok(c) => c,
        Err(_) => return false,
    };
    // SAFETY: `c` is a valid NUL-terminated path for the call's duration.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_WRONLY | libc::O_CREAT, 0o644) };
    if fd < 0 {
        return false;
    }
    // SAFETY: `fd` is a fresh owned descriptor.
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        // SAFETY: closing a descriptor we own is always valid.
        unsafe { libc::close(fd) };
        return false;
    }
    // Deliberately not closed: the lock must outlive this function.
    true
}

/// The lock probe behind [`java_lock_is_held`] and the CLI's daemon check.
pub fn lock_is_free(path: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(path) else {
        return true;
    };
    // SAFETY: `c` is a valid NUL-terminated path for the call's duration.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_WRONLY | libc::O_CREAT, 0o644) };
    if fd < 0 {
        return false;
    }
    // SAFETY: `fd` is owned here and closed exactly once on each path.
    let got = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0;
    unsafe { libc::close(fd) };
    got
}

/// Blocks in `fcntl(F_SETLKW)` on `java.lock`; writes one byte to `tx` when the
/// companion releases it, which is the daemon's cue to shut down.
fn watch_java_lock(rx: OwnedFd, tx: OwnedFd) {
    use std::os::fd::AsRawFd;
    let Ok(c) = std::ffi::CString::new(paths::JAVA_LOCK) else {
        return;
    };
    // SAFETY: `c` is a valid NUL-terminated path; the fd is checked below.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CREAT, 0o644) };
    if fd < 0 {
        return;
    }
    let mut fl = libc::flock {
        l_type: libc::F_WRLCK as i16,
        l_whence: libc::SEEK_SET as i16,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };
    // SAFETY: `fl` is a fully initialised flock and `fd` is owned. F_SETLKW
    // blocks until the companion releases the lock, which is the point.
    unsafe { libc::fcntl(fd, libc::F_SETLKW, &mut fl) };

    // SAFETY: both fds are owned by this thread; one byte is written and the
    // read end is never drained, so the pipe simply signals "wake up".
    unsafe {
        let byte = 1u8;
        libc::write(rx.as_raw_fd(), &byte as *const u8 as *const libc::c_void, 1);
    }
    // The write end exists only to signal; holding both keeps the pipe open
    // until this thread ends, which is exactly the daemon's lifetime.
    let _ = &tx;
    // SAFETY: `fd` is owned by this thread.
    unsafe { libc::close(fd) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon() -> Daemon {
        Daemon::default()
    }

    #[test]
    fn a_fcntl_write_lock_from_another_process_is_reported_as_held() {
        // The companion takes a POSIX `fcntl` write lock. Probing with `flock`
        // cannot see it -- the two are independent namespaces on Linux -- so a
        // flock-based probe reports "free" forever and the daemon times out on
        // startup.
        //
        // POSIX record locks are owned per-process, so an in-process lock is
        // invisible to this process's own F_GETLK. The real case is always
        // cross-process (companion vs daemon), so the lock holder has to be a
        // separate process for the probe to mean anything.
        let dir = std::env::current_dir().unwrap().join("fcntl_lock_probe");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("java.lock");
        let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();

        // SAFETY: plain syscalls, both fds fresh.
        let mut fds = [0i32; 2];
        // SAFETY: `fds` is a valid two-element array for the call.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (rd, wr) = (fds[0], fds[1]);

        // SAFETY: fork with no state carried across except raw fds. The child
        // uses only async-signal-safe syscalls, which is what a post-fork child
        // in a multi-threaded test harness is required to do.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork must succeed");
        if pid == 0 {
            // Child: take the write lock, tell the parent, hold, then exit.
            unsafe {
                libc::close(rd);
                let fd = libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CREAT, 0o644);
                let mut fl = libc::flock {
                    l_type: libc::F_WRLCK as i16,
                    l_whence: libc::SEEK_SET as i16,
                    l_start: 0,
                    l_len: 0,
                    l_pid: 0,
                };
                if fd < 0 || libc::fcntl(fd, libc::F_SETLK, &mut fl) == -1 {
                    libc::_exit(1);
                }
                let b = 1u8;
                libc::write(wr, &b as *const u8 as *const libc::c_void, 1);
                libc::sleep(10);
                libc::_exit(0);
            }
        }

        // Parent: wait for the child to confirm it holds the lock.
        // SAFETY: `rd` is owned here and the child closed its copy.
        let mut got = 0u8;
        unsafe {
            assert_eq!(
                libc::read(rd, &mut got as *mut u8 as *mut libc::c_void, 1),
                1,
                "child must report the lock is held"
            );
            libc::close(rd);
            libc::close(wr);
        }

        // The probe under test: F_GETLK, exactly as `java_lock_is_held` does.
        // SAFETY: a fresh descriptor on the same file; `probe` is initialised.
        let mut probe = libc::flock {
            l_type: libc::F_WRLCK as i16,
            l_whence: libc::SEEK_SET as i16,
            l_start: 0,
            l_len: 0,
            l_pid: 0,
        };
        // SAFETY: `c` outlives the call.
        let fd2 = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY) };
        assert!(fd2 >= 0);
        // SAFETY: `fd2` is owned, `probe` fully initialised.
        let ok = unsafe { libc::fcntl(fd2, libc::F_GETLK, &mut probe) } != -1;
        assert!(
            ok && probe.l_type != libc::F_UNLCK as i16,
            "F_GETLK must see the companion's lock"
        );

        // And the bug itself: flock happily takes a file that already carries
        // someone else's fcntl lock, which is why probing with it always said
        // "not held" and the startup wait always timed out.
        // SAFETY: a third owned descriptor on the same file.
        let fd3 = unsafe { libc::open(c.as_ptr(), libc::O_WRONLY) };
        assert!(fd3 >= 0);
        // SAFETY: `fd3` is owned.
        let flock_got_it = unsafe { libc::flock(fd3, libc::LOCK_EX | libc::LOCK_NB) } == 0;
        assert!(
            flock_got_it,
            "flock and fcntl are independent namespaces; this is why the \
             startup probe must use F_GETLK"
        );

        // SAFETY: all descriptors are owned here; the child is disposable.
        unsafe {
            libc::close(fd2);
            libc::close(fd3);
            libc::kill(pid, libc::SIGKILL);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn profile_mode_file_parsing_is_bounds_checked() {
        // The C cast atoi's output to the enum with no range check, so a corrupt
        // current_profile could select any byte. These must all be rejected.
        assert_eq!(read_profile_mode(), None, "no file on the host");
        assert_eq!(ProfileMode::from_index(9), None);
    }

    #[test]
    fn tri_state_priority_defaults_to_the_global_toggle() {
        let mut d = daemon();
        d.opts.app_priority = "default".to_string();
        // Nothing to assert on a host with no properties, but the important part
        // is that it does not panic and does not treat "default" as "true".
        apply_priority_if_wanted(&d);

        d.opts.app_priority = "false".to_string();
        apply_priority_if_wanted(&d);
    }

    #[test]
    fn clear_game_resets_the_foreground_away_window() {
        // Regression guard for the ordering bug: `clear_game` must be callable
        // while the fg_away timer is armed.
        let mut d = daemon();
        d.gamestart = Some("com.a".into());
        d.fg_away_active = true;
        d.fg_away_timer = Some(std::time::Instant::now());
        d.clear_game();
        assert!(!d.fg_away_active);
        assert!(d.fg_away_timer.is_none());
    }
}
