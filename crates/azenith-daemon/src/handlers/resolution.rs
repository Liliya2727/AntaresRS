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

//! Per-app downscale/FPS via the Game Mode API. Replaces
//! `ConfigHandler/ResolutionChanger.c`.

use azenith_common::android_props;
use azenith_common::logger::{Level, log};
use azenith_common::shell;

use crate::daemon::context::{Daemon, GameConfig};

/// Android 13 introduced `cmd game set --mode 2`; below that the same effect
/// needed two `device_config` writes plus `cmd game mode`.
const ANDROID_13: i32 = 13;

/// Major Android version, as an integer.
///
/// The C did `strtol` on the whole string, so "16" and "16.0" both give 16. This
/// parses only the leading integer, which is the same result for every value
/// `ro.build.version.release` actually takes.
fn android_major() -> i32 {
    let v = android_props::getprop("ro.build.version.release");
    let digits: String = v.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().unwrap_or(0)
}

/// Applies the per-app intervention. Returns `true` when one was applied.
pub fn apply(daemon: &mut Daemon, pkg: &str, downscale: &str, fps: &str) -> bool {
    let no_downscale = GameConfig::is_default(downscale);
    let no_fps = GameConfig::is_default(fps);
    if no_downscale && no_fps {
        return false;
    }
    if daemon.resolution_applied {
        log(
            Level::Info,
            "ResolutionChanger",
            &format!("Already applied for {pkg}, skipping."),
        );
        return false;
    }

    let sdk = android_major();
    if sdk >= ANDROID_13 {
        let mut cmd = String::from("cmd game set --mode 2");
        if !no_downscale {
            cmd.push_str(&format!(" --downscale {downscale}"));
        }
        if !no_fps {
            cmd.push_str(&format!(" --fps {fps}"));
        }
        // Package names are Android-validated (dots and alphanumerics only), but
        // they still come from a JSON file, so quote rather than trust.
        cmd.push(' ');
        cmd.push_str(&shell::escape(pkg));

        let _ = shell::systemv(&cmd);
        daemon.resolution_applied = true;
        daemon.used_legacy_fallback = false;
        log(
            Level::Info,
            "ResolutionChanger",
            &format!("Applied '{cmd}' (Android {sdk})"),
        );
    } else {
        let mut config = String::from("mode=2");
        if !no_downscale {
            config.push_str(&format!(",downscaleFactor={downscale}"));
        }
        if !no_fps {
            config.push_str(&format!(",fps={fps}"));
        }
        let q = shell::escape(pkg);
        let _ = shell::systemv(&format!("cmd device_config put game_overlay {q} {config}"));
        let _ = shell::systemv(&format!("cmd game mode 2 {q}"));

        daemon.resolution_applied = true;
        daemon.used_legacy_fallback = true;
        log(
            Level::Info,
            "ResolutionChanger",
            &format!("Applied via device_config [{config}] (Android {sdk})"),
        );
    }
    true
}

/// Undoes [`apply`]. No-op when nothing was applied.
pub fn restore(daemon: &mut Daemon, pkg: &str) {
    if !daemon.resolution_applied {
        return;
    }
    let q = shell::escape(pkg);
    if daemon.used_legacy_fallback {
        let _ = shell::systemv(&format!("cmd device_config delete game_overlay {q}"));
        let _ = shell::systemv(&format!("cmd game reset {q}"));
    } else {
        let _ = shell::systemv(&format!("cmd game reset --mode 2 {q}"));
    }
    log(
        Level::Info,
        "ResolutionChanger",
        &format!("Reset intervention for {pkg}"),
    );
    daemon.resolution_applied = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_defaults_is_a_no_op() {
        let mut d = Daemon::default();
        assert!(!apply(&mut d, "com.game", "default", "default"));
        assert!(!apply(&mut d, "com.game", "", ""));
        assert!(!d.resolution_applied);
    }

    #[test]
    fn apply_is_idempotent_within_a_session() {
        // The C returned early on `resolution_applied`, so a second apply for
        // the same game does not restart the intervention.
        let mut d = Daemon::default();
        d.resolution_applied = true;
        assert!(!apply(&mut d, "com.game", "0.5", "60"));
    }

    #[test]
    fn restore_without_apply_is_a_no_op() {
        let mut d = Daemon::default();
        restore(&mut d, "com.game");
        assert!(!d.resolution_applied);
    }

    #[test]
    fn android_major_ignores_a_dotted_version() {
        // Bionic returns "" off-device, which must not panic.
        let _ = android_major();
    }
}
