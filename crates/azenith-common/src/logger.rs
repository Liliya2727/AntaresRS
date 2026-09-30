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

//! Timestamped file + logcat logging.
//!
//! The C `log_zenith` formatted into a 256-byte buffer and silently truncated.
//! There is no size ceiling here — a truncated sysfs path is a log line that
//! points at nothing, which was worse than a long line.

use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::android_props;
use crate::paths;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
    Fatal = 4,
}

impl Level {
    const fn short(self) -> char {
        match self {
            Self::Debug => 'D',
            Self::Info => 'I',
            Self::Warn => 'W',
            Self::Error => 'E',
            Self::Fatal => 'F',
        }
    }

    /// Android logcat priority. `Fatal` and `Debug` both fall to `DEBUG`,
    /// exactly as the C switch did.
    ///
    /// Not `#[cfg]`-gated: it is a pure total function over the enum, and gating
    /// it would delete the only test that pins the mapping to the C behaviour.
    /// The host build never calls it (there is no logcat off-device), hence the
    /// `dead_code` exemption.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    const fn android_priority(self) -> i32 {
        match self {
            Self::Info => 4,                // ANDROID_LOG_INFO
            Self::Warn => 5,                // ANDROID_LOG_WARN
            Self::Error => 6,               // ANDROID_LOG_ERROR
            Self::Debug | Self::Fatal => 3, // ANDROID_LOG_DEBUG
        }
    }
}

/// `logcat` sink. Uses `__android_log_write` with the message as an *argument*,
/// never as a format string — the old `thermalcore` passed it as the format,
/// which turns any `%` in a package name into a garbage read.
fn android_log(tag: &str, level: Level, message: &str) {
    let (Ok(c_tag), Ok(c_msg)) = (std::ffi::CString::new(tag), std::ffi::CString::new(message))
    else {
        return;
    };
    #[cfg(target_os = "android")]
    // SAFETY: both are valid NUL-terminated C strings for the call; bionic reads
    // `text` as a plain string and never applies printf semantics to it.
    unsafe {
        crate::android_props::logcat_write(
            level.android_priority(),
            c_tag.as_ptr(),
            c_msg.as_ptr(),
        );
    }
    #[cfg(not(target_os = "android"))]
    let _ = (&c_tag, &c_msg, level);
}

/// `YYYY-MM-DD HH:MM:SS.mmm` in **local** time, matching C `timern()`.
///
/// The old `binutils` logger forked `/bin/date` per line; this is a `SystemTime`
/// read plus a civil-from-days conversion. bionic gives local time as an offset
/// from UTC, so the offset has to be applied before the calendar conversion —
/// deriving the date from raw UTC seconds would log a different day than the
/// C version across the UTC boundary.
fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    secs += local_utc_offset_secs(secs);

    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);

    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}.{millis:03}",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// Local-time offset from UTC, in seconds, for the instant `utc_secs`.
///
/// bionic's `struct tm` carries `tm_gmtoff` (see `libc` `linux_like::tm`), so this
/// is a single libc call. It stays a call rather than a TZ-database walk because
/// Android ships no `/etc/localtime` on most builds, and hand-parsing zoneinfo
/// would silently fall back to UTC for exactly the users in non-UTC zones.
///
/// `localtime_r` is the reentrant form on purpose: the logger is reachable from
/// both daemon threads, and plain `localtime` would hand out a shared static.
fn local_utc_offset_secs(utc_secs: i64) -> i64 {
    // SAFETY: `tm` is a valid, fully-initialised out-param that outlives the
    // call, and the returned pointer is only read while it is still in scope.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&(utc_secs as libc::time_t), &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to (y, m, d).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Appends one line to `path`, creating it (and the log dir) on demand.
fn append_line(path: &str, level: Level, tag: &str, message: &str) {
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(
            format!("{} {} {}: {}\n", timestamp(), level.short(), tag, message).as_bytes(),
        );
    }
}

/// Logs to the main file and logcat. This is `log_zenith()`.
pub fn log(level: Level, tag: &str, message: &str) {
    append_line(paths::LOG_FILE, level, tag, message);
    android_log(tag, level, message);
}

/// Logs to the verbose file and logcat. This is `log_verbose()` — gated on
/// `persist.sys.azenith.debugmode == "true"`, checked at the call.
pub fn vlog(level: Level, tag: &str, message: &str) {
    append_line(paths::LOG_VFILE, level, tag, message);
    android_log(tag, level, message);
}

/// `true` when verbose logging is wanted. One property read per call, same as C.
pub fn debug_enabled() -> bool {
    android_props::is_true(&android_props::getprop("persist.sys.azenith.debugmode"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // 2024 is a leap year: day 59 is Feb 29.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        // Day numbers cross-checked against Python's datetime, not hand-counted:
        assert_eq!(civil_from_days(20_459), (2026, 1, 6));
        assert_eq!(civil_from_days(20_454), (2026, 1, 1));
    }

    #[test]
    fn timestamp_is_well_formed() {
        let ts = timestamp();
        // "YYYY-MM-DD HH:MM:SS.mmm" — 23 chars, separators at fixed offsets.
        assert_eq!(ts.len(), 23, "unexpected timestamp {ts:?}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], " ");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
        assert_eq!(&ts[19..20], ".");
        assert!(ts[20..23].chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn android_priority_matches_the_c_switch() {
        assert_eq!(Level::Info.android_priority(), 4);
        assert_eq!(Level::Warn.android_priority(), 5);
        assert_eq!(Level::Error.android_priority(), 6);
        assert_eq!(Level::Debug.android_priority(), 3);
        assert_eq!(Level::Fatal.android_priority(), 3);
    }
}
