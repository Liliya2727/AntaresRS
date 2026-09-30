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

//! The one sysfs/procfs write primitive in the project.
//!
//! Replaces `echo_to_file()` (C `FileHandler`/`BypassCharge`) and the three
//! identical `write_unlock_core()` copies in the old crates. The chmod dance is
//! not decoration: a node left at 0644 gets clobbered by the ROM's own userspace
//! writers, which is why the daemon re-locks what it sets.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Writes `value\n` to `path`, optionally re-locking the node read-only.
///
/// A missing path is a silent success, not an error: the whole point is that
/// one code path can drive every vendor's node set without `exists()` guards at
/// each call site.
pub fn write_sysfs(path: &str, value: &str, lock: bool) -> std::io::Result<()> {
    let p = Path::new(path);
    if !p.exists() {
        return Ok(());
    }

    fs::set_permissions(p, fs::Permissions::from_mode(0o644))?;
    fs::write(p, format!("{value}\n"))?;
    if lock {
        fs::set_permissions(p, fs::Permissions::from_mode(0o444))?;
    }
    Ok(())
}

/// [`write_sysfs`] without the re-lock.
pub fn write_unlock(path: &str, value: &str) {
    let _ = write_sysfs(path, value, false);
}

/// [`write_sysfs`] with the re-lock.
pub fn write_lock(path: &str, value: &str) {
    let _ = write_sysfs(path, value, true);
}

/// `chmod` that tolerates a missing node, for the lock-only calls the old
/// `binutils` scattered around its governor writes.
pub fn chmod(path: &str, mode: u32) {
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(mode);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Under the project dir, not `std::env::temp_dir()`: on this host /tmp is a
    // RAM-backed tmpfs that parallel agent sessions exhaust.
    fn tmp(tag: &str) -> String {
        let dir = format!(
            "{}/target/azenith-sysfs-test-{}-{}",
            env!("CARGO_MANIFEST_DIR"),
            std::process::id(),
            tag
        );
        let _ = fs::create_dir_all(&dir);
        let p = format!("{dir}/node");
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn writes_value_with_trailing_newline() {
        let p = tmp("nl");
        fs::write(&p, "old").unwrap();
        write_sysfs(&p, "42", false).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "42\n");
        fs::remove_file(&p).ok();
    }

    #[test]
    fn lock_leaves_node_read_only() {
        let p = tmp("lock");
        fs::write(&p, "old").unwrap();
        write_sysfs(&p, "7", true).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "7\n");
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o444, "locking must end at 0444, got {mode:o}");
        fs::remove_file(&p).ok();
    }

    #[test]
    fn missing_node_is_silent_success() {
        // The whole cross-vendor design depends on this: writing a node a given
        // SoC does not have must not be an error.
        let p = tmp("absent");
        assert!(write_sysfs(&p, "1", true).is_ok());
    }
}
