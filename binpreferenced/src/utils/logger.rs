//
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
//

//! Status/verbose log writer for the preference tweaks.
//!
//! Writes straight into the shared AntaresRS `AZenith.log` / `AZenithVerbose.log`
//! files instead of shelling out to `sys.azenith-service`, keeping the exact
//! on-disk shape every other AntaresRS binary produces:
//!
//! ```text
//! YYYY-MM-DD HH:MM:SS.mmm LEVEL [component] message
//! ```
//!
//! DEBUG lines go to the verbose log (only when debug mode is on); every other
//! level goes to the main log. Each line is mirrored to logcat.

use crate::utils::debugmode;
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::raw::{c_char, c_int};
use std::os::unix::io::AsRawFd;

pub const LOG_DIR: &str = "/data/adb/.config/AntaresRS";
const LOG_FILE: &str = "/data/adb/.config/AntaresRS/AZenith.log";
const VERBOSE_FILE: &str = "/data/adb/.config/AntaresRS/AZenithVerbose.log";

const LOG_MAX_BYTES: u64 = 512 * 1024;
const LOG_KEEP_FILES: usize = 2;

const ANDROID_LOG_DEBUG: c_int = 3;
const ANDROID_LOG_INFO: c_int = 4;
const ANDROID_LOG_WARN: c_int = 5;
#[allow(dead_code)]
const ANDROID_LOG_ERROR: c_int = 6;

const LOGCAT_TAG: &str = "AZenith_Prefs";

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_print(prio: c_int, tag: *const c_char, fmt: *const c_char, ...) -> c_int;
}

fn logcat(prio: c_int, message: &str) {
    let Ok(tag) = CString::new(LOGCAT_TAG) else { return };
    let Ok(msg) = CString::new(message.replace('\0', "")) else { return };
    let fmt = c"%s";
    unsafe {
        __android_log_print(prio, tag.as_ptr(), fmt.as_ptr(), msg.as_ptr());
    }
}

fn timestamp() -> String {
    unsafe {
        let mut t: libc::timespec = std::mem::zeroed();
        if libc::clock_gettime(libc::CLOCK_REALTIME, &raw mut t) != 0 {
            return String::from("[TimeError]");
        }
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&raw const t.tv_sec, &raw mut tm).is_null() {
            return String::from("[TimeError]");
        }
        let mut buf = [0 as c_char; 32];
        let n = libc::strftime(buf.as_mut_ptr(), buf.len(), c"%Y-%m-%d %H:%M:%S".as_ptr(), &raw const tm);
        if n == 0 {
            return String::from("[TimeFormatError]");
        }
        let base = String::from_utf8_lossy(&buf[..n]).into_owned();
        format!("{}.{:03}", base, t.tv_nsec / 1_000_000)
    }
}

fn rotate_if_needed(path: &str) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() < LOG_MAX_BYTES {
        return;
    }

    for i in (1..=LOG_KEEP_FILES).rev() {
        let src = if i == 1 { path.to_string() } else { format!("{}.{}", path, i - 1) };
        let dst = format!("{}.{}", path, i);

        let _ = fs::remove_file(&dst);
        let _ = fs::rename(&src, &dst);
    }
}

fn append(path: &str, line: &str) {
    rotate_if_needed(path);

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else { return };
    let fd = file.as_raw_fd();
    unsafe { libc::flock(fd, libc::LOCK_EX) };
    let _ = file.write_all(line.as_bytes());
    unsafe { libc::flock(fd, libc::LOCK_UN) };
}

fn write_line(level: &str, prio: c_int, verbose: bool, component: &str, message: &str) {
    let _ = fs::create_dir_all(LOG_DIR);

    let component = if component.is_empty() { "prefs" } else { component };
    let line = format!("{} {} [{}] {}\n", timestamp(), level, component, message);
    append(if verbose { VERBOSE_FILE } else { LOG_FILE }, &line);

    logcat(prio, &format!("[{component}] {message}"));
}

/// DEBUG detail for a component; only emitted when debug mode is on, routed to
/// the verbose log.
pub fn log_verbose(component: &str, message: &str) {
    if debugmode() {
        write_line("DEBUG", ANDROID_LOG_DEBUG, true, component, message);
    }
}

/// A significant, low-frequency event (start/stop/profile change/config reload).
pub fn log_info(component: &str, message: &str) {
    write_line("INFO", ANDROID_LOG_INFO, false, component, message);
}

/// An oddity the caller recovered from; execution continues.
#[allow(dead_code)]
pub fn log_warn(component: &str, message: &str) {
    write_line("WARN", ANDROID_LOG_WARN, false, component, message);
}

/// An operation failed and needs attention. Include path/value/error context.
#[allow(dead_code)]
pub fn log_error(component: &str, message: &str) {
    write_line("ERROR", ANDROID_LOG_ERROR, false, component, message);
}
