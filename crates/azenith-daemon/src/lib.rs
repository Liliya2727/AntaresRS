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

//! AZenith service daemon.
//!
//! Replaces the C daemon (`archdaemon/`). Entry point and CLI live in `main`;
//! the event loop, its state, and the modules it drives are under `daemon`,
//! `app_loader`, and friends.

// Test modules build a `Daemon` by mutating one field at a time, which is the
// readability you want when the point is "given this one flag, the poll timeout
// should be X". The lint's suggested struct-literal rewrite would bury the one
// field that matters.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

pub mod app_loader;
pub mod bypass_charge;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod handlers;
pub mod integrity;
pub mod pid_tracker;
mod preload;
pub mod priority;
pub mod profiles;
pub mod shutdown;
pub mod utility;
