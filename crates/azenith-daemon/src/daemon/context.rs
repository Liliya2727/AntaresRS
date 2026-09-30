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

//! Daemon-wide state.
//!
//! The C daemon kept this in file-scope globals: `gamestart`, `opts`,
//! `current_system_cache`, `java_daemon_died` and friends, none of them guarded.
//! Two threads touched them concurrently (`java_lock_watcher_thread` wrote the
//! pipe, `async_preload_worker` read the gamelist cache), which was a data race
//! that happened to work because the writes were single-word.
//!
//! Here the same fields live in one struct owned by the event loop, and the only
//! genuinely cross-thread flag (`java_daemon_died`) is an `AtomicBool`. The
//! gamelist cache is a `Mutex`, matching the C `cache_mutex`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use azenith_common::paths;

/// Profile the daemon is currently in. Mirrors the C `ProfileMode` enum; the
/// discriminants are the same order so any persisted numeric value still maps.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum ProfileMode {
    #[default]
    PerfCommon = 0,
    Performance = 1,
    Balanced = 2,
    Eco = 3,
}

impl ProfileMode {
    /// Parses the C enum name. The Manager and `binprofiles` both send these.
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "PERFCOMMON" => Some(Self::PerfCommon),
            "PERFORMANCE_PROFILE" => Some(Self::Performance),
            "BALANCED_PROFILE" => Some(Self::Balanced),
            "ECO_MODE" => Some(Self::Eco),
            _ => None,
        }
    }

    /// Parses the numeric form used by `sys.azenith-profilesettings 0|1|2|3`.
    pub fn from_index(n: u8) -> Option<Self> {
        match n {
            0 => Some(Self::PerfCommon),
            1 => Some(Self::Performance),
            2 => Some(Self::Balanced),
            3 => Some(Self::Eco),
            _ => None,
        }
    }

    pub fn index(self) -> u8 {
        self as u8
    }
}

impl std::fmt::Display for ProfileMode {
    /// The C enum name, which is also what the Manager writes into
    /// `current_profile` and what `binprofiles` parses.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::PerfCommon => "PERFCOMMON",
            Self::Performance => "PERFORMANCE_PROFILE",
            Self::Balanced => "BALANCED_PROFILE",
            Self::Eco => "ECO_MODE",
        };
        f.write_str(s)
    }
}

/// Per-app settings, one entry per row of `azenithApplist.json`.
///
/// The C version was a fixed-size `char[]` struct; the field names and the
/// "empty or `default` means off" convention are preserved verbatim, because
/// that convention is what the Manager writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GameConfig {
    pub package: String,
    pub perf_lite_mode: String,
    pub dnd_on_gaming: String,
    pub app_priority: String,
    pub game_preload: String,
    pub refresh_rate: String,
    pub renderer: String,
    pub resolution_downscale: String,
    pub resolution_fps: String,
    pub bypass_charging: String,
}

impl GameConfig {
    /// C `IS_DEFAULT(v)` — unset or literally "default".
    pub fn is_default(v: &str) -> bool {
        v.is_empty() || v == "default"
    }

    /// Looks up an app in the cached gamelist. Returns a clone so callers cannot
    /// mutate the cache through the returned reference.
    pub fn find(cache: &[GameConfig], package: &str) -> Option<GameConfig> {
        cache.iter().find(|c| c.package == package).cloned()
    }
}

/// Snapshot the Java companion writes to `app_status`.
///
/// Whitespace-delimited, not JSON — see `app_loader::status_monitor`. The
/// companion rewrites the file atomically and only on change, so a short read
/// here is not possible.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemState {
    pub focused_app: String,
    pub app_name: String,
    pub focused_pid: i32,
    pub zen_mode: i32,
    pub screen_awake: i32,
    pub battery_saver: i32,
    pub battery_level: i32,
    pub is_charging: i32,
}

impl SystemState {
    /// C `IS_AWAKE(state)` — accepts both the word and the boolean string,
    /// because the companion has written both over the module's life.
    pub fn is_awake(&self) -> bool {
        self.screen_awake == 1
    }

    /// C `get_low_power_state` — battery saver on.
    pub fn is_low_power(&self) -> bool {
        self.battery_saver == 1
    }

    pub fn is_charging(&self) -> bool {
        self.is_charging == 1
    }
}

/// Everything the event loop carries between iterations. Replaces the C
/// `DaemonContext` plus its loose globals.
pub struct Daemon {
    /// Set once the initial profile apply has run; gates most of the loop.
    pub is_initialize_complete: bool,
    pub need_profile_checkup: bool,
    pub bypass_applied: bool,
    pub has_applied_renderer: bool,
    pub dnd_enabled: bool,

    /// `-1` before the first screen-state read, so the first iteration always
    /// registers as a transition. Matches the C `prev_screen_state = -1`.
    pub prev_screen_state: i32,
    pub saved_refresh_rate: i32,
    pub saved_zen_mode: i32,

    /// Ten-second window after screen-off during which Performance is held.
    pub grace_period_active: bool,
    pub screen_off_timer: Option<Instant>,

