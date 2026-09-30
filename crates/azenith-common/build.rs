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
//! The version is **derived here**, not handed in. An earlier version had
//! `.github/scripts/verify.sh` `export AZENITH_VERSION` and build.rs read it
//! — which works in a shell and silently does not work in CI, where each
//! `run:` block is a separate process. The build then linked the `.placeholder`
//! default while `module.prop` carried the real version, and
//! `check_module_version()` hard-exits the daemon on install. Deriving it from
//! the same inputs `compile_zip.sh` uses makes that class of desync impossible:
//! there is no hand-off to get wrong.
//!
//! Cache invalidation: Cargo only reruns a build script when a file it was told
//! to watch changes, and git state is not a file. `.git/index` is — `git commit`
//! rewrites it — so watching it means a new commit reruns this script, and
//! therefore relinks everything downstream. That cost is only paid on commit,
//! and a daemon carrying the previous commit's version is a boot-time failure
//! with no log, so paying it is the point.
//!
//! `AZENITH_VERSION` still wins if set, so a one-off build can override without
//! editing a tracked file.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let version = std::env::var("AZENITH_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(derive_version);

    println!("cargo:rustc-env=AZENITH_MODULE_VERSION={version}");
    println!("cargo:rerun-if-env-changed=AZENITH_VERSION");
    println!("cargo:rerun-if-changed=../../version");
    println!("cargo:rerun-if-changed=../../version_type");

    if let Some(root) = git_root() {
        // Two paths because either alone has a hole: `.git/HEAD` catches a new
        // branch or a fresh clone, `.git/index` catches a new commit on the
        // current branch. A merge or rebase rewrites both.
        println!("cargo:rerun-if-changed={}", root.join(".git/HEAD").display());
        println!("cargo:rerun-if-changed={}", root.join(".git/index").display());
    }
}

/// `5.3 (1858-8211d46-Stellars-Beta)`, matching `compile_zip.sh`'s format.
fn derive_version() -> String {
    let root = repo_root();

    let read = |name: &str| {
        std::fs::read_to_string(root.join(name))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };

    let version = read("version");
    let version_type = read("version_type");
    let count = git(&root, &["rev-list", "HEAD", "--count"]);
    let short = git(&root, &["rev-parse", "--short", "HEAD"]);

    // Outside a git checkout, or with the version files missing, fail the build
    // rather than stamping something plausible. The C's failure mode here was
    // silently shipping `.placeholder`, which the integrity check then refused
    // on the device with nothing in the log; a loud build failure is strictly
    // better than a daemon that dies at boot.
    match (version.is_empty(), count, short) {
        (false, Ok(count), Ok(short)) => format!("{version} ({count}-{short}-{version_type})"),
        (true, _, _) => panic!(
            "version file missing or empty at {}; expected the repo root",
            root.display()
        ),
        (false, Err(_), _) | (false, _, Err(_)) => panic!(
            "cannot read git state at {}; the compiled-in version would not \
             match module.prop and the daemon would refuse to start",
            root.display()
        ),
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crate lives at <root>/crates/azenith-common")
        .to_path_buf()
}

fn git_root() -> Option<PathBuf> {
    let root = repo_root();
    git(&root, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from)
}

fn git(dir: &Path, args: &[&str]) -> Result<String, ()> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|_| ())?;
    if !out.status.success() {
        return Err(());
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        Err(())
    } else {
        Ok(text)
    }
}
