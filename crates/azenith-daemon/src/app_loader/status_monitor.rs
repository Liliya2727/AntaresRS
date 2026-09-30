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

//! Parses the Java companion's `app_status` file.
//!
//! The format is whitespace-delimited `<key> <value...>`, one per line — not
//! JSON, and not validated on either side:
//!
//! ```text
//! focused_app <pkg> <pid> <uid>
//! screen_awake <0|1>
//! battery_saver <0|1>
//! zen_mode <int>
//! battery_level <int>
//! is_charging <0|1>
//! app_name <label>
//! refresh_rate <int>
//! max_refresh_rate <int>
//! ```
//!
//! The companion rewrites the file atomically (tmp + rename) and only on change,
//! so a reader never sees a partial write. That is the only reason a plain
//! `read_to_string` is safe here — it would otherwise be a torn-read race.
//!
//! If the writer and this reader ever disagree, nothing catches it: there is no
//! schema, no version field, and no test on the Kotlin side. Both sides have to
//! change together.

use azenith_common::logger::{Level, log};
use azenith_common::paths;

use super::super::daemon::context::SystemState;

/// Reads and parses `app_status`.
///
/// Returns `None` when the file is missing — the C version left the cache
/// untouched in that case, which matters because the first read happens before
/// the companion has necessarily written anything.
pub fn read_app_status() -> Option<SystemState> {
    let raw = match std::fs::read_to_string(paths::APP_MONITOR_FILE) {
        Ok(s) => s,
        Err(_) => return None,
    };
    Some(parse_app_status(&raw))
}

/// Parses an `app_status` payload. Split out from the file read so it is
/// testable against real captured data.
pub fn parse_app_status(raw: &str) -> SystemState {
    // Defaults mirror the C pre-loop memsets exactly. `battery_level = -1` is
    // the "unknown" sentinel; 0 would read as a critically flat battery.
    let mut st = SystemState {
        app_name: "Unknown".to_string(),
        battery_level: -1,
        ..Default::default()
    };

    for line in raw.lines() {
        // `split_once` on the first space, mirroring the C `strncmp(line, "key ")`
        // prefix tests: a key with no trailing space value is simply ignored.
        let Some((key, rest)) = line.split_once(' ') else {
            continue;
        };
        let rest = rest.trim();
        match key {
            // C used `sscanf("%127s %d %d")`: pkg, pid, uid. The uid was parsed
            // and discarded; only the pid is kept.
            "focused_app" => {
                let mut it = rest.split_whitespace();
                if let Some(pkg) = it.next() {
                    st.focused_app = pkg.to_string();
                }
                if let Some(pid) = it.next().and_then(|p| p.parse::<i32>().ok()) {
                    st.focused_pid = pid;
                }
            }
            "screen_awake" => st.screen_awake = parse_int(rest, st.screen_awake),
            "battery_saver" => st.battery_saver = parse_int(rest, st.battery_saver),
            "zen_mode" => st.zen_mode = parse_int(rest, st.zen_mode),
            "battery_level" => st.battery_level = parse_int(rest, st.battery_level),
            "is_charging" => st.is_charging = parse_int(rest, st.is_charging),
            // The label may contain spaces, so the whole remainder is the value.
            // `lines()` already dropped the newline the C code had to strip.
            "app_name" => st.app_name = rest.to_string(),
            _ => {}
        }
    }
    st
}

/// Parses an int, keeping `prev` when the token is absent or malformed.
///
/// The C `sscanf` return value was never checked, so a bad line left the field
/// at whatever it was — which for `battery_level` is the `-1` sentinel.
fn parse_int(s: &str, prev: i32) -> i32 {
    s.trim().parse::<i32>().unwrap_or(prev)
}

/// Reads `background_apps` — one `<pkg> <pid> <uid>` line per recent process.
///
/// Returned newest-first as written. The C reader capped tracked game PIDs at 2
/// here while `MAX_GAME_PIDS` was 8; that cap is *not* reproduced, because it
/// was the bug. See [`crate::daemon::context::MAX_GAME_PIDS`].
pub fn read_background_apps() -> Vec<BackgroundApp> {
    let Ok(raw) = std::fs::read_to_string(paths::BACKGROUND_APPS) else {
        return Vec::new();
    };
    parse_background_apps(&raw)
}

/// One tracked process from `background_apps`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundApp {
    pub package: String,
    pub pid: i32,
    pub uid: i32,
}

