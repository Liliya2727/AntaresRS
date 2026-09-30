// Copyright (C) 2024-2025 Zexshia
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

//! Deletes `persist.sys.azenith*` properties that are not in the whitelist.
//!
//! Replaces `StartupInit/PropValidator.c`. This is a live contract with the
//! Manager app: a property the Manager writes but this list omits is silently
//! destroyed on the next daemon start.

use azenith_common::android_props;
use azenith_common::logger::{Level, log};
use azenith_common::shell;

/// Properties the daemon and the Manager are allowed to keep.
///
/// `persist.sys.azenithconf.renderer` appears twice in the C array; a duplicate
/// entry changes nothing for a membership test, so it is not reproduced here.
const VALID_AZENITH_PROPS: &[&str] = &[
    "persist.sys.azenith.custom_default_balanced_IO",
    "persist.sys.azenith.custom_default_cpu_gov",
    "persist.sys.azenith.custom_default_maligpu_gov",
    "persist.sys.azenith.custom_performance_IO",
    "persist.sys.azenith.custom_performance_cpu_gov",
    "persist.sys.azenith.custom_performance_maligpu_gov",
    "persist.sys.azenith.custom_powersave_IO",
    "persist.sys.azenith.custom_powersave_cpu_gov",
    "persist.sys.azenith.custom_powersave_maligpu_gov",
    "persist.sys.azenith.debugmode",
    "persist.sys.azenith.default_balanced_IO",
    "persist.sys.azenith.default_cpu_gov",
    "persist.sys.azenith.default_maligpu_gov",
    "persist.sys.azenith.disabletweak",
    "persist.sys.azenith.service",
    "persist.sys.azenith.soctype",
    "persist.sys.azenith.state",
    "persist.sys.azenith.profilenotifications",
    "persist.sys.azenith.dropforeground",
    "persist.sys.azenithconf.AIenabled",
    "persist.sys.azenithconf.APreload",
    "persist.sys.azenithconf.DThermal",
    "persist.sys.azenithconf.SFL",
    "persist.sys.azenithconf.bypasschg",
    "persist.sys.azenithconf.bypasschgthreshold",
    "persist.sys.azenithconf.renderer",
    "persist.sys.azenithconf.bypasspath",
    "persist.sys.azenithconf.clearbg",
    "persist.sys.azenithconf.cpulimit",
    "persist.sys.azenithconf.disabletrace",
    "persist.sys.azenithconf.dnd",
    "persist.sys.azenithconf.fpsged",
    "persist.sys.azenithconf.freqoffset",
    "persist.sys.azenithconf.fstrim",
    "persist.sys.azenithconf.iosched",
    "persist.sys.azenithconf.justintime",
    "persist.sys.azenithconf.litemode",
    "persist.sys.azenithconf.logd",
    "persist.sys.azenithconf.malisched",
    "persist.sys.azenithconf.preloadbudget",
    "persist.sys.azenithconf.schedtunes",
    "persist.sys.azenithconf.schemeconfig",
    "persist.sys.azenithconf.showtoast",
    "persist.sys.azenithconf.thermalcore",
    "persist.sys.azenithconf.usefpsgo",
    "persist.sys.azenithconf.walttunes",
];

const PREFIX: &str = "persist.sys.azenith";

/// Caps how many names get buffered before deletion, matching the C's
/// `MAX_PENDING_DELETE`. Overflow is logged, not silently dropped.
const MAX_PENDING_DELETE: usize = 128;

/// Deletes any `persist.sys.azenith*` property not in [`VALID_AZENITH_PROPS`].
pub fn validate() {
    let mut pending: Vec<String> = Vec::new();

    android_props::foreach_prop(|name| {
        if !name.starts_with(PREFIX) {
            return;
        }
        if VALID_AZENITH_PROPS.contains(&name) {
            return;
        }
        if pending.len() < MAX_PENDING_DELETE {
            log(
                Level::Warn,
                "PropValidator",
                &format!("flagged stale prop -> {name}"),
            );
            pending.push(name.to_string());
        } else {
            log(
                Level::Warn,
                "PropValidator",
                &format!("buffer full, skip {name}"),
            );
        }
    });

    if pending.is_empty() {
        log(
            Level::Info,
            "PropValidator",
            "no unused prop found, continue...",
        );
        return;
    }

    log(
        Level::Info,
        "PropValidator",
        &format!("Found {} unused prop cleaning...", pending.len()),
    );
    for name in &pending {
        log(Level::Warn, "PropValidator", &format!("delete -> {name}"));
        let _ = shell::systemv(&format!("resetprop -p --delete {name}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_whitelisted_prop_uses_the_prefix() {
        // A typo outside the prefix could never be flagged for deletion, which
        // would silently disable the validator for that key.
        for p in VALID_AZENITH_PROPS {
            assert!(p.starts_with(PREFIX), "{p} is outside the prefix");
        }
    }

    #[test]
    fn whitelist_has_no_duplicates() {
        // The C array listed persist.sys.azenithconf.renderer twice.
        let mut seen = std::collections::HashSet::new();
        for p in VALID_AZENITH_PROPS {
            assert!(seen.insert(*p), "duplicate entry {p}");
        }
    }

    #[test]
    fn a_known_prop_is_not_queued() {
        assert!(VALID_AZENITH_PROPS.contains(&"persist.sys.azenith.state"));
        assert!(!VALID_AZENITH_PROPS.contains(&"persist.sys.azenith.conf.typo"));
    }
}
