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

//! Module tamper/version gate. Replaces `AZenithUtility/ModuleIntegrity.c`.
//!
//! Two checks, both by literal string match against `module.prop`:
//!   1. `is_kanged()` — the name and author lines must be exactly what this
//!      build expects. Anything else is a third-party edit and is fatal.
//!   2. `check_module_version()` — the `version=` line must equal the
//!      `MODULE_VERSION` compiled into this binary, which `build.rs` injected at
//!      compile time from `AZENITH_VERSION`.
//!
//! Both call `exit(1)` on failure after clearing the runtime state properties,
//! so a tampered module stops advertising itself as running.

use azenith_common::android_props;
use azenith_common::logger::{Level, log};
use azenith_common::version::MODULE_VERSION;

/// The exact `module.prop` header this build expects. Any change here is a
/// release-time decision, not a refactor.
const EXPECTED_NAME: &str = "name=AZenith火";
const EXPECTED_AUTHOR: &str = "author=ArchHaven Developers";

/// Reads `module.prop` once. Returns an empty string if it cannot be read, which
/// makes both checks fail — an unreadable module.prop is not a passing one.
fn module_prop() -> String {
    std::fs::read_to_string(azenith_common::paths::MODULE_PROP).unwrap_or_default()
}

/// True when the module's identity lines are intact. Non-fatal: this only
/// reports, because it is also called from the inotify handler where exiting is
/// handled by the caller.
pub fn is_intact() -> bool {
    let prop = module_prop();
    if prop.is_empty() {
        return false;
    }
    let name_ok = prop.lines().any(|l| l == EXPECTED_NAME);
    let author_ok = prop.lines().any(|l| l == EXPECTED_AUTHOR);
    name_ok && author_ok
}

/// Fatal integrity check, called at boot and on every `module.prop` event.
pub fn is_kanged() {
    if is_intact() {
        return;
    }
    log(
        Level::Error,
        "Integrity",
        "Module modified by 3rd party, exiting.",
    );
    clear_runtime_state();
    std::process::exit(1);
}

/// Fatal version check: the on-disk `version=` must equal the compiled-in one.
pub fn check_module_version() {
    let prop = module_prop();
    let expected = format!("version={MODULE_VERSION}");
    if prop.lines().any(|l| l == expected) {
        return;
    }
    log(
        Level::Error,
        "Integrity",
        &format!("Module version mismatch (module.prop != {MODULE_VERSION}), exiting."),
    );
    clear_runtime_state();
    std::process::exit(1);
}

/// Clears the properties that make the Manager believe the daemon is running.
///
/// Called on both fatal exits so the Manager shows "stopped" rather than a
/// stale "running" that nothing will ever update.
fn clear_runtime_state() {
    android_props::setprop("persist.sys.azenith.state", "stopped");
    android_props::setprop("persist.sys.azenith.service", "");
}

/// `true` when `MODULE_VERSION` is still the un-injected placeholder.
///
/// That happens when the daemon is built without `AZENITH_VERSION` set, i.e.
/// without running `verify.sh` first. On a real install it always fails the
/// version check and exits, so this is only a diagnostic for developers.
pub fn is_uninjected() -> bool {
    azenith_common::version::is_placeholder()
}

/// Prints the resolved build identity. Used by the `--version` CLI path.
pub fn describe() -> String {
    let state = if is_intact() { "intact" } else { "MODIFIED" };
    let arch = azenith_common::android_props::getprop("ro.product.cpu.abi");
    format!(
        "AZenith {MODULE_VERSION} [{state}] arch={}",
        if arch.is_empty() { "unknown" } else { &arch }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_module_prop_is_not_intact() {
        // On the host /data/adb/... does not exist, so this must be false —
        // an unreadable module.prop must never read as passing.
        assert!(!is_intact());
    }

    #[test]
    fn an_uninjected_build_is_flagged() {
        // Built without AZENITH_VERSION: the placeholder sentinel must be
        // detectable, otherwise a developer builds a daemon that hard-exits on
        // every device and has no warning.
        assert!(is_uninjected(), "cargo test builds without AZENITH_VERSION");
    }

    #[test]
    fn identity_matching_is_exact_not_substring() {
        let prop = "name=AZenith火\nauthor=ArchHaven Developers\nversion=1.2.3\n";
        assert!(prop.lines().any(|l| l == EXPECTED_NAME));
        assert!(prop.lines().any(|l| l == EXPECTED_AUTHOR));

        // A near-miss must not pass.
        let tampered = "name=AZenithX\nauthor=ArchHaven Developers\n";
        assert!(!tampered.lines().any(|l| l == EXPECTED_NAME));
    }
}
