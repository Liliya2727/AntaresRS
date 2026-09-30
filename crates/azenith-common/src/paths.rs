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

//! Every path the module touches, in one place.
//!
//! Ported verbatim from the C daemon's `AZenith.h:57-73`. Nothing outside this
//! module should hardcode one of these strings.

pub const MODULE_DIR: &str = "/data/adb/modules/AZenith";
pub const MODULE_PROP: &str = "/data/adb/modules/AZenith/module.prop";
pub const MODULE_UPDATE: &str = "/data/adb/modules/AZenith/update";
pub const MODULE_REMOVE: &str = "/data/adb/modules/AZenith/remove";
pub const MODULE_REBOOT: &str = "/data/adb/modules/AZenith/reboot";

pub const CONFIG_DIR: &str = "/data/adb/.config/AZenith";
pub const API_DIR: &str = "/data/adb/.config/AZenith/API";
pub const GAMELIST_DIR: &str = "/data/adb/.config/AZenith/gamelist";
pub const DEBUG_DIR: &str = "/data/adb/.config/AZenith/debug";
pub const PRELOAD_DIR: &str = "/data/adb/.config/AZenith/preload";
pub const BYPASSCHG_CONFIG: &str = "/data/adb/.config/AZenith/bypasschgconfig";

pub const LOCK_FILE: &str = "/data/adb/.config/AZenith/API/.lock";
pub const JAVA_LOCK: &str = "/data/adb/.config/AZenith/java.lock";
pub const LOG_FILE: &str = "/data/adb/.config/AZenith/debug/AZenith.log";
pub const LOG_VFILE: &str = "/data/adb/.config/AZenith/debug/AZenithVerbose.log";
pub const LOG_FILE_PRELOAD: &str = "/data/adb/.config/AZenith/preload/AZenithPR.log";
pub const PROFILE_MODE: &str = "/data/adb/.config/AZenith/API/current_profile";
pub const PROFILE_MODE_APP: &str = "/data/data/zx.azenith/API/current_profile";
pub const GAME_INFO: &str = "/data/adb/.config/AZenith/API/gameinfo";
pub const GAME_INFO_APP: &str = "/data/data/zx.azenith/API/gameinfo";
pub const GAMELIST: &str = "/data/adb/.config/AZenith/gamelist/azenithApplist.json";
pub const DAEMON_MODES: &str = "/data/adb/.config/AZenith/API/current_modes";
pub const APP_MONITOR_FILE: &str = "/data/adb/.config/AZenith/app_status";
pub const BACKGROUND_APPS: &str = "/data/adb/.config/AZenith/background_apps";
pub const DAEMON_STATE_FILE: &str = "/data/adb/.config/AZenith/daemon_state";

/// A libsu symlink target on KSU/APatch; the C daemon relies on this being in
/// `PATH` so bare `sys.azenith-*` invocations resolve.
pub const KSU_BIN: &str = "/data/adb/ksu/bin";
pub const AP_BIN: &str = "/data/adb/ap/bin";

/// The page-touching preload binary, spawned as a separate process (plan Q3).
///
/// Spelled out as an absolute path rather than the bare `sys.azenith-preloadbin`
/// the C used: `preload.rs` spawns it with `Command::new`, which does not go
/// through `systemv()` and therefore gets no `PATH` fixup. An absolute path also
/// works on plain Magisk, where `customize.sh` creates no symlink at all.
pub const PRELOAD_BIN: &str = "/data/adb/modules/AZenith/system/bin/sys.azenith-preloadbin";

/// `PATH` handed to every child process. Matches the C `MY_PATH` macro
/// (`AZenith.h:84-86`) — `systemv()` gives children nothing else.
pub const MY_PATH: &str = "/system/bin:/system/xbin:/data/adb/ap/bin:/data/adb/ksu/bin:/data/adb/magisk:/debug_ramdisk:/sbin:/sbin/su:/su/bin:/su/xbin:/data/data/com.termux/files/usr/bin";
