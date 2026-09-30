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

//! Readers for the flat config files under `/data/adb/.config/AZenith/`.
//!
//! Each file holds one scalar, newline-terminated. The C daemon read them with
//! `fgets` + `trim_newline` and treated a failed open as "keep the previous
//! value" — so a missing or momentarily-truncated file must return the default
//! without disturbing the caller.

/// Reads one line, trimmed of trailing whitespace, or `""` if unreadable.
pub fn read_line(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Reads one line as an integer, or `fallback` if missing or unparseable.
pub fn read_int_or(path: &str, fallback: i32) -> i32 {
    read_line(path).parse().unwrap_or(fallback)
}

/// Writes one scalar back, newline-terminated, matching what the Manager expects.
///
/// A missing parent directory is not an error here: the daemon writes into
/// directories `customize.sh` created at install, and a failure should be
/// visible in the log rather than silently ignored.
pub fn write_line(path: &str, value: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{value}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        // Under the project dir, not `std::env::temp_dir()`: /tmp is a
        // RAM-backed tmpfs on this host.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch")
            .to_path_buf();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let p = dir.join(format!("config-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn trailing_newline_is_stripped() {
        let p = scratch("trim");
        std::fs::write(&p, "  900 \n").unwrap();
        assert_eq!(read_line(p.to_str().unwrap()), "900");
    }

    #[test]
    fn a_missing_file_reads_as_empty_not_an_error() {
        // The C kept the previous value on a failed open; returning "" lets the
        // caller distinguish "absent" from "changed to empty".
        assert_eq!(read_line("/nonexistent/azenith/freqoffset"), "");
    }

    #[test]
    fn int_read_falls_back_instead_of_zeroing() {
        let p = scratch("int");
        std::fs::write(&p, "not-a-number\n").unwrap();
        let path = p.to_str().unwrap();
        assert_eq!(read_int_or(path, -1), -1);
        std::fs::write(&p, "42\n").unwrap();
        assert_eq!(read_int_or(path, -1), 42);
    }

    #[test]
    fn write_line_round_trips_with_a_newline() {
        let p = scratch("write");
        let path = p.to_str().unwrap();
        write_line(path, "7").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "7\n");
    }
}
