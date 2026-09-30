//
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
//

// Thin runner: all logic lives in the `azenith_utilityconf` lib so the unified
// sys.azenith-service can call `run()` directly (plan Q1, Option A) instead of
// fork+exec'ing this binary. The `_` passthrough arm still shells out for an
// unmatched argv[1]; the unified binary drops that.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::ExitCode::from(azenith_utilityconf::dispatch(&args) as u8)
}