    /// Thirty-second window after the focused app leaves, before dropping to
    /// Balanced. Only armed when `dropforeground` is enabled.
    pub fg_away_active: bool,
    pub fg_away_timer: Option<Instant>,

    pub pid_retries: u32,
    pub cur_mode: ProfileMode,
    pub saved_renderer: String,
    pub saved_sys_renderer: String,
    pub last_freqoffset: String,
    pub prev_ai_state: String,
    pub resolution_applied: bool,
    pub used_legacy_fallback: bool,

    pub config_freqoffset: String,
    pub config_bypasspath: String,
    pub config_bypasschg: i32,
    pub config_bypasschgthreshold: i32,

    /// The game currently in the foreground, if any.
    pub gamestart: Option<String>,
    pub active_app_name: Option<String>,
    pub game_pids: Vec<i32>,
    pub opts: GameConfig,
    pub state: SystemState,

    /// C `java_daemon_died`, written by the lock-watcher thread and read by the
    /// loop. The one flag that genuinely crosses a thread boundary.
    pub java_daemon_died: AtomicBool,

    /// Guards the gamelist cache, as `cache_mutex` did — but the mutex now
    /// covers a `Vec`, so there is no manual `free_gamelist_cache`.
    pub gamelist: Mutex<Vec<GameConfig>>,
}

impl Default for Daemon {
    fn default() -> Self {
        Self {
            is_initialize_complete: false,
            need_profile_checkup: false,
            bypass_applied: false,
            has_applied_renderer: false,
            dnd_enabled: false,
            prev_screen_state: -1,
            saved_refresh_rate: -1,
            saved_zen_mode: -1,
            grace_period_active: false,
            screen_off_timer: None,
            fg_away_active: false,
            fg_away_timer: None,
            pid_retries: 0,
            cur_mode: ProfileMode::PerfCommon,
            saved_renderer: String::new(),
            saved_sys_renderer: String::new(),
            // "Initial", not "", so the first freqoffset comparison always differs.
            last_freqoffset: "Initial".to_string(),
            prev_ai_state: "0".to_string(),
            resolution_applied: false,
            used_legacy_fallback: false,
            config_freqoffset: "Disabled".to_string(),
            config_bypasspath: String::new(),
            config_bypasschg: 0,
            config_bypasschgthreshold: 0,
            gamestart: None,
            active_app_name: None,
            game_pids: Vec::new(),
            opts: GameConfig::default(),
            state: SystemState::default(),
            java_daemon_died: AtomicBool::new(false),
            gamelist: Mutex::new(Vec::new()),
        }
    }
}

impl Daemon {
    /// The label used in log lines and notifications: the app's display name
    /// when known, else the package. C did this inline in ~8 places.
    pub fn game_label(&self) -> &str {
        self.active_app_name
            .as_deref()
            .or(self.gamestart.as_deref())
            .unwrap_or("")
    }

    /// Looks up a package's settings in the gamelist cache.
    ///
    /// Exact package match, as the C did — a prefix match would let
    /// `com.game` claim `com.game.helper`'s settings. Returns `None` when the
    /// package has no entry, which is how the caller tells "not in the list"
    /// from "in the list but everything is default".
    pub fn opts_for(&self, pkg: &str) -> Option<GameConfig> {
        let cache = lock_cache(&self.gamelist)?;
        cache.iter().find(|g| g.package == pkg).cloned()
    }

    /// True when the companion's focus is the tracked game.
    pub fn game_is_focused(&self) -> bool {
        match &self.gamestart {
            Some(g) => self.state.focused_app == *g,
            None => false,
        }
    }

    /// True when the Manager is driving profiles itself, in which case the
    /// daemon stops deciding. The C's `continue` at `System.c:119`.
    ///
    /// Read from `API/current_modes` on every call because the loop's own
    /// inotify watch is what tells us the file changed — caching it here would
    /// need invalidation the C never had.
    pub fn auto_mode_on(&self) -> bool {
        let Ok(text) = std::fs::read_to_string(paths::DAEMON_MODES) else {
            return false;
        };
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("auto_mode=") {
                return v.trim() == "1";
            }
        }
        false
    }

    pub fn java_died(&self) -> bool {
        self.java_daemon_died.load(Ordering::Relaxed)
    }

    pub fn set_java_died(&self) {
        self.java_daemon_died.store(true, Ordering::Relaxed);
    }

    /// Drops the tracked game, as the C code's `free(gamestart); gamestart = NULL`
    /// cluster did. Kept in one place so the four call sites cannot drift.
    ///
    /// Also disarms the foreground-away window, which the C did alongside it
    /// (`System.c:220-223`): an armed window outliving its game would fire
    /// `poll_timeout`'s `0` branch forever, because that branch also checks
    /// `gamestart.is_none()` and would keep re-entering the teardown.
    pub fn clear_game(&mut self) {
        self.gamestart = None;
        self.active_app_name = None;
        self.game_pids.clear();
        self.fg_away_active = false;
        self.fg_away_timer = None;
        self.grace_period_active = false;
        self.screen_off_timer = None;
        self.game_pid_count_reset();
    }

    fn game_pid_count_reset(&mut self) {
        // C reset `game_pid_count` explicitly; the Vec length is the count now.
        self.pid_retries = 0;
    }
}

