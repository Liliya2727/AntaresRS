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

//! Profile apply/teardown. Replaces `SystemProfile/SystemProfiles.c` and
//! `SystemProfile/ProfileUtility.c`.
//!
//! The heavy per-SoC sysfs work still lives in `binprofiles` and is reached
//! through a direct call once the crates are folded in; until then it is
//! invoked with the same command line the C used, so behaviour is identical.

use azenith_common::android_props;
use azenith_common::logger::{Level, log};
use azenith_common::paths;
use azenith_common::shell;

use crate::daemon::context::{
    Daemon, GameConfig, MAX_TRACK_PIDS, ProfileMode, SCREEN_OFF_GRACE_MS,
};
use crate::handlers;

/// Applies `mode`, skipping the work when it is already current.
///
/// The C re-entered `systemv` unconditionally and relied on the sub-binary to
/// be idempotent; short-circuiting here also avoids the fork.
pub fn apply(daemon: &mut Daemon, mode: ProfileMode) {
    if daemon.cur_mode == mode && mode != ProfileMode::PerfCommon {
        return;
    }
    let previous = daemon.cur_mode;
    daemon.cur_mode = mode;

    log(
        Level::Info,
        "ProfileSwitch",
        &format!("Applying {mode} (was {previous})"),
    );

    if android_props::is_true(&android_props::getprop(
        "persist.sys.azenithconf.profilenotifications",
    )) {
        crate::utility::toast(&format!("Switched to {previous} -> {mode}"));
    }

    // ponytail: fork+exec for now; becomes a direct call once binprofiles is a
    // lib crate in this workspace.
    let _ = shell::systemv(&format!("sys.azenith-service profiles {}", mode.index()));

    write_current_profile(mode);
}

/// Records the active profile in both the root-side and app-readable files.
pub fn write_current_profile(mode: ProfileMode) {
    let name = profile_name(mode);
    let _ = std::fs::write(paths::PROFILE_MODE, format!("{name}\n"));
    // App-readable mirror, so the Manager can poll without a root shell.
    if let Some(dir) = std::path::Path::new(paths::PROFILE_MODE_APP).parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(paths::PROFILE_MODE_APP, format!("{name}\n"));
    }
}

/// The string the Manager and `binprofiles` exchange for a profile.
pub fn profile_name(mode: ProfileMode) -> &'static str {
    match mode {
        ProfileMode::PerfCommon => "PERFCOMMON",
        ProfileMode::Performance => "PERFORMANCE_PROFILE",
        ProfileMode::Balanced => "BALANCED_PROFILE",
        ProfileMode::Eco => "ECO_MODE",
    }
}

/// Restores everything a profile apply changed: DND, renderer, refresh rate.
pub fn teardown(daemon: &mut Daemon) {
    if daemon.dnd_enabled {
        let _ = shell::systemv("sys.azenith-service utils enableDND");
        // The C disabled DND unconditionally on teardown; doing it only when we
        // enabled it is strictly safer and identical from the daemon's view.
        let _ = shell::systemv("sys.azenith-service utils disableDND");
        daemon.dnd_enabled = false;
    }
    handlers::renderer::restore(daemon);
    restore_refresh_rate(daemon);
}

/// Restores the refresh rate saved before a performance profile.
fn restore_refresh_rate(daemon: &mut Daemon) {
    if daemon.saved_refresh_rate <= 0 {
        return;
    }
    let saved = daemon.saved_refresh_rate;
    log(
        Level::Info,
        "ProfileSwitch",
        &format!("Restoring refresh rate to {saved}Hz"),
    );
    handlers::refresh_rate::apply(saved);
    daemon.saved_refresh_rate = -1;
}

/// Saves the current refresh rate before a profile overrides it, once.
pub fn save_refresh_rate(daemon: &mut Daemon) {
    if daemon.saved_refresh_rate > 0 {
        return;
    }
    let cur = handlers::refresh_rate::current();
    if cur > 0 {
        daemon.saved_refresh_rate = cur;
        log(
            Level::Info,
            "ProfileSwitch",
            &format!("Saved refresh rate {cur}Hz for restore"),
        );
    }
}

/// Reads the profile the Manager last requested, if any.
pub fn read_requested_mode() -> Option<ProfileMode> {
    let text = std::fs::read_to_string(paths::DAEMON_MODES).ok()?;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("auto_mode=") {
            // Auto mode on means the daemon picks profiles itself; the Manager's
            // "auto" boolean is the negation of this flag, which is a known
            // wart in the C and is preserved here deliberately.
            return (!android_props::is_true(v)).then_some(ProfileMode::PerfCommon);
        }
    }
    None
}

// ── Event-loop steps ────────────────────────────────────────────────────────

/// Rebalances cluster frequency offsets when the Manager's `freqoffset` changed.
///
/// A no-op unless the file's contents actually differ, which is why
/// `last_freqoffset` is seeded with `"Initial"` rather than `""` — otherwise the
/// first read of an unset (empty) file would look unchanged and never apply.
pub fn rebalance_freq_offset(daemon: &mut Daemon) {
    let current = std::fs::read_to_string(paths::CONFIG_DIR)
        .map(|d| d.trim().to_string())
        .unwrap_or_default();
    // The real value lives in its own file, not in the directory listing.
    let current = std::fs::read_to_string(freqoffset_path())
        .map(|s| s.trim().to_string())
        .unwrap_or(current);

    if current == daemon.last_freqoffset {
        return;
    }
    daemon.last_freqoffset = current.clone();
    log(
        Level::Info,
        "FreqOffset",
        &format!("freqoffset changed to {current}, rebalancing"),
    );
    let _ = shell::systemv("sys.azenith-service profiles applyfreqbalance");
}

