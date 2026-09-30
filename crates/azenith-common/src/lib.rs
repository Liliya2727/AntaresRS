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

//! Shared property, sysfs, logging, path and shell helpers for every AZenith binary.
//!
//! Extracted from the C daemon's `AZenith.h` macros and the three duplicate
//! `write_unlock_core()` copies in `binprofiles`/`binutils`/`binpreferenced`.

pub mod android_props;
pub mod logger;
pub mod paths;
pub mod shell;
pub mod sysfs;
pub mod version;

/// Reads a whole small config file, trimmed, or `""` when it is absent/unreadable.
pub fn read_trimmed(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
