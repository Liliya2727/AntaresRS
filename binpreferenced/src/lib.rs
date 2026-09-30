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

/// A subcommand containing `.` is treated as a path to exec, not a subcommand.
const PASS_THROUGH_MARK: char = '.';

pub mod prefs;
pub mod utils;

use std::fs;
use std::path::Path;
use std::process::Command;
use prefs::*;
use utils::*;

pub fn get_parent_pid() -> Option<u32> {
    fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|stat| {
            stat.split_whitespace()
                .nth(3)
                .and_then(|ppid| ppid.parse::<u32>().ok())
        })
}

pub fn get_process_cmdline(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{}/cmdline", pid))
        .ok()
        .map(|s| s.replace('\0', " ").trim().to_string())
}

// ponytail: still exported for manual on-device triage; the unified binary
// calls `run()` in-process so it never needs the ppid guard.
pub fn verify_caller() -> bool {
    if let Some(ppid) = get_parent_pid() {
        if let Some(cmdline) = get_process_cmdline(ppid) {
            return cmdline.contains("sys.azenith-service") || cmdline.contains("sys.azenith");
        }
    }
    false
}

/// Applies the eight preference tweaks, then flushes. This is the whole former
/// `main`, minus the `env::args()` read and the ppid guard — the unified binary
/// calls it in-process, so there is no parent to verify.
///
/// No arguments (the production invocation, which is how `binprofiles`
/// `initialize()` reaches this) means "apply everything".
pub fn run(args: &[String]) -> i32 {
    init_debugmode();

    match args.first().map(String::as_str) {
        None | Some("apply") | Some("prefs") | Some("preferred") => {
            prefsettings();
        }
        Some(other) => {
            // Passthrough, as before: only exec if the argument names something
            // that looks like a real path. A typo therefore becomes an exec
            // attempt, which is a wart of the original kept on purpose.
            if Path::new(other).exists() || other.contains(PASS_THROUGH_MARK) {
                let _ = Command::new(other).args(&args[1..]).status();
            }
        }
    }

    let _ = Command::new("sync").status();
    0
}