/// Parses `background_apps`. Malformed lines are skipped rather than aborting
/// the whole read — a single bad line must not lose every other tracked process.
pub fn parse_background_apps(raw: &str) -> Vec<BackgroundApp> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut it = line.split_whitespace();
        let (Some(pkg), Some(pid), Some(uid)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(pid), Ok(uid)) = (pid.parse::<i32>(), uid.parse::<i32>()) else {
            continue;
        };
        if pkg.is_empty() {
            continue;
        }
        out.push(BackgroundApp {
            package: pkg.to_string(),
            pid,
            uid,
        });
    }
    out
}

/// Logs a one-line summary of the state read, for the verbose log.
pub fn log_state(st: &SystemState) {
    log(
        Level::Debug,
        "AppMonitor",
        &format!(
            "focused={} pid={} awake={} zen={} batt={}% chg={} name={}",
            st.focused_app,
            st.focused_pid,
            st.screen_awake,
            st.zen_mode,
            st.battery_level,
            st.is_charging,
            st.app_name
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
focused_app com.miHoYo.Yuanshen 12345 10123
screen_awake 1
battery_saver 0
zen_mode 2
battery_level 87
is_charging 1
app_name Genshin Impact
refresh_rate 120
max_refresh_rate 144
";

    #[test]
    fn parses_a_full_status_file() {
        let st = parse_app_status(SAMPLE);
        assert_eq!(st.focused_app, "com.miHoYo.Yuanshen");
        assert_eq!(st.focused_pid, 12345);
        assert_eq!(st.screen_awake, 1);
        assert_eq!(st.battery_saver, 0);
        assert_eq!(st.zen_mode, 2);
        assert_eq!(st.battery_level, 87);
        assert_eq!(st.is_charging, 1);
        // The label keeps its spaces and loses the trailing newline, which the
        // C code had to strip with strcspn.
        assert_eq!(st.app_name, "Genshin Impact");
    }

    #[test]
    fn unknown_battery_is_minus_one_not_zero() {
        // The sentinel matters: 0 would make the daemon think the device is
        // critically flat on a missing field.
        let st = parse_app_status("focused_app com.a 1 0\n");
        assert_eq!(st.battery_level, -1);
    }

    #[test]
    fn absent_app_name_falls_back_to_unknown() {
        let st = parse_app_status("screen_awake 1\n");
        assert_eq!(st.app_name, "Unknown");
    }

    #[test]
    fn empty_file_yields_all_defaults() {
        let st = parse_app_status("");
        assert_eq!(st.focused_app, "");
        assert_eq!(st.app_name, "Unknown");
        assert_eq!(st.battery_level, -1);
        assert_eq!(st.is_charging, 0);
    }

    #[test]
    fn focused_app_without_pid_keeps_package() {
        // C `sscanf` assigned the package then failed on the pid; the package
        // was already written, so it survived with pid left at 0.
        let st = parse_app_status("focused_app com.orphan\n");
        assert_eq!(st.focused_app, "com.orphan");
        assert_eq!(st.focused_pid, 0);
    }

    #[test]
    fn malformed_int_keeps_the_previous_value() {
        let st = parse_app_status("battery_level 55\nbattery_level notanumber\n");
        assert_eq!(st.battery_level, 55);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        // refresh_rate / max_refresh_rate are written but read elsewhere; they
        // must not disturb the fields this parser owns.
        let st = parse_app_status("refresh_rate 90\nscreen_awake 1\n");
        assert_eq!(st.screen_awake, 1);
    }

    #[test]
    fn key_without_a_space_is_skipped() {
        let st = parse_app_status("focused_app\nscreen_awake 1\n");
        assert_eq!(st.focused_app, "");
        assert_eq!(st.screen_awake, 1);
    }

    #[test]
    fn background_apps_round_trips() {
        let apps = parse_background_apps("com.a 100 1000\ncom.b 200 1001\n");
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].package, "com.a");
        assert_eq!(apps[0].pid, 100);
        assert_eq!(apps[0].uid, 1000);
        assert_eq!(apps[1].package, "com.b");
    }

    #[test]
    fn background_apps_skips_bad_lines_but_keeps_good_ones() {
        // A torn line must not cost us every other tracked process.
        let apps =
            parse_background_apps("garbage\ncom.a 100 1000\ncom.b notanint 1\ncom.c 300 1002\n");
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].package, "com.a");
        assert_eq!(apps[1].package, "com.c");
    }
}