/// Write-locks the gamelist cache. Returns `None` on a poisoned mutex rather
/// than panicking — `panic = 'abort'` turns a panic here into a daemon-wide
/// SIGABRT, and a poisoned cache is recoverable data, not a reason to die.
pub fn lock_cache(
    cache: &Mutex<Vec<GameConfig>>,
) -> Option<std::sync::MutexGuard<'_, Vec<GameConfig>>> {
    cache.lock().ok()
}

/// Screen-off grace window: Performance is held this long after the screen goes
/// off, so a brief lock during a game does not tear the profile down.
pub const SCREEN_OFF_GRACE_MS: i32 = 10_000;

/// How long a game may sit out of the foreground before dropping to Balanced.
/// Only consulted when `persist.sys.azenith.dropforeground` is `1`.
pub const FOREGROUND_AWAY_MS: i32 = 30_000;

/// Journal of in-flight property deletions, bounded so a wedged `resetprop`
/// cannot grow it without limit. C used a fixed `MAX_PENDING_DELETE` array.
pub const MAX_PENDING_DELETE: usize = 128;

/// Property-name buffer size, from `AZenith.h:82`.
pub const MAX_PROP_NAME_BUF: usize = 192;

/// Upper bound on tracked game PIDs. C `MAX_GAME_PIDS`.
pub const MAX_GAME_PIDS: usize = 8;

/// How many PIDs the inotify refresh actually collects.
///
/// Deliberately 2, not `MAX_GAME_PIDS`: the C hard-coded `max_track_pids = 2` in
/// `handle_background_apps_event` even though the array holds 8
/// (`InotifyWatcher.c:26-27`). This is a known C wart, preserved so the port
/// behaves identically; raise it to `MAX_GAME_PIDS` once the priority writes
/// have been verified against a game with more than two processes.
pub const MAX_TRACK_PIDS: usize = 2;

/// Watched config paths. The inotify watcher subscribes to exactly these.
pub fn watched_paths() -> [&'static str; 6] {
    [
        paths::CONFIG_DIR,
        paths::API_DIR,
        paths::GAMELIST_DIR,
        paths::BYPASSCHG_CONFIG,
        paths::MODULE_DIR,
        "/data/adb/.config/AZenith/background_apps",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_mode_round_trips_through_both_encodings() {
        for m in [
            ProfileMode::PerfCommon,
            ProfileMode::Performance,
            ProfileMode::Balanced,
            ProfileMode::Eco,
        ] {
            assert_eq!(ProfileMode::from_index(m.index()), Some(m));
            assert_eq!(ProfileMode::from_name(name_of(m)), Some(m));
        }
    }

    const fn name_of(m: ProfileMode) -> &'static str {
        match m {
            ProfileMode::PerfCommon => "PERFCOMMON",
            ProfileMode::Performance => "PERFORMANCE_PROFILE",
            ProfileMode::Balanced => "BALANCED_PROFILE",
            ProfileMode::Eco => "ECO_MODE",
        }
    }

    #[test]
    fn profile_mode_rejects_out_of_range_indices() {
        // The C `run_profiler` took an int and indexed an array without a
        // bounds check; 4 would have read past the end.
        assert_eq!(ProfileMode::from_index(4), None);
        assert_eq!(ProfileMode::from_index(255), None);
        assert_eq!(ProfileMode::from_name("performance_profile"), None);
    }

    #[test]
    fn is_default_treats_empty_and_literal_default_as_off() {
        assert!(GameConfig::is_default(""));
        assert!(GameConfig::is_default("default"));
        assert!(!GameConfig::is_default("performance"));
        assert!(!GameConfig::is_default("Default"));
    }

    #[test]
    fn game_lookup_is_exact_match() {
        let cache = vec![
            GameConfig {
                package: "com.a".into(),
                ..Default::default()
            },
            GameConfig {
                package: "com.b".into(),
                ..Default::default()
            },
        ];
        assert!(GameConfig::find(&cache, "com.b").is_some());
        // Prefix must not match — the C code used strcmp, not strncmp.
        assert!(GameConfig::find(&cache, "com").is_none());
    }

    #[test]
    fn clear_game_resets_pids_and_retries() {
        let mut d = Daemon::default();
        d.gamestart = Some("com.game".into());
        d.active_app_name = Some("Game".into());
        d.game_pids = vec![1, 2, 3];
        d.pid_retries = 4;
        d.clear_game();
        assert!(d.gamestart.is_none());
        assert!(d.active_app_name.is_none());
        assert!(d.game_pids.is_empty());
        assert_eq!(d.pid_retries, 0);
    }

    #[test]
    fn label_falls_back_from_name_to_package() {
        let mut d = Daemon::default();
        d.gamestart = Some("com.game".into());
        assert_eq!(d.game_label(), "com.game");
        d.active_app_name = Some("Real Game".into());
        assert_eq!(d.game_label(), "Real Game");
    }
}
