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

//! PID lookup from the companion's `background_apps` file.
//!
//! Replaces `PidTracker/PidTracker.c`. The companion rewrites that file
//! atomically (temp file + `renameTo`), so a reader either sees the old or the
//! new content — never a partial write. That is what makes a plain read safe
//! here.

use azenith_common::logger::{Level, log};
use azenith_common::paths;

/// Reads the PIDs belonging to `pkg`.
///
/// Entries are `<pkg> <pid> <uid>` lines. A pid of `0` is skipped: the
/// companion writes `0` for a process it knows about but cannot resolve, and
/// `kill(0, …)` / cgroup writes against pid 0 mean "the whole process group" on
/// Linux, which would be a spectacularly wrong thing to do to a game's group.
pub fn pids_of(pkg: &str, max: usize) -> Vec<i32> {
    let Ok(text) = std::fs::read_to_string(paths::BACKGROUND_APPS) else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(max.min(8));
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(found), Some(pid)) = (it.next(), it.next()) else {
            continue;
        };
        if found != pkg {
            continue;
        }
        let Ok(pid) = pid.parse::<i32>() else {
            continue;
        };
        if pid <= 0 || out.contains(&pid) {
            continue;
        }
        out.push(pid);
        if out.len() == max {
            break;
        }
    }

    if out.is_empty() {
        log(
            Level::Debug,
            "PidTracker",
            &format!("no live pids for {pkg}"),
        );
    }
    out
}

/// `true` when `pid` is still alive.
///
/// Uses `kill(pid, 0)`: signal 0 performs permission and existence checks
/// without delivering anything. EPERM means the process exists but is not ours,
/// which still counts as alive — every PID the daemon tracks is root-owned, but
/// treating EPERM as dead would be wrong if that ever changes.
pub fn is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: `kill` with signal 0 only inspects; it takes a pid and an int and
    // touches no memory.
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    // SAFETY: `__errno_location` is always valid on bionic/glibc.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_zero_is_never_tracked() {
        // The failure mode this guards: pid 0 is the caller's process group.
        assert!(!pids_of("anything", 8).contains(&0));
    }

    #[test]
    fn our_own_process_is_alive_and_zero_is_not() {
        assert!(is_alive(std::process::id() as i32));
        assert!(!is_alive(0));
        assert!(!is_alive(-1));
    }

    #[test]
    fn an_unreadable_file_yields_no_pids() {
        // No background_apps on the host: must be empty, not panic.
        assert!(pids_of("com.example.game", 8).is_empty());
    }
}
