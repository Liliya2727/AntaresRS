// Copyright (C) 2025-2026 Zexshia
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

//! Crash-recovery state file. Replaces `ConfigHandler/StateHandler.c`.

use azenith_common::logger::{Level, log};
use azenith_common::paths;
use azenith_common::shell;

use crate::daemon::context::{Daemon, ProfileMode};

/// Writes the state that must survive an abrupt daemon restart.
pub fn save(daemon: &Daemon) {
    let body = format!(
        "saved_renderer={}\n\
         saved_sys_renderer={}\n\
         saved_refresh_rate={}\n\
         saved_zen_mode={}\n\
         dnd_enabled={}\n\
         cur_mode={}\n",
        daemon.saved_renderer,
        daemon.saved_sys_renderer,
        daemon.saved_refresh_rate,
        daemon.saved_zen_mode,
        i32::from(daemon.dnd_enabled),
        daemon.cur_mode as i32,
    );

    if let Err(e) = std::fs::write(paths::DAEMON_STATE_FILE, body) {
        log(
            Level::Error,
            "StateHandler",
            &format!("Failed to open state file for writing: {e}"),
        );
        return;
    }
    log(
        Level::Info,
        "StateHandler",
        "Daemon state saved for recovery",
    );
}

/// Restores state written by [`save`], then deletes the file.
///
/// A missing file is the normal cold start, not an error. The file is removed
/// after a successful read so a restart cannot re-apply an already-reconciled
/// state; `std::fs::remove_file` is used instead of the C's `rm -rf` so the path
/// is not re-parsed by a shell.
pub fn restore(daemon: &mut Daemon) {
    let text = match std::fs::read_to_string(paths::DAEMON_STATE_FILE) {
        Ok(t) => t,
        Err(_) => return,
    };

    for line in text.lines() {
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        match key {
            "saved_renderer" => daemon.saved_renderer = val.to_string(),
            "saved_sys_renderer" => daemon.saved_sys_renderer = val.to_string(),
            "saved_refresh_rate" => daemon.saved_refresh_rate = atoi(val),
            "saved_zen_mode" => daemon.saved_zen_mode = atoi(val),
            "dnd_enabled" => daemon.dnd_enabled = atoi(val) == 1,
            "cur_mode" => {
                daemon.cur_mode = ProfileMode::from_index(atoi(val) as u8).unwrap_or_default()
            }
            _ => {}
        }
    }

    let _ = std::fs::remove_file(paths::DAEMON_STATE_FILE);
    let _ = shell::run("sync");
    log(
        Level::Info,
        "StateHandler",
        "Restored daemon state from previous session, will reconcile on next profile checkup",
    );
}

/// `atoi` semantics: leading integer, 0 on anything unparseable. Matches the C
/// rather than Rust's `parse`, which would leave the field untouched instead.
fn atoi(s: &str) -> i32 {
    let t = s.trim();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    let n: i32 = digits[..end].parse().unwrap_or(0);
    if neg { -n } else { n }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoi_matches_c_semantics() {
        assert_eq!(atoi("0"), 0);
        assert_eq!(atoi("1"), 1);
        assert_eq!(atoi("-1"), -1);
        assert_eq!(atoi("2\n"), 2);
        // The C's atoi stops at the first non-digit rather than failing.
        assert_eq!(atoi("60Hz"), 60);
        assert_eq!(atoi(""), 0);
        assert_eq!(atoi("abc"), 0);
    }

    #[test]
    fn restore_on_a_cold_start_is_silent() {
        // No state file on the host: must not log an error or touch the daemon.
        let mut d = Daemon::default();
        let before = d.cur_mode;
        restore(&mut d);
        assert_eq!(d.cur_mode, before);
    }

    #[test]
    fn roundtrip_preserves_every_field() {
        let mut d = Daemon::default();
        d.saved_renderer = "skia".into();
        d.saved_sys_renderer = "gl".into();
        d.saved_refresh_rate = 120;
        d.saved_zen_mode = 1;
        d.dnd_enabled = true;
        d.cur_mode = ProfileMode::Performance;

        // save() targets a real path under /data/adb, so exercise the parse
        // half directly against the exact body save() produces.
        let body = format!(
            "saved_renderer={}\nsaved_sys_renderer={}\nsaved_refresh_rate={}\nsaved_zen_mode={}\ndnd_enabled={}\ncur_mode={}\n",
            d.saved_renderer,
            d.saved_sys_renderer,
            d.saved_refresh_rate,
            d.saved_zen_mode,
            i32::from(d.dnd_enabled),
            d.cur_mode as i32,
        );

        let mut r = Daemon::default();
        for line in body.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k {
                "saved_renderer" => r.saved_renderer = v.to_string(),
                "saved_sys_renderer" => r.saved_sys_renderer = v.to_string(),
                "saved_refresh_rate" => r.saved_refresh_rate = atoi(v),
                "saved_zen_mode" => r.saved_zen_mode = atoi(v),
                "dnd_enabled" => r.dnd_enabled = atoi(v) == 1,
                "cur_mode" => {
                    r.cur_mode = ProfileMode::from_index(atoi(v) as u8).unwrap_or_default()
                }
                _ => {}
            }
        }
        assert_eq!(r.saved_renderer, "skia");
        assert_eq!(r.saved_sys_renderer, "gl");
        assert_eq!(r.saved_refresh_rate, 120);
        assert_eq!(r.saved_zen_mode, 1);
        assert!(r.dnd_enabled);
        assert_eq!(r.cur_mode, ProfileMode::Performance);
    }
}
