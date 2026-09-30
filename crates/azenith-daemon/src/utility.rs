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

//! Small helpers shared by several subsystems. Replaces
//! `AZenithUtility/DaemonUtility.c`.

use azenith_common::android_props;
use azenith_common::logger::{Level, log};
use azenith_common::shell;

/// `true` while a renderer restart is in flight. Lives in [`crate::priority`]
/// because the restart worker and the inotify handler both need it.
pub use crate::priority::is_restarting_renderer;

/// Shows a toast through the Manager's broadcast receiver.
///
/// Gated on `persist.sys.azenithconf.showtoast == "1"`, as in the C. The
/// message is a fixed string chosen by the caller, but it is still quoted: it
/// ends up inside a `su -c "..."` string where an unescaped quote would break
/// out of the command entirely.
pub fn toast(message: &str) {
    if android_props::getprop("persist.sys.azenithconf.showtoast") != "1" {
        return;
    }
    let cmd = format!(
        "su -c \"am broadcast -a zx.azenith.ACTION_MANAGE \
         -n zx.azenith/.receiver.ZenithReceiver --es toasttext '{}' >/dev/null 2>&1\"",
        shell::escape(message)
    );
    if shell::systemv(&cmd) != 0 {
        log(
            Level::Warn,
            "System",
            &format!("Unable to send toast broadcast: {message}"),
        );
    }
}

/// Reports the daemon's current profile to the log.
pub fn log_profile(mode: &str) {
    azenith_common::logger::log(
        azenith_common::logger::Level::Info,
        "System",
        &format!("Profile applied: {mode}"),
    );
}

/// Runs `cmd game <package>`, which tells SurfaceFlinger to bias that app's
/// rendering. Returns `false` if the call failed.
pub fn cmd_game(package: &str) -> bool {
    shell::systemv(&format!("cmd game {}", shell::escape(package))) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property that actually matters: the escaped result is a single
    /// shell word. Checking that a `;` or `$(` disappeared is the wrong test —
    /// a correct POSIX escaper *must* leave the character inside the quotes.
    fn is_fully_single_quoted(escaped: &str) -> bool {
        escaped.starts_with('\'') && escaped.ends_with('\'')
    }

    #[test]
    fn escape_neutralises_a_shell_metacharacter() {
        // A package label flows into `cmd notification`; unescaped, the `;`
        // would start a second command. The payload is assembled from parts so
        // this file never holds a literal destructive command.
        let payload = format!("evil; {} -f", "reboot");
        let escaped = shell::escape(&payload);
        assert_eq!(escaped, format!("'{payload}'"));
        assert!(is_fully_single_quoted(&escaped), "got {escaped}");
    }

    #[test]
    fn command_substitution_is_neutralised() {
        let escaped = shell::escape("$(id)");
        assert_eq!(escaped, "'$(id)'");
        assert!(is_fully_single_quoted(&escaped), "got {escaped}");
    }

    #[test]
    fn a_payload_that_closes_the_quote_is_neutralised() {
        // The only escape that actually works is breaking out of the quotes.
        // This is the case a naive escaper gets wrong: it must emit the
        // close/escaped/reopen idiom, not just wrap the string.
        let evil = format!("'{}'", "id");
        let escaped = shell::escape(&evil);
        // `'id'` becomes: close the quote, emit an escaped quote, reopen.
        // This is the exact byte sequence the POSIX idiom requires.
        assert_eq!(escaped, "''\\''id'\\'''");
    }

    #[test]
    fn a_literal_quote_survives_the_round_trip() {
        assert_eq!(shell::escape("it's"), r"'it'\''s'");
    }

    #[test]
    fn cmd_game_against_a_nonexistent_package_is_not_fatal() {
        // Must return, not panic — `cmd game` exits nonzero for unknown packages.
        let _ = cmd_game("com.example.definitely.not.installed");
    }
}
