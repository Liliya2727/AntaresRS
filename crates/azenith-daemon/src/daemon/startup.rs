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

//! Daemon startup: integrity gate, config load, lock, then the event loop.
//! Replaces `StartupInit/DaemonStartup.c` and `DaemonContext.c`.

use azenith_common::android_props;
use azenith_common::logger::{Level, log};
use azenith_common::paths;

use crate::app_loader;
use crate::config::{prop_validator, state_handler};
use crate::daemon::context::Daemon;
use crate::daemon::event_loop;
use crate::daemon::inotify::Watcher;
use crate::integrity;

/// How long the daemon waits for the Java companion to take `java.lock`.
///
/// The companion must hold the lock before the daemon does anything, because the
/// daemon dies when the companion dies and reads `app_status` constantly. The C
/// used 120 s; the same budget is kept so a slow cold boot behaves identically.
const JAVA_LOCK_WAIT_SECS: u64 = 120;

/// Runs the daemon. Returns the process exit code.
pub fn run() -> i32 {
    // 1. Refuse to run a tampered or mismatched module before touching anything.
    if !integrity::is_intact() {
        return 1;
    }

    // 2. Drop properties the Manager no longer uses.
    prop_validator::validate();

    // 3. Read config from disk and the property store.
    let mut daemon = Daemon::default();
    load_config(&mut daemon);

    // 4. Recover state from an abrupt previous exit, if any.
    state_handler::restore(&mut daemon);

    // 5. Take the lock. This *is* the "is another daemon running" test, and the
    //    lock is held for the process lifetime — the fd is deliberately kept.
    if !crate::daemon::inotify::acquire_daemon_lock() {
        log(
            Level::Error,
            "System",
            "Another daemon already holds the lock.",
        );
        return 1;
    }

    android_props::setprop("persist.sys.azenith.state", "running");
    android_props::setprop("persist.sys.azenith.service", "1");

    // 6. Watch the config paths before the loop can miss an event.
    let mut watcher = match Watcher::new() {
        Ok(w) => w,
        Err(e) => {
            log(Level::Fatal, "System", &format!("inotify init failed: {e}"));
            return 1;
        }
    };

    // 7. Wait for the companion, then hand the lock-watch pipe to the watcher.
    match watcher.wait_for_java_lock(JAVA_LOCK_WAIT_SECS) {
        Ok(()) => {}
        Err(e) => {
            log(
                Level::Fatal,
                "System",
                &format!("Java companion never took {JAVA_LOCK_WAIT_SECS}s lock: {e}"),
            );
            return 1;
        }
    }

    // 8. Initial state read, so the first loop iteration is not blind.
    if let Some(st) = app_loader::status_monitor::read_app_status() {
        daemon.state = st;
    }
    app_loader::gamelist::reload_cache(&daemon.gamelist);
    daemon.need_profile_checkup = true;
    daemon.is_initialize_complete = true;

    log(Level::Info, "System", "Daemon is Ready!");

    let code = event_loop::run(&mut daemon, &mut watcher);

    android_props::setprop("persist.sys.azenith.state", "stopped");
    android_props::setprop("persist.sys.azenith.service", "");
    code
}

/// Reads the flat config files the C's `ConfigLoader.c` parsed.
fn load_config(daemon: &mut Daemon) {
    daemon.config_freqoffset = read_trimmed(&format!("{}/freqoffset", paths::CONFIG_DIR))
        .unwrap_or_else(|| "Disabled".to_string());
    daemon.config_bypasspath =
        read_trimmed(&format!("{}/bypasspath", paths::BYPASSCHG_CONFIG)).unwrap_or_default();
    daemon.config_bypasschg = read_trimmed(&format!("{}/bypasschg", paths::BYPASSCHG_CONFIG))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    daemon.config_bypasschgthreshold =
        read_trimmed(&format!("{}/bypasschgthreshold", paths::BYPASSCHG_CONFIG))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

    log(
        Level::Info,
        "ConfigLoader",
        &format!(
            "freqoffset={} bypass={} path={}",
            daemon.config_freqoffset, daemon.config_bypasschg, daemon.config_bypasspath
        ),
    );
}

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_config_tolerates_a_missing_config_dir() {
        // Nothing exists on the host; must not panic and must leave the
        // documented defaults in place.
        let mut d = Daemon::default();
        load_config(&mut d);
        assert_eq!(d.config_freqoffset, "Disabled");
        assert_eq!(d.config_bypasschg, 0);
        assert!(d.config_bypasspath.is_empty());
    }
}
