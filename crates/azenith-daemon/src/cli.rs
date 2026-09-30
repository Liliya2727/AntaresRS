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

//! Command dispatch. Replaces `Main.c` and `BinaryCLI/BinaryCLI.c`.
//!
//! Two dispatch layers, in this order:
//!
//! 1. the *flags* below, which are what the Manager and the shell scripts call;
//! 2. *subcommand names* — `daemon`, `profiles`, `utils`, `thermal`, `prefs` —
//!    which is how the unified binary absorbs the four existing Rust crates
//!    (Q1). A symlink named `sys.azenith-utilityconf` arriving as `argv[0]`
//!    selects that crate's dispatch, so existing callers keep working unchanged.
//!
//! The daemon-running gate is preserved exactly: nine flags work with the
//! daemon stopped, and only `--profile`/`--log`/`--verboselog`/`--checkbypasschg`
//! require it up.

use azenith_common::logger::{Level, log};
use azenith_common::paths;
use azenith_common::shell;
use azenith_common::version::MODULE_VERSION;

use crate::daemon::context::ProfileMode;

/// Long/short flag match, mirroring the C `IS_CMD` macro.
fn is_cmd(arg: &str, long: &str, short: &str) -> bool {
    arg == long || arg == short
}

const HELP: &str = "\
Usage: sys.azenith-service [options]

  --appactivity, -actv         Launch the Manager app
  --run, -r                    Run the daemon in the foreground
  --version, -V                Print the module version
  --clearlogs, -c              Truncate the debug logs
  --hidenotifications, -hn     Hide the profile toast
  --shownotifications, -sn     Show the profile toast
  --bypasspathlist, -bpl       List the detected charging nodes
  --rerun, -rr                 Restart the running daemon
  --profile, -p <mode>         Switch profile (0-3, or a PROFILE name)
  --log, -l <TAG> <level> <msg>
  --verboselog, -vl <TAG> <level> <msg>
  --checkbypasschg, -cbc       Report charging-bypass compatibility

Subcommands (unified binary, Q1):
  daemon                      Same as --run
  profiles <0-3>              Apply a profile
  profiles applyfreqbalance   Rebalance cluster frequency offsets
  utils <subcommand>          sys.azenith-utilityconf
  thermal                     rianixia-thermalcore
  prefs                       azenith-preferencedtweaks
  preload                     sys.azenith-preloadbin
";

fn print_help() {
    print!("{HELP}");
}

fn print_version() {
    println!("AZenith {MODULE_VERSION}");
}

/// The crate a symlinked `argv[0]` selects, and the subcommand routing when the
/// unified binary is called by its own name.
fn crate_for<'a>(binary: &str, sub: Option<&'a str>) -> Option<&'a str> {
    match binary {
        "sys.azenith-utilityconf" => Some("utils"),
        "sys.azenith-profilesettings" => Some("profiles"),
        "sys.azenith-rianixiathermalcore" => Some("thermal"),
        "sys.azenith-preferencedtweaks" => Some("prefs"),
        "sys.azenith-preloadbin" => Some("preload"),
        _ => sub,
    }
}

/// Runs the CLI, returning the process exit code.
///
/// `argv` excludes `argv[0]`'s program name in `args`, but the *name itself* is
/// taken from `bin` so symlink dispatch works.
pub fn run(bin: &str, args: &[String]) -> i32 {
    if args.is_empty() || is_cmd(&args[0], "--help", "-h") {
        print_help();
        return 0;
    }
    let cmd = args[0].as_str();

    // Symlink dispatch first: a call through `sys.azenith-utilityconf` must reach
    // that crate even if a flag name would otherwise match.
    if bin != "sys.azenith-service"
        && let Some(known) = crate_for(bin, None)
    {
        return dispatch_crate(known, args);
    }

    match cmd {
        c if is_cmd(c, "--appactivity", "-actv") => {
            let _ = shell::systemv("am start -n zx.azenith/.MainActivity");
            0
        }
        c if is_cmd(c, "--run", "-r") => {
            crate::daemon::startup::run();
            0
        }
        c if is_cmd(c, "--version", "-V") => {
            print_version();
            0
        }
        c if is_cmd(c, "--clearlogs", "-c") => {
            let _ = std::fs::write(paths::LOG_FILE, "");
            let _ = std::fs::write(paths::LOG_VFILE, "");
            0
        }
        c if is_cmd(c, "--hidenotifications", "-hn") => {
            azenith_common::android_props::setprop(
                "persist.sys.azenith.profilenotifications",
                "false",
            );
            0
        }
        c if is_cmd(c, "--shownotifications", "-sn") => {
            azenith_common::android_props::setprop(
                "persist.sys.azenith.profilenotifications",
                "true",
            );
            0
        }
        c if is_cmd(c, "--bypasspathlist", "-bpl") => {
            for n in crate::bypass_charge::all_nodes() {
                println!("{}  {}", n.name, n.path);
            }
            0
        }
        c if is_cmd(c, "--rerun", "-rr") => {
            let _ = shell::systemv("sys.azenith-utilityconf restartservice");
            0
        }
        _ => run_gated(cmd, args),
    }
}

