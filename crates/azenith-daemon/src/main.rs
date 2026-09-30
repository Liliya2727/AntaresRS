// Copyright (C) 2025-2026 Zexshia
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

//! Binary entry point. CLI dispatch replaces the C `Main.c`.

use std::process::ExitCode;

fn main() -> ExitCode {
    // Root gate first, as in `Main.c:24-27`: every subcommand writes to
    // `/data/adb` or sysfs, and failing early avoids a confusing per-command
    // error instead.
    // SAFETY: `getuid` is always safe and cannot fail.
    if unsafe { libc::getuid() } != 0 {
        eprintln!("\x1b[31mERROR:\x1b[0m Please run this program as root");
        return ExitCode::FAILURE;
    }

    // `bin` is the invoked name, which is how a symlinked
    // `sys.azenith-utilityconf` still reaches that crate's dispatch.
    let mut argv = std::env::args();
    let bin = argv.next().unwrap_or_default();
    let args: Vec<String> = argv.collect();

    ExitCode::from(azenith_daemon::cli::run(&bin, &args) as u8)
}
