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

//! Ordered teardown. Replaces the exit path in `System.c` / `DaemonUtility.c`.
//!
//! Everything here must be safe to run exactly once, on any exit path,
//! including the fatal `integrity` exits — which is why it is a separate
//! function rather than inline code in the loop.

use azenith_common::android_props;
use azenith_common::logger::{Level, log};
use azenith_common::paths;

use crate::bypass_charge;
use crate::daemon::context::Daemon;

/// Runs the shutdown sequence and marks the daemon stopped.
///
/// Order matters: disabling bypass charging comes first because leaving a
/// vendor charging node latched is the one piece of this teardown that can
/// survive the process and affect the device afterwards.
pub fn run(daemon: &mut Daemon) {
    if daemon.bypass_applied {
        log(
            Level::Info,
            "System",
            "Disabling bypass charging before exit.",
        );
        let _ = bypass_charge::disable();
    }

    if !daemon.game_pids.is_empty() {
        log(Level::Info, "System", "Releasing game profile.");
        let pkg = daemon.gamestart.clone().unwrap_or_default();
        crate::handlers::resolution::restore(daemon, &pkg);
    }

    crate::config::state_handler::save(daemon);

    android_props::setprop("persist.sys.azenith.state", "stopped");
    android_props::setprop("persist.sys.azenith.service", "");

    // The state file is transient: `StateHandler` deletes it after restoring, so
    // a stale one must not survive this exit.
    let _ = std::fs::remove_file(paths::DAEMON_STATE_FILE);
}
