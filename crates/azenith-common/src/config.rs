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

//! Config file readers (freqoffset, bypass, API/current_modes, etc.).
//!
//! Mirrors the C `ConfigLoader.c` semantics exactly: strip trailing newline,
//! trim whitespace, allow comments, return `""` on missing files. The original
//! code did not validate the format — it only read the first line.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Reads the first non-empty, non-comment line from `path`. Returns `""` if
/// the file does not exist, cannot be opened, or contains no usable line.
/// Behaviour matches `ConfigLoader.c:read_config_value`.
pub fn read_line(path: &str) -> String {
    let p = Path::new(path);
    let Ok(f) = File::open(p) else {
        return String::new();
    };
    let reader = BufReader::new(f);
    for line in reader.lines().flatten() {
        let mut s = line;
        // Trim trailing CRLF (strip_newline equivalent)
        if s.ends_with('\n') {
            s.pop();
        }
        if s.ends_with('\r') {
            s.pop();
        }
        // Comments: start with `#`
        if let Some(hash) = s.find('#') {
            s.truncate(hash);
        }
        let t = s.trim();
        if t.is_empty() {
            continue;
        }
        return t.to_string();
    }
    String::new()
}

/// Reads an integer from `path`. Returns 0 if missing or malformed.
pub fn read_int(path: &str) -> i32 {
    read_line(path).parse::<i32>().unwrap_or(0)
}

/// Reads a boolean with `"1"`/`"0"` convention. Missing = false.
pub fn read_bool_1(path: &str) -> bool {
    read_line(path) == "1"
}

/// Writes a single line (no newline added) — the callers in C wrote their own
/// newlines via `fprintf` in some cases and `echo` in others; this helper is
/// intentionally minimal.
pub fn write_line(path: &str, value: &str) -> std::io::Result<()> {
    std::fs::write(Path::new(path), value.as_bytes())
}

/// Atomic write by rename (best-effort) to match the "tmp + renameTo" pattern
/// used by the Java side; the C side did not always use rename, but config
/// files are small. Not strictly required to match C byte-for-byte.
pub fn write_line_atomic(path: &str, value: &str) -> std::io::Result<()> {
    let p = Path::new(path);
    let dir = p.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp.{}",
        p.file_name().and_then(|x| x.to_str()).unwrap_or("tmp"),
        std::process::id()
    ));
    std::fs::write(&tmp, value.as_bytes())?;
    std::fs::rename(&tmp, p)?;
    Ok(())
}
