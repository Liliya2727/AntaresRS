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

//! Profile and chipset tuning. Was the `sys.azenith-profilesettings` binary; now
//! a library the unified `sys.azenith-service` calls directly, so the profile
//! path no longer costs a fork+exec and no longer needs the caller check.

pub mod chipsets;
pub mod profiles;
pub mod utils;

/// Dispatches one subcommand. This is the whole former `main`.
pub fn run(args: &[String]) -> i32 {
    let Some(first) = args.first() else {
        return 0;
    };
    match first.as_str() {
        "0" | "initialize" => profiles::initialize(),
        "1" | "performance_profile" => profiles::performance_profile(),
        "2" | "balanced_profile" => profiles::balanced_profile(),
        "3" | "eco_mode" => profiles::eco_mode(),
        "applyfreqbalance" => utils::applyfreqbalance(),
        "applyfreqgame" => utils::applyfreqgame(),
        other => {
            // Passthrough, as the C did: an unrecognised argument that names an
            // existing file (or looks like one) is exec'd. A typo silently
            // becomes an exec attempt — a known wart, preserved deliberately.
            if std::path::Path::new(other).exists() || other.contains('.') {
                let _ = std::process::Command::new(other)
                    .args(&args[1..])
                    .status();
            }
        }
    }
    0
}
