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

//! Stamps `AZENITH_MODULE_VERSION` into the crate.
//!
//! The version is **derived here**, not handed in. An earlier version of this
//! had `.github/scripts/verify.sh` `export AZENITH_VERSION` and build.rs read
//! it — which works in a shell and silently does not work in CI, where each
//! `run:` block is a separate process. The build then linked the `.placeholder`
//! default while `module.prop` carried the real version, and
//! `check_module_version()` hard-exits the daemon on install. Deriving it from
//! the same three sources verify.sh uses makes that class of desync
//! impossible: there is no hand-off to get wrong.
//!
//! Sources, all already in the repo: the `version` file, the `version_type`
//! file, and the git commit count + short hash. `AZENITH_VERSION` still wins if
//! set, so a local one-off build can override without editing a tracked file.

use std::path::Path;
use std::process::Command;

fn main() {
    let version = std::env::var("AZENITH_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(derive_version);

    println!("cargo:rustc-env=AZENITH_MODULE_VERSION={version}");
    println!("cargo:rerun-if-env-changed=AZENITH_VERSION");
    // Without these, editing `version` or committing would not retrigger the
    // build script and the daemon would ship last commit's version string.
    println!("cargo:rerun-if-changed=../../version");
    println!("cargo:rerun-if-changed=../../version_type");
}

/// `5.3 (1858-8211d46-Stellars-Beta)`, matching `compile_zip.sh`'s format.
fn derive_version() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crate is at <root>/crates/azenith-common")
        .to_path_buf();

    let read = |name: &str| {
        std::fs::read_to_string(root.join(name))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };

    let version = read("version");
    let version_type = read("version_type");
    let count = git(&root, &["rev-list", "HEAD", "--count"]);
    let short = git(&root, &["rev-parse", "--short", "HEAD"]);

    // Outside a git checkout, or with the version files missing, fall back to a
    // value that is obviously wrong rather than a plausible one: the integrity
    // check will refuse it, which is the correct outcome for a build that never
    // saw verify.sh.
    match (version.is_empty(), count, short) {
        (false, Ok(count), Ok(short)) => format!("{version} ({count}-{short}-{version_type})"),
        _ => ".unstamped".to_string(),
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, ()> {
    let out = Command::new("git").args(args).current_dir(dir).output().map_err(|_| ())?;
    if !out.status.success() {
        return Err(());
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() { Err(()) } else { Ok(text) }
}
