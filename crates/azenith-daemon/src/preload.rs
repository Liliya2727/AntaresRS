// Copyright (C) 2026-2027 Zexshia
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Game library preloading (GamePreload.c).
//!
//! Runs on the `async-preload` thread after a game is detected. The
//! page-touching itself has to happen in a *separate process* (the
//! `sys.azenith-preloadbin` binary, plan Q3) because it maps hundreds of MB of
//! shared objects into the address space; doing that in the daemon would
//! inflate its RSS for the rest of the boot.
//!
//! ponytail: the C used `popen` and parsed the child's stdout with `sscanf`.
//! This still parses the child's stdout, because that output format is the
//! interface between the two binaries and changing both at once buys nothing.
//! The C also `sleep(5)`d first — kept: it lets the game's own loader win the
//! race for the pages that actually matter.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use azenith_common::android_props::getprop;
use azenith_common::logger::{Level, log};
use azenith_common::paths;

/// Extensions the C logged individually. Anything else is ignored.
const TRACKED_EXTENSIONS: [&str; 6] = ["so", "apk", "dm", "odex", "vdex", "art"];

/// Preload `package`'s libraries. Never propagates a failure: the C logged and
/// returned, and a failed preload must not disturb the game.
pub fn game_preload(package: &str) {
    std::thread::sleep(Duration::from_secs(5));

    if package.is_empty() {
        log(Level::Warn, "GamePreload", "package is null or empty");
        return;
    }

    let Some(apk_path) = apk_path(package) else {
        log(
            Level::Warn,
            "GamePreload",
            &format!("failed to get APK path for {package}"),
        );
        return;
    };
    let lib_path = apk_dir(&apk_path).join("lib/arm64");

    // The C prefers lib/arm64 when it holds a .so, and otherwise preloads the
    // split APKs — same decision, without a DIR* handle.
    let target = if has_shared_object(&lib_path) {
        log(
            Level::Info,
            "GamePreload",
            &format!("preloading game libs for {package}"),
        );
        lib_path
    } else {
        log(
            Level::Info,
            "GamePreload",
            &format!("preloading game split apks for {package}"),
        );
        apk_dir(&apk_path)
    };

    // getprop returns "" when unset, which is exactly the C's `PROP_VALUE_MAX`
    // empty case; it defaulted to 500M.
    let budget = getprop("persist.sys.azenithconf.preloadbudget");
    let budget = if budget.is_empty() { "500M" } else { &budget };

    let Some(stdout) = spawn_preload(&budget, &target) else {
        log(
            Level::Error,
            "GamePreload",
            &format!("failed to run preloadbin for {package}"),
        );
        return;
    };

    let (mut total_pages, mut total_size) = (0u64, String::new());
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some((pages, size)) = parse_touched_pages(&line) {
            total_pages += pages;
            total_size = size;
            continue;
        }
        if is_tracked_file(&line) {
            log(Level::Debug, "GamePreload", &format!("Touched: {line}"));
        }
    }

    log(
        Level::Info,
        "GamePreload",
        &format!("game {package} preloaded: total {total_pages} pages touched (~{total_size})"),
    );
}

/// `cmd package path <pkg> | head -n1 | cut -d: -f2`, without the pipeline.
///
/// ponytail: the C shelled out to `cmd` and post-processed with three text
/// tools to reach the same answer one `cmd` call gives directly. Same binary,
/// one fork instead of four.
fn apk_path(package: &str) -> Option<String> {
    let out = Command::new("cmd")
        .args(["package", "path", package])
        .output()
        .ok()?;
    let first = out.stdout.split(|b| *b == b'\n').find(|l| !l.is_empty())?;
    let path = String::from_utf8_lossy(first);
    let path = path.split_once(':')?.1.trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// Spawns the preload binary, returning the read end of its stdout.
fn spawn_preload(budget: &str, target: &Path) -> Option<std::process::ChildStdout> {
    Command::new(paths::PRELOAD_BIN)
        .args(["-v", "-t", "-m", budget])
        .arg(target)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()
        .and_then(|mut c| {
            let out = c.stdout.take();
            // Detached: nobody waits on it, and `game_preload` is already on its
            // own thread. Dropping the `Child` does not kill the process.
            std::mem::forget(c);
            out
        })
}

/// The directory holding the APK — the C's "last slash, truncated" logic.
fn apk_dir(apk_path: &str) -> PathBuf {
    Path::new(apk_path)
        .parent()
        .unwrap_or(Path::new("/"))
        .to_path_buf()
}

fn has_shared_object(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_str().is_some_and(|n| n.ends_with(".so")))
    })
}

/// Parses the preload binary's `Touched Pages: N (SIZE)` line.
///
/// The C used `sscanf(line, "Touched Pages: %d (%31[^)])", ...)` and required
/// *both* fields, or it logged a warning and contributed nothing to the total.
fn parse_touched_pages(line: &str) -> Option<(u64, String)> {
    let rest = line.split_once("Touched Pages:")?.1.trim_start();
    let (pages, tail) = rest.split_once('(')?;
    let size = tail.strip_suffix(')')?.trim();
    let pages: u64 = pages.trim().parse().ok()?;
    (!size.is_empty()).then(|| (pages, size.to_string()))
}

fn is_tracked_file(line: &str) -> bool {
    TRACKED_EXTENSIONS
        .iter()
        .any(|ext| line.ends_with(&format!(".{ext}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact line the preload binary prints. This function is the only
    /// thing standing between the C's `sscanf` and a silently wrong page
    /// count, so it gets a test for the real format.
    #[test]
    fn parses_the_preload_bin_summary_line() {
        assert_eq!(
            parse_touched_pages("[  1234/  5678] Touched Pages: 42 (168.0K)"),
            Some((42, "168.0K".to_string()))
        );
    }

    #[test]
    fn a_line_without_the_summary_is_not_a_summary() {
        for line in [
            "/data/app/x/lib/arm64/libfoo.so",
            "Touched Pages: 42",
            "Touched Pages: (168.0K)",
            "Touched Pages: abc (168.0K)",
            "Touched Pages: 42 ()",
        ] {
            assert_eq!(parse_touched_pages(line), None, "rejected: {line}");
        }
    }

    #[test]
    fn only_the_six_extensions_the_c_tracked_are_logged() {
        assert!(is_tracked_file("/x/libfoo.so"));
        assert!(is_tracked_file("/x/split_config.apk"));
        assert!(is_tracked_file("/x/base.dm"));
        assert!(is_tracked_file("/x/base.odex"));
        assert!(is_tracked_file("/x/base.vdex"));
        assert!(is_tracked_file("/x/base.art"));
        assert!(!is_tracked_file("/x/AndroidManifest.xml"));
        assert!(!is_tracked_file("/x/libfoo.so.debug"));
    }

    #[test]
    fn apk_dir_strips_the_final_component() {
        assert_eq!(
            apk_dir("/data/app/~~abc==/com.foo-1/base.apk"),
            Path::new("/data/app/~~abc==/com.foo-1")
        );
    }
}
