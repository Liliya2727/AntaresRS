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

pub mod utils;

use std::process::Command;
use utils::*;

/// Dispatches one subcommand. This is the whole former `main`, minus the
/// `env::args()` read: the unified binary already has the arguments and passes
/// them down, so there is no reason to re-parse the process environment.
///
/// Returns a process exit code. The dispatch itself has no error channel —
/// each subcommand logs its own failures, as it always has.
pub fn dispatch(args: &[String]) -> i32 {
    let Some(function) = args.first() else {
        return 0;
    };
    let rest = &args[1..];
    // Subcommands that need an argument silently no-op without one, exactly as
    // the `if args.len() > 2` guards did. Preserved rather than "improved": the
    // daemon's own output is what a user debugs with.
    let arg = || rest.first().map(String::as_str);

    match function.as_str() {
        "setsgov" => { if let Some(a) = arg() { setsgov(a) } }
        "setsIO" => { if let Some(a) = arg() { sets_io(a) } }
        "setsMaliGov" => { if let Some(a) = arg() { sets_mali_gov(a) } }
        "setthermalcore" => { if let Some(a) = arg() { setthermalcore(a) } }
        "checkmalipath" => check_mali_path(),
        "FSTrim" => fstrim(),
        "enableDND" => enable_dnd(),
        "disableDND" => disable_dnd(),
        "setrefreshrates" => { if let Some(a) = arg() { setrefreshrates(a) } }
        "restartservice" => restartservice(),
        "setrender" => { if let Some(a) = arg() { setrender(a) } }
        // Passthrough for anything unrecognised. A typo in a subcommand name
        // therefore becomes an attempted exec — a known wart of the original,
        // and the reason the unified binary keeps this behind `dispatch`. A
        // failed exec reports 1 so a typo is visible to a caller that checks.
        other => {
            if Command::new(other).args(rest).status().is_err() {
                return 1;
            }
        }
    }
    0
}