/// The four commands that need a live daemon.
fn run_gated(cmd: &str, args: &[String]) -> i32 {
    if !require_daemon_running() {
        return 1;
    }
    match cmd {
        c if is_cmd(c, "--profile", "-p") => handle_profile(args),
        c if is_cmd(c, "--log", "-l") => handle_log(args),
        c if is_cmd(c, "--verboselog", "-vl") => handle_verboselog(args),
        c if is_cmd(c, "--checkbypasschg", "-cbc") => {
            crate::bypass_charge::report_compatibility();
            0
        }
        "daemon" => {
            crate::daemon::startup::run();
            0
        }
        "profiles" => handle_profile(&args[1..]),
        "utils" | "thermal" | "prefs" | "preload" => dispatch_crate(cmd, &args[1..]),
        _ => {
            eprintln!("\x1b[31mERROR:\x1b[0m Unknown command: {cmd}");
            1
        }
    }
}

/// Routes a subcommand to the module that owns it.
///
/// Direct call, not fork+exec: this is the whole point of plan Q1 (Option A).
/// The C daemon had to spawn `sys.azenith-utilityconf` and friends because it
/// could not call them; now every one of those crates is a library linked into
/// this binary, so the round trip is a function call.
fn dispatch_crate(which: &str, args: &[String]) -> i32 {
    match which {
        "utils" => azenith_utilityconf::dispatch(args),
        "profiles" => azenith_profilesettings::run(args),
        "prefs" => azenith_preferencedtweaks::run(args),
        "thermal" => rianixia_thermalcore::run(args),
        // Still a separate process, by design (plan Q3): it `dlopen`s arbitrary
        // game `.so` files and must not pollute the daemon's address space.
        "preload" => {
            let mut cmd = "sys.azenith-preloadbin".to_string();
            for a in args {
                cmd.push(' ');
                cmd.push_str(&shell::escape(a));
            }
            shell::systemv(&cmd)
        }
        _ => 1,
    }
}

fn handle_profile(args: &[String]) -> i32 {
    let Some(arg) = args.get(1) else {
        eprintln!("ERROR: --profile needs a mode");
        return 1;
    };
    let mode = match arg.parse::<u8>() {
        Ok(n) => ProfileMode::from_index(n),
        // The C also accepted the long enum names.
        Err(_) => ProfileMode::from_name(arg),
    };
    let Some(mode) = mode else {
        eprintln!("\x1b[31mERROR:\x1b[0m Invalid profile: {arg}");
        return 1;
    };

    // A profile switch is a request, not an action: the running daemon picks it
    // up on its next poll so it happens on the daemon's thread, not this one.
    let body = format!("profile {}\n", mode.index());
    if let Err(e) = std::fs::write(paths::GAME_INFO, body) {
        eprintln!("ERROR: cannot write {}: {e}", paths::GAME_INFO);
        return 1;
    }
    0
}

fn handle_log(args: &[String]) -> i32 {
    write_log(args, Level::Info)
}

fn handle_verboselog(args: &[String]) -> i32 {
    write_log(args, Level::Debug)
}

/// `<TAG> <LEVEL> <MESSAGE...>` — the shape `binprofiles` and `binpreferenced`
/// already shell out with.
fn write_log(args: &[String], default_level: Level) -> i32 {
    let (Some(tag), Some(rest)) = (args.get(1), args.get(2)) else {
        eprintln!("ERROR: log needs <TAG> <LEVEL> <MESSAGE>");
        return 1;
    };
    let level = rest.parse::<Level>().unwrap_or(default_level);
    let msg = args[3..].join(" ");
    log(level, tag, &msg);
    0
}

/// Reports whether the daemon holds its lock, the same way the C did: try to
/// take the lock non-blocking, and if that succeeds, nobody holds it.
///
/// A `flock` probe is the only race-free check — the lock file carries no pid,
/// and reading one would be wrong the moment the daemon restarts.
fn require_daemon_running() -> bool {
    if crate::daemon::inotify::lock_is_free(paths::LOCK_FILE) {
        eprintln!("\x1b[31mERROR:\x1b[0m AZenith daemon is not running.");
        eprintln!("Run: sys.azenith-service --run");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_cmd_matches_long_and_short() {
        assert!(is_cmd("--run", "--run", "-r"));
        assert!(is_cmd("-r", "--run", "-r"));
        assert!(!is_cmd("-run", "--run", "-r"));
        assert!(!is_cmd("--running", "--run", "-r"));
    }

    #[test]
    fn symlink_names_select_their_crate() {
        assert_eq!(crate_for("sys.azenith-utilityconf", None), Some("utils"));
        assert_eq!(
            crate_for("sys.azenith-profilesettings", None),
            Some("profiles")
        );
        assert_eq!(
            crate_for("sys.azenith-rianixiathermalcore", None),
            Some("thermal")
        );
        assert_eq!(crate_for("sys.azenith-service", None), None);
    }

    #[test]
    fn explicit_subcommand_wins_for_the_real_name() {
        assert_eq!(
            crate_for("sys.azenith-service", Some("utils")),
            Some("utils")
        );
    }

    #[test]
    fn help_and_no_args_succeed() {
        assert_eq!(run("sys.azenith-service", &[]), 0);
        assert_eq!(run("sys.azenith-service", &["--help".into()]), 0);
    }

    #[test]
    fn an_unknown_command_fails() {
        // Without a daemon up, an unknown command still reports the daemon gate
        // first, exactly as the C did — the gate precedes the unknown-command
        // error.
        let code = run("sys.azenith-service", &["--nonsense".into()]);
        assert_eq!(code, 1);
    }
}
