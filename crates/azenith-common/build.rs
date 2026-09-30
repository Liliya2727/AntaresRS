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

//! Stamps `AZENITH_MODULE_VERSION` into the crate from `AZENITH_VERSION`.
//!
//! Kept to a single env read and a single `cargo:rustc-env` — a build script
//! that touches the filesystem would make the build order-dependent.

fn main() {
    let version = std::env::var("AZENITH_VERSION").unwrap_or_else(|_| ".placeholder".to_string());
    println!("cargo:rustc-env=AZENITH_MODULE_VERSION={version}");
    println!("cargo:rerun-if-env-changed=AZENITH_VERSION");
}
