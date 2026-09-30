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

//! The `poll(2)` state machine. Replaces `System/System.c`.
//!
//! The loop is fully event-driven: `timeout_ms` is `-1` (block forever) whenever
//! nothing is pending, so an idle daemon costs no CPU at all. The only reason it
//! wakes is a file event, the companion's death, or one of the three timed
//! windows.

use azenith_common::logger::{Level, log};

use crate::bypass_charge;
use crate::daemon::context::{Daemon, FOREGROUND_AWAY_MS, ProfileMode, SCREEN_OFF_GRACE_MS};
use crate::daemon::inotify::Watcher;

/// Runs until an exit is requested. Returns the process exit code.
pub fn run(daemon: &mut Daemon, watcher: &mut Watcher) -> i32 {
    loop {
        // The daemon deliberately dies with its Java companion: the companion
        // owns `app_status`, so without it the loop is reading a stale file and
        // would apply profiles against fiction. This is the C `java_daemon_died`
        // exit, and it is why `service.sh` starts the companion first.
        if daemon.java_died() {
            log(
                Level::Info,
                "System",
                "Java companion is gone, shutting down.",
            );
            return 0;
        }

        let timeout = poll_timeout(daemon);

        let wake = match watcher.poll_events(daemon, timeout) {
            Ok(w) => w,
            Err(e) => {
                log(Level::Error, "System", &format!("poll failed: {e}"));
                return 1;
            }
        };

        if wake.should_exit {
            break;
        }

        // Everything below runs at most once per wakeup, so a burst of file
        // events cannot turn into N profile applications.
        daemon.need_profile_checkup = false;

        // 1. Frequency offset rebalance. Cheap, always safe.
        crate::profiles::rebalance_freq_offset(daemon);

        // 2. Dynamic bypass charging.
        bypass_charge::maybe_apply_dynamic_bypass(daemon);

        // 3. Auto mode short-circuit: the Manager is driving, so the daemon
        //    stops deciding on its own. This is the C `continue`.
        if daemon.auto_mode_on() {
            continue;
        }

        // 4. Foreground game detection / teardown.
        crate::profiles::detect_game(daemon);

        // 5. Screen state transitions.
        crate::profiles::on_screen_state_change(daemon);

        // 6. Foreground-drop window.
        on_foreground_away(daemon);

        // 7. Battery saver forces eco/balanced.
        if daemon.state.is_low_power() {
            daemon.cur_mode = ProfileMode::Eco;
        }

        // 8. Main profile work.
        if daemon.need_profile_checkup {
            crate::profiles::apply_current(daemon);
        }
    }

    crate::shutdown::run(daemon);
    0
}

/// Chooses the poll timeout for this iteration.
///
/// Mirrors the `if/else if` chain in `System.c:85-103`, including the detail
/// that an *expired* window yields `0` and not `-1`: the handler that clears the
/// window runs later in the same iteration, so spinning for one more wakeup is
/// what lets it happen promptly. Returning `-1` there would stall until the next
/// unrelated file event.
fn poll_timeout(daemon: &Daemon) -> i32 {
    // A PID respawn is being retried: spin, so the retry is not delayed by a
    // whole poll interval.
    if daemon.pid_retries > 0 {
        return 0;
    }
    if daemon.grace_period_active {
        let elapsed = daemon
            .screen_off_timer
            .map(|t| t.elapsed().as_millis() as i32)
            .unwrap_or(0);
        return if elapsed < SCREEN_OFF_GRACE_MS {
            SCREEN_OFF_GRACE_MS - elapsed
        } else {
            0
        };
    }
    if daemon.fg_away_active {
        // With no game running there is nothing to hold Performance for, so the
        // window is dropped outright — the C does this inline at System.c:100.
        if daemon.gamestart.is_none() {
            return 0;
        }
        let elapsed = daemon
            .fg_away_timer
            .map(|t| t.elapsed().as_millis() as i32)
            .unwrap_or(0);
        return if elapsed < FOREGROUND_AWAY_MS {
            FOREGROUND_AWAY_MS - elapsed
        } else {
            0
        };
    }
    // Idle: block until a file event. This is what keeps a running daemon at
    // ~0% CPU.
    -1
}

/// The foreground-drop window: once the game leaves the foreground, wait before
/// tearing the profile down so a brief app switch does not reset everything.
fn on_foreground_away(daemon: &mut Daemon) {
    if !daemon.fg_away_active {
        return;
    }
    let expired = daemon
        .fg_away_timer
        .map(|t| t.elapsed().as_millis() as i32 >= FOREGROUND_AWAY_MS)
        .unwrap_or(true);
    if !expired {
        return;
    }
    log(Level::Info, "System", "Foreground drop window expired.");
    daemon.clear_game();
    daemon.fg_away_active = false;
    daemon.fg_away_timer = None;
    daemon.need_profile_checkup = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_daemon_blocks_indefinitely() {
        let d = Daemon::default();
        assert_eq!(poll_timeout(&d), -1, "an idle daemon must not burn CPU");
    }

    #[test]
    fn respawn_retry_spins_rather_than_sleeping() {
        let mut d = Daemon::default();
        d.pid_retries = 1;
        assert_eq!(poll_timeout(&d), 0);
    }

    #[test]
    fn grace_window_counts_down_then_blocks() {
        let mut d = Daemon::default();
        d.grace_period_active = true;
        d.screen_off_timer = Some(std::time::Instant::now());
        let t = poll_timeout(&d);
        assert!(t > 0 && t <= SCREEN_OFF_GRACE_MS, "got {t}");

        // Simulate the window having elapsed: the C returns 0 here, not -1, so
        // the handler that clears the window runs on the very next wakeup.
        d.screen_off_timer = Some(
            std::time::Instant::now()
                - std::time::Duration::from_millis(SCREEN_OFF_GRACE_MS as u64 + 1),
        );
        assert_eq!(poll_timeout(&d), 0);
    }

    #[test]
    fn the_away_window_is_dropped_when_no_game_is_running() {
        let mut d = Daemon::default();
        d.fg_away_active = true;
        d.gamestart = None;
        assert_eq!(poll_timeout(&d), 0);
    }

    #[test]
    fn the_away_window_counts_down_only_while_a_game_is_tracked() {
        // With no game, the C drops the window outright rather than counting it
        // down — so `poll_timeout` must be 0, not a countdown.
        let mut d = Daemon::default();
        d.gamestart = Some("com.game".into());
        d.fg_away_active = true;
        d.fg_away_timer = Some(std::time::Instant::now());
        let t = poll_timeout(&d);
        assert!(t > 0 && t <= FOREGROUND_AWAY_MS, "got {t}");
    }

    #[test]
    fn an_expired_away_window_tears_the_game_down() {
        let mut d = Daemon::default();
        d.fg_away_active = true;
        d.fg_away_timer = Some(
            std::time::Instant::now()
                - std::time::Duration::from_millis(FOREGROUND_AWAY_MS as u64 + 1),
        );
        on_foreground_away(&mut d);
        assert!(!d.fg_away_active, "the window must not stay armed");
        assert!(
            d.need_profile_checkup,
            "teardown must trigger a profile apply"
        );
    }
}
