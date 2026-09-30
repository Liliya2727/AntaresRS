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

//! Parses `gamelist/azenithApplist.json`, the per-app settings file.
//!
//! The C version (`AppLoader.c`) hand-scanned the file with `strstr` for the
//! literal `": {`, then scanned forward for each of the nine setting keys,
//! bounded each time by the *next* `": {` so a key could not leak across
//! entries. It ignored the JSON grammar entirely — which is why it silently
//! produced `"default"` for any key the Manager had not written yet, and why a
//! malformed file degraded to an empty cache with no diagnostic beyond a log
//! line.
//!
//! This uses `serde` instead. Same input, same output for well-formed files, but
//! a real parse error is now reported instead of producing a half-populated
//! cache.
//!
//! ## Format
//!
//! A flat object keyed by package name:
//!
//! ```json
//! {
//!   "com.example.game": {
//!     "perf_lite_mode": "default",
//!     "dnd_on_gaming": "default",
//!     "app_priority": "default",
//!     "game_preload": "default",
//!     "refresh_rate": "default",
//!     "renderer": "default",
//!     "resolution_downscale": "default",
//!     "resolution_fps": "60",
//!     "bypass_charging": "default"
//!   }
//! }
//! ```
//!
//! The seed file at the repo root still carries an older `resolution_target`
//! key instead of `resolution_downscale`/`resolution_fps`. That is harmless here
//! (unknown keys are ignored, the two fields fall back to `"default"`) but note
//! the C parser would have found neither, so it behaved the same way.
//!
//! Values are all strings — the Manager writes enum names, not numbers.

use std::collections::BTreeMap;

use azenith_common::logger::{Level, log};
use azenith_common::paths;
use serde::Deserialize;

use super::super::daemon::context::GameConfig;

/// Raw on-disk shape: package name -> its settings.
///
/// `serde_json::Map` preserves nothing useful here, so this is a plain
/// `BTreeMap`: ordering is irrelevant for lookups and it gives a deterministic
/// iteration order for logging, which `HashMap` would not.
type RawGamelist = BTreeMap<String, RawGameEntry>;

/// The nine per-app settings. Every one is optional because the Manager writes
/// only the keys the user has touched, and older Manager versions wrote fewer.
#[derive(Debug, Default, Deserialize)]
struct RawGameEntry {
    perf_lite_mode: Option<String>,
    dnd_on_gaming: Option<String>,
    app_priority: Option<String>,
    game_preload: Option<String>,
    refresh_rate: Option<String>,
    renderer: Option<String>,
    resolution_downscale: Option<String>,
    resolution_fps: Option<String>,
    bypass_charging: Option<String>,
}

/// Why a gamelist read failed. Kept as a real error type rather than a bool so
/// the caller can log the specific cause — the C code collapsed "file missing",
/// "empty" and "malformed" into one log line.
#[derive(Debug, PartialEq, Eq)]
pub enum GamelistError {
    /// Could not read the file at all.
    Io(std::io::ErrorKind),
    /// File was present but not valid JSON, or not the expected shape.
    Parse(String),
    /// File parsed but held no entries.
    Empty,
}

impl std::fmt::Display for GamelistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(k) => write!(f, "cannot read {}: {k}", paths::GAMELIST),
            Self::Parse(m) => write!(f, "malformed gamelist: {m}"),
            Self::Empty => write!(f, "gamelist is empty"),
        }
    }
}

/// Reads and parses the gamelist from disk.
pub fn load_gamelist() -> Result<Vec<GameConfig>, GamelistError> {
    let raw = std::fs::read_to_string(paths::GAMELIST).map_err(|e| GamelistError::Io(e.kind()))?;
    parse_gamelist(&raw)
}

/// Parses a gamelist payload. Separated from the read so it is testable.
pub fn parse_gamelist(raw: &str) -> Result<Vec<GameConfig>, GamelistError> {
    let map: RawGamelist =
        serde_json::from_str(raw).map_err(|e| GamelistError::Parse(e.to_string()))?;

    if map.is_empty() {
        return Err(GamelistError::Empty);
    }

    Ok(map
        .into_iter()
        .map(|(package, e)| e.into_config(package))
        .collect())
}

impl RawGameEntry {
    /// Materialises one cache row. Absent keys become `"default"`, which is the
    /// same sentinel the C parser used for "not found" and what
    /// [`GameConfig::is_default`] tests against.
    fn into_config(self, package: String) -> GameConfig {
        const D: &str = "default";
        GameConfig {
            package,
            perf_lite_mode: self.perf_lite_mode.unwrap_or_else(|| D.into()),
            dnd_on_gaming: self.dnd_on_gaming.unwrap_or_else(|| D.into()),
            app_priority: self.app_priority.unwrap_or_else(|| D.into()),
            game_preload: self.game_preload.unwrap_or_else(|| D.into()),
            refresh_rate: self.refresh_rate.unwrap_or_else(|| D.into()),
            renderer: self.renderer.unwrap_or_else(|| D.into()),
            resolution_downscale: self.resolution_downscale.unwrap_or_else(|| D.into()),
            resolution_fps: self.resolution_fps.unwrap_or_else(|| D.into()),
            bypass_charging: self.bypass_charging.unwrap_or_else(|| D.into()),
        }
    }
}