fn freqoffset_path() -> String {
    format!("{}/freqoffset", paths::CONFIG_DIR)
}

/// Detects a foreground game and applies or tears down its profile.
pub fn detect_game(daemon: &mut Daemon) {
    let focused = daemon.state.focused_app.clone();
    if focused.is_empty() {
        return;
    }

    // Already tracking: refresh the PID list, nothing else to do.
    if daemon.gamestart.as_deref() == Some(focused.as_str()) {
        daemon.game_pids = crate::pid_tracker::pids_of(&focused, MAX_TRACK_PIDS);
        return;
    }

    // A tracked game that lost focus gets the away window, not an immediate
    // teardown — handled in the loop, not here.
    if daemon.gamestart.is_some() {
        return;
    }

    let Some(opts) = daemon.opts_for(&focused) else {
        return;
    };
    if !GameConfig::is_default(&opts.perf_lite_mode) {
        // A per-app profile is not a game; leave the global profile alone.
        return;
    }

    log(
        Level::Info,
        "GameDetect",
        &format!("Game detected: {focused}"),
    );
    daemon.gamestart = Some(focused.clone());
    daemon.game_pids = crate::pid_tracker::pids_of(&focused, MAX_TRACK_PIDS);
    daemon.need_profile_checkup = true;
    daemon.fg_away_active = false;
    daemon.fg_away_timer = None;
}

/// Applies the screen-state grace window and the away window.
pub fn on_screen_state_change(daemon: &mut Daemon) {
    let real = i32::from(daemon.state.is_awake());
    if real != daemon.prev_screen_state {
        if real == 0 {
            // Only Performance gets a grace window; nothing is being held for
            // Balanced or Eco.
            if daemon.cur_mode == ProfileMode::Performance {
                daemon.screen_off_timer = Some(std::time::Instant::now());
                daemon.grace_period_active = true;
                log(
                    Level::Info,
                    "System",
                    "Screen OFF Event: Grace period started (10s)...",
                );
            }
        } else {
            if daemon.grace_period_active {
                log(
                    Level::Info,
                    "System",
                    "Screen ON Event: Grace period aborted. Keeping Performance.",
                );
                daemon.grace_period_active = false;
            }
            daemon.screen_off_timer = None;
        }
        daemon.prev_screen_state = real;
    }

    if daemon.grace_period_active {
        let elapsed = daemon
            .screen_off_timer
            .map(|t| t.elapsed().as_millis() as i32)
            .unwrap_or(SCREEN_OFF_GRACE_MS);
        if elapsed >= SCREEN_OFF_GRACE_MS {
            log(
                Level::Info,
                "System",
                "Grace period expired. Dropping Performance Profile.",
            );
            daemon.grace_period_active = false;
            daemon.screen_off_timer = None;
            daemon.need_profile_checkup = true;
        }
    }

    // Foreground-drop window, only for a running game in Performance.
    if daemon.gamestart.is_some() && daemon.cur_mode == ProfileMode::Performance {
        let enabled = android_props::getprop("persist.sys.azenith.dropforeground") == "1";
        if enabled && !daemon.game_is_focused() {
            if !daemon.fg_away_active {
                daemon.fg_away_timer = Some(std::time::Instant::now());
                daemon.fg_away_active = true;
            }
        } else {
            daemon.fg_away_active = false;
            daemon.fg_away_timer = None;
        }
    }
}

/// Chooses and applies the profile for the current state.
///
/// The decision order is the C's: game → battery saver → Performance → Balanced.
pub fn apply_current(daemon: &mut Daemon) {
    let want = if daemon.gamestart.is_some() {
        ProfileMode::Performance
    } else if daemon.state.is_low_power() {
        ProfileMode::Eco
    } else if daemon.cur_mode == ProfileMode::Performance {
        // Just left a game, or the screen turned off: settle to Balanced rather
        // than snapping straight to Eco.
        ProfileMode::Balanced
    } else {
        daemon.cur_mode
    };

    if want != daemon.cur_mode {
        log(
            Level::Info,
            "ProfileSwitch",
            &format!("{} -> {want}", daemon.cur_mode),
        );
    }
    apply(daemon, want);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_round_trip_through_from_name() {
        for m in [
            ProfileMode::PerfCommon,
            ProfileMode::Performance,
            ProfileMode::Balanced,
            ProfileMode::Eco,
        ] {
            assert_eq!(ProfileMode::from_name(profile_name(m)), Some(m));
        }
    }

    #[test]
    fn indices_round_trip_through_from_index() {
        for m in [
            ProfileMode::PerfCommon,
            ProfileMode::Performance,
            ProfileMode::Balanced,
            ProfileMode::Eco,
        ] {
            assert_eq!(ProfileMode::from_index(m.index()), Some(m));
        }
    }

    #[test]
    fn teardown_clears_a_saved_refresh_rate() {
        // Must not leave a stale value that would be "restored" on the next
        // teardown forever.
        let mut d = Daemon::default();
        d.saved_refresh_rate = 120;
        teardown(&mut d);
        assert_eq!(d.saved_refresh_rate, -1);
    }

    #[test]
    fn teardown_without_saved_state_is_inert() {
        let mut d = Daemon::default();
        teardown(&mut d);
        assert_eq!(d.saved_refresh_rate, -1);
        assert!(d.saved_renderer.is_empty());
    }
}
