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

//! I/O priority and OOM-reclaim priority for game PIDs.
//!
//! Replaces `AZenithUtility/AppPriority.c`.
//!
//! Only `ioprio_set` and `oom_score_adj` are used. The C also wrote `nice = -20`,
//! which needs `CAP_SYS_NICE` and silently failed from a non-init caller — so it
//! is dropped rather than kept as a no-op that logs an error every game start.
//!
//! Every constant below is from the NDK's own
//! `sysroot/usr/include/linux/ioprio.h` and `asm-generic/unistd.h`, not from
//! memory: `__NR_ioprio_set` is **30** (not 251, which is an x86-64-era number
//! that does not apply here), `IOPRIO_CLASS_SHIFT` is 13, and there are only
//! four classes — `RT`, `BE`, `IDLE`, `NONE`.

use azenith_common::logger::{Level, log};

/// `__NR_ioprio_set` from `asm-generic/unistd.h:55`.
const SYS_IOPRIO_SET: libc::c_long = 30;
/// `IOPRIO_CLASS_SHIFT` from `linux/ioprio.h:10`.
const IOPRIO_CLASS_SHIFT: libc::c_long = 13;
/// `IOPRIO_CLASS_BE` from `linux/ioprio.h:21` — best-effort, the class a game
/// wants: real priority without starving a foreground sync.
const IOPRIO_CLASS_BE: libc::c_long = 2;
/// `IOPRIO_BE_NR` is `IOPRIO_NORM` (4): data `0` is the *best* BE level.
const IOPRIO_DATA_BEST: libc::c_long = 0;
/// `IOPRIO_WHO_PROCESS` from `linux/ioprio.h:30`.
const IOPRIO_WHO_PROCESS: libc::c_int = 1;

/// Best-effort priority for one PID. Never fatal — a PID can die between the
/// companion's report and this call, and `panic = "abort"` makes any panic here
/// fatal to the whole daemon.
pub fn set_priority(pid: i32) {
    if pid <= 0 {
        return;
    }
    if let Err(e) = set_ioprio(pid) {
        log(
            Level::Debug,
            "AppPriority",
            &format!("ioprio for {pid}: {e}"),
        );
    }
    if let Err(e) = set_oom_score_adj(pid) {
        log(
            Level::Debug,
            "AppPriority",
            &format!("oom_score_adj for {pid}: {e}"),
        );
    }
}

/// `ioprio_set(IOPRIO_WHO_PROCESS, pid, IOPRIO_CLASS_BE << 13 | 0)`.
fn set_ioprio(pid: i32) -> Result<(), std::io::Error> {
    let ioprio = (IOPRIO_CLASS_BE << IOPRIO_CLASS_SHIFT) | IOPRIO_DATA_BEST;
    // SAFETY: `ioprio_set` takes three ints and touches no memory. `syscall(2)`
    // is variadic, and these are the three `long`/`int` arguments the kernel
    // reads — all scalars, so no pointer-validity obligation applies.
    let rc = unsafe {
        libc::syscall(
            SYS_IOPRIO_SET,
            IOPRIO_WHO_PROCESS,
            pid as libc::c_int,
            ioprio,
        )
    };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Lowers OOM-reclaim score so the kernel prefers killing something other than
/// the game when memory runs out.
///
/// Written directly rather than through `sysfs::write_sysfs`: that helper
/// `chmod`s the node first, and `/proc/<pid>/oom_score_adj` rejects `chmod` with
/// EPERM even as root.
fn set_oom_score_adj(pid: i32) -> Result<(), String> {
    let path = format!("/proc/{pid}/oom_score_adj");
    std::fs::write(&path, b"-500\n").map_err(|e| format!("{path}: {e}"))
}

/// `true` while a renderer restart is in flight.
///
/// The C kept this as a global set by the app-restart path. It is a single bit
/// read by the inotify handler on a different call path, so it is an atomic
/// rather than a `Daemon` field that would need `&mut` plumbing.
static RESTARTING_RENDERER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_restarting_renderer(on: bool) {
    RESTARTING_RENDERER.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_restarting_renderer() -> bool {
    RESTARTING_RENDERER.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encoded_ioprio_matches_the_kernel_encoding() {
        // Best-effort, best data level = 2 << 13 = 16384. If this ever changes,
        // the kernel silently stores a different class than intended.
        assert_eq!(
            (IOPRIO_CLASS_BE << IOPRIO_CLASS_SHIFT) | IOPRIO_DATA_BEST,
            16384
        );
        assert_eq!(SYS_IOPRIO_SET, 30);
    }

    #[test]
    fn a_dead_or_invalid_pid_is_not_fatal() {
        // Above the default pid_max: must return, not panic. Under
        // `panic = "abort"` a panic here would kill the daemon mid-game.
        set_priority(0);
        set_priority(-1);
        set_priority(4_194_303);
    }

    #[test]
    fn the_renderer_flag_is_settable_without_daemon_state() {
        set_restarting_renderer(true);
        assert!(is_restarting_renderer());
        set_restarting_renderer(false);
        assert!(!is_restarting_renderer());
    }
}