/// Loads the gamelist into the daemon cache, logging the outcome.
///
/// Returns the number of entries cached. The C `reload_gamelist_cache` took the
/// daemon context only to decide whether to log; the log is unconditional here
/// because the caller decides when a reload is interesting.
pub fn reload_cache(cache: &std::sync::Mutex<Vec<GameConfig>>) -> usize {
    match load_gamelist() {
        Ok(list) => {
            let n = list.len();
            // Replace under one lock. The C version freed the old array, then
            // built a new one, then published it — a concurrent reader could see
            // `g_game_cache == NULL` in the middle.
            match cache.lock() {
                Ok(mut g) => *g = list,
                Err(_) => {
                    log(
                        Level::Error,
                        "AppLoader",
                        "gamelist cache mutex poisoned, keeping old list",
                    );
                    return 0;
                }
            }
            log(
                Level::Info,
                "AppLoader",
                &format!("Gamelist cached. Total: {n} games registered."),
            );
            n
        }
        Err(e) => {
            log(
                Level::Error,
                "AppLoader",
                &format!("Failed to load gamelist: {e}"),
            );
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = r#"{
      "com.example.game": {
        "perf_lite_mode": "default",
        "dnd_on_gaming": "true",
        "app_priority": "false",
        "game_preload": "default",
        "refresh_rate": "120",
        "renderer": "skiagl",
        "resolution_downscale": "720",
        "resolution_fps": "60",
        "bypass_charging": "default"
      }
    }"#;

    #[test]
    fn parses_one_entry_with_every_key() {
        let list = parse_gamelist(ONE).expect("should parse");
        assert_eq!(list.len(), 1);
        let g = &list[0];
        assert_eq!(g.package, "com.example.game");
        assert_eq!(g.dnd_on_gaming, "true");
        assert_eq!(g.app_priority, "false");
        assert_eq!(g.refresh_rate, "120");
        assert_eq!(g.renderer, "skiagl");
        assert_eq!(g.resolution_downscale, "720");
        assert_eq!(g.resolution_fps, "60");
    }

    #[test]
    fn missing_keys_become_default() {
        let list = parse_gamelist(r#"{"com.a":{}}"#).expect("should parse");
        let g = &list[0];
        assert_eq!(g.perf_lite_mode, "default");
        assert_eq!(g.renderer, "default");
        assert_eq!(g.resolution_fps, "default");
    }

    #[test]
    fn unknown_keys_are_ignored() {
        // The repo's own seed file carries `resolution_target`, which no code
        // reads any more. It must not fail the parse.
        let list = parse_gamelist(
            r#"{"com.a":{"perf_lite_mode":"default","resolution_target":"default"}}"#,
        )
        .expect("unknown keys must not break parsing");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].resolution_downscale, "default");
    }

    #[test]
    fn keys_cannot_leak_between_entries() {
        // The C scanner bounded each key search by the next entry; this is the
        // regression test for that bounding, done properly by the parser.
        let raw = r#"{
          "com.a": { "renderer": "skiagl" },
          "com.b": { "perf_lite_mode": "default" }
        }"#;
        let list = parse_gamelist(raw).expect("should parse");
        let a = list.iter().find(|g| g.package == "com.a").unwrap();
        let b = list.iter().find(|g| g.package == "com.b").unwrap();
        assert_eq!(a.renderer, "skiagl");
        // com.b must NOT inherit com.a's renderer.
        assert_eq!(b.renderer, "default");
    }

    #[test]
    fn multiple_entries_are_all_returned() {
        let raw = r#"{"com.a":{},"com.b":{},"com.c":{}}"#;
        assert_eq!(parse_gamelist(raw).expect("should parse").len(), 3);
    }

    #[test]
    fn empty_object_is_reported_as_empty_not_as_success() {
        // The C code logged "empty or invalid" and returned an empty cache.
        assert_eq!(parse_gamelist("{}"), Err(GamelistError::Empty));
    }

    #[test]
    fn malformed_json_is_an_error_not_a_silent_empty_cache() {
        // This is the behavioural improvement over the C scanner: a truncated
        // write used to yield a partial cache with no error at all.
        assert!(matches!(
            parse_gamelist(r#"{"com.a": {"#),
            Err(GamelistError::Parse(_))
        ));
    }

    #[test]
    fn a_top_level_array_is_rejected() {
        // Not the documented shape; serde refuses rather than guessing.
        assert!(matches!(
            parse_gamelist(r#"[{"com.a":{}}]"#),
            Err(GamelistError::Parse(_))
        ));
    }

    #[test]
    fn a_non_object_entry_is_rejected() {
        assert!(matches!(
            parse_gamelist(r#"{"com.a":"default"}"#),
            Err(GamelistError::Parse(_))
        ));
    }
}
