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

//! Build-time module version.
//!
//! Injected by `build.rs` from `AZENITH_VERSION`, which `.github/scripts/verify.sh`
//! sets. The C daemon instead had `#define MODULE_VERSION` `sed`-patched into
//! `AZenith.h` — the failure mode there was a local build silently producing a
//! daemon that hard-exits on a real install, because the tree default is
//! `.placeholder`.
//!
//! `verify_system_integrity()` compares this against the `version=` line in
//! `module.prop`, so a mismatch is a refusal to start, not a warning.

/// The version string compiled into this binary.
pub const MODULE_VERSION: &str = env!("AZENITH_MODULE_VERSION");

/// `true` when this build was never stamped by `verify.sh`.
///
/// Guards nothing at runtime; it exists so a locally built binary says so in the
/// log instead of looking like a real install.
pub fn is_placeholder() -> bool {
    MODULE_VERSION == ".placeholder"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set_at_compile_time() {
        // Any value is acceptable; what must not happen is an empty string,
        // which would make every integrity comparison vacuously true.
        assert!(!MODULE_VERSION.is_empty());
    }
}
