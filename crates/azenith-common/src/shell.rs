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

//! Controlled-environment child processes.
//!
//! Ported from the C `systemv()` / `execute_command()` / `execute_direct()` trio.
//! The important part is the environment: children get `PATH` and nothing else,
//! so they cannot see the daemon's `LD_LIBRARY_PATH` or `ANDROID_ROOT`.

use std::process::Command;

use crate::paths;

/// Runs `cmd` under `/system/bin/sh -c` with a cleared environment and only
/// `PATH` set, and returns its exit code, or `-1` when it could not be run.
///
/// This blocks on the child, exactly as the C version did via `waitpid` — the
/// daemon's main loop is intentionally serial about profile application.
pub fn systemv(cmd: &str) -> i32 {
    match Command::new("/system/bin/sh")
        .arg("-c")
        .arg(cmd)
        .env_clear()
        .env("PATH", paths::MY_PATH)
        .status()
    {
        Ok(status) => status.code().unwrap_or(-1),
        Err(_) => -1,
    }
}

/// Runs a binary with a cleared environment and returns its trimmed stdout.
///
/// The C `execute_command` capped output at 256 bytes; the cap was there to fit
/// a fixed buffer, so it is gone.
pub fn capture(cmd: &str) -> String {
    match Command::new("/system/bin/sh")
        .arg("-c")
        .arg(cmd)
        .env_clear()
        .env("PATH", paths::MY_PATH)
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => String::new(),
    }
}

/// Runs a command and returns stdout regardless of exit status.
///
/// Used where the C code ignored the status and only wanted the text, e.g. the
/// `dumpsys` parse in background-app clearing.
pub fn capture_any_status(cmd: &str) -> String {
    match Command::new("/system/bin/sh")
        .arg("-c")
        .arg(cmd)
        .env_clear()
        .env("PATH", paths::MY_PATH)
        .output()
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

/// Runs a command and reports whether it succeeded, discarding output.
pub fn run(cmd: &str) -> bool {
    systemv(cmd) == 0
}

/// Single-quotes `s` for safe interpolation into a `/system/bin/sh -c` command.
///
/// Package names and app labels flow into `cmd notification` and friends. They
/// come from the gamelist JSON and the companion's status file — both of which
/// the Manager writes, and the Manager takes package names from installed apps.
/// A label containing `;` or `$(...)` would otherwise be a shell injection into
/// a root process. Android's `toybox sh` has no `$'...'` and no here-strings,
/// so this stays POSIX: wrap in single quotes, and break out of the quoting to
/// embed a literal single quote.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}
