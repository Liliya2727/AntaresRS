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

//! HWUI renderer switching. Replaces `ConfigHandler/RenderingHandler.c`.

use azenith_common::android_props;
use azenith_common::logger::{Level, log};
use azenith_common::shell;

use crate::daemon::context::Daemon;

const PROP_RENDERER: &str = "debug.hwui.renderer";
const PROP_CONF: &str = "persist.sys.azenithconf.renderer";

/// Switches the HWUI renderer, saving the previous value on first call.
///
/// Returns `true` when a change was made, which is the caller's cue that the
/// game must be restarted for it to take effect.
pub fn apply(daemon: &mut Daemon, target: &str) -> bool {
    // "default" and empty both mean "don't touch the renderer" in the C — the
    // gamelist writes "default" for apps that do not override it.
    if target.is_empty() || target == "default" {
        return false;
    }

    let current = read_or_default(PROP_RENDERER);
    let current_conf = read_or_default(PROP_CONF);

    // Saved on first call only, so a second profile apply does not overwrite the
    // original with the value this daemon itself set.
    if daemon.saved_renderer.is_empty() {
        daemon.saved_renderer = current.clone();
        daemon.saved_sys_renderer = current_conf;
    }

    if current == target {
        return false;
    }

    log(
        Level::Info,
        "RenderHandler",
        &format!("Renderer mismatch! Current: {current} | Target: {target}. Switching..."),
    );
    let _ = shell::systemv(&format!("sys.azenith-service utils setrender {target}"));
    android_props::setprop(PROP_CONF, target);
    true
}

/// Restores the renderer saved by [`apply`], if one was saved and still differs.
///
/// Shared by the Balanced and Eco paths, which were byte-identical in the C.
pub fn restore(daemon: &mut Daemon) {
    if daemon.saved_renderer.is_empty() {
        return;
    }
    let saved = daemon.saved_renderer.clone();

    let current = read_or_default(PROP_RENDERER);
    if current != saved {
        log(
            Level::Info,
            "RenderHandler",
            &format!("Restoring original system renderer: {saved}"),
        );
        if saved == "default" {
            let _ = shell::systemv("sys.azenith-service utils setrender default");
            android_props::setprop(PROP_CONF, "default");
        } else {
            let _ = shell::systemv(&format!("sys.azenith-service utils setrender {saved}"));
            android_props::setprop(PROP_CONF, &daemon.saved_sys_renderer);
        }
    }
    daemon.saved_renderer.clear();
    daemon.saved_sys_renderer.clear();
}

fn read_or_default(prop: &str) -> String {
    let v = android_props::getprop(prop);
    if v.is_empty() {
        "default".to_string()
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_empty_are_no_ops() {
        // Both spellings mean "not overridden" in the gamelist; touching the
        // renderer here would force a pointless app restart every profile apply.
        let mut d = Daemon::default();
        assert!(!apply(&mut d, "default"));
        assert!(!apply(&mut d, ""));
        assert!(d.saved_renderer.is_empty(), "a no-op must not save a value");
    }

    #[test]
    fn restore_without_a_saved_value_does_nothing() {
        let mut d = Daemon::default();
        restore(&mut d);
        assert!(d.saved_renderer.is_empty());
    }
}
