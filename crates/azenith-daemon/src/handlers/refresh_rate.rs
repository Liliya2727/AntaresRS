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

//! Refresh-rate reads and application. Replaces
//! `ConfigHandler/RefreshRateHandler.c`.

use azenith_common::logger::{Level, log};
use azenith_common::paths;
use azenith_common::shell;

/// The companion's reported refresh rates, in Hz.
///
/// `None` when the field is absent, which is different from `Some(0)`: the C
/// returned `-1` for a missing file and `0` for a parsed zero, and the caller
/// only saves a rate when it is `> 0`, so both end up not applied — but the log
/// line differs and this keeps the distinction readable.
fn companion_rate(key: &str) -> Option<i32> {
    let text = std::fs::read_to_string(paths::APP_MONITOR_FILE).ok()?;
    for line in text.lines() {
        // Exact key match, not a prefix: `refresh_rate` must not also match
        // `max_refresh_rate`.
        let mut it = line.split_whitespace();
        match (it.next(), it.next()) {
            (Some(k), Some(v)) if k == key => return v.parse().ok(),
            (Some(k), None) if k == key => return None,
            _ => {}
        }
    }
    None
}

/// Current refresh rate in Hz, or `-1` when the companion has not reported one.
///
/// The C returned `-1` for both "file missing" and "field absent", so a caller
/// saving a rate to restore later would store `-1` and never restore.
pub fn current() -> i32 {
    companion_rate("refresh_rate").unwrap_or(-1)
}

/// Hardware maximum in Hz, defaulting to 60.
///
/// The default is not arbitrary: it is the C fallback, and 60 is the rate every
/// Android panel supports, so a wrong guess degrades to "no change" rather than
/// to an unsupported mode.
pub fn max() -> i32 {
    match companion_rate("max_refresh_rate") {
        Some(v) if v > 0 => v,
        _ => 60,
    }
}

/// Applies `target`, capped to the hardware maximum.
pub fn apply(target: i32) {
    if target <= 0 {
        return;
    }
    let cap = max();
    let final_rr = if target > cap {
        log(
            Level::Warn,
            "RefreshRate",
            &format!("Requested {target}Hz exceeds hardware max {cap}Hz. Capping to max."),
        );
        cap
    } else {
        target
    };

    log(
        Level::Info,
        "RefreshRate",
        &format!("Set refresh rates to {final_rr}Hz"),
    );
    // Numeric, so no escaping needed: the value is an i32 the gamelist parser
    // already validated.
    let _ = shell::systemv(&format!(
        "sys.azenith-service utils setrefreshrates {final_rr}"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_unknown_off_device() {
        // No companion file on the host: must be -1, not 0. A 0 would be
        // indistinguishable from "the panel reported zero".
        assert_eq!(current(), -1);
    }

    #[test]
    fn max_falls_back_to_sixty() {
        // The C default; a panic or 0 here would either crash the daemon or
        // request a 0Hz mode.
        assert_eq!(max(), 60);
    }

    #[test]
    fn apply_ignores_a_non_positive_target() {
        // Must not shell out with `--downscale 0` / `setrefreshrates 0`.
        apply(0);
        apply(-1);
    }
}
