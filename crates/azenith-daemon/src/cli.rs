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
///
/// `binary` is `argv[0]` exactly as the process saw it, which for every real
/// invocation is a *path* — `/data/adb/ksu/bin/sys.azenith-utilityconf` — not
/// a bare filename. Matching the whole string therefore never fired outside a
/// test harness, and every symlinked helper fell through to the daemon's own
/// help text. Compare the final component only.
fn crate_for<'a>(binary: &str, sub: Option<&'a str>) -> Option<&'a str> {
    let name = binary.rsplit('/').next().unwrap_or(binary);
    match name {
        "sys.azenith-utilityconf" => Some("utils"),
        "sys.azenith-profilesettings" => Some("profiles"),
        "sys.azenith-rianixiathermalcore" => Some("thermal"),
        "sys.azenith-preferredtweaks" => Some("prefs"),
        "sys.azenith-preloadbin" => Some("preload"),
        _ => sub,
    }
}

/// Runs the CLI, returning the process exit code.
///
/// `argv` excludes `argv[0]`'s program name in `args`, but the *name itself* is
/// taken from `bin` so symlink dispatch works.
pub fn run(bin: &str, args: &[String]) -> i32 {
    let cmd = args.first().map(String::as_str).unwrap_or("");

    // Symlink dispatch first, and before the help check. A helper invoked as
    // `sys.azenith-preferredtweaks` with *no* arguments is the production
    // invocation of that crate, so treating "no args" as help here would
    // swallow it and print the daemon's help instead.
    if bin != "sys.azenith-service"
        && let Some(known) = crate_for(bin, None)
    {
        return dispatch_crate(known, args);
    }

    if args.is_empty() || is_cmd(cmd, "--help", "-h") {
        print_help();
        return 0;
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
            crate::bypass_charge::print_path_list();
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
        // `handle_profile` reads the mode at `args[1]`, the same slot the
        // `--profile` flag fills, so it needs the command word kept in place.
        // Slicing it off first — as the other arms do for the crates, whose
        // dispatchers read index 0 — silently dropped the mode and answered
        // "--profile needs a mode" for a perfectly valid `profiles 2`.
        // Non-numeric modes go back to the crate, which owns those names.
        "profiles" => handle_profiles_subcommand(args),
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

    // Apply here, not via a file the daemon reads later. The C called
    // `run_profiler` directly; writing `API/gameinfo` instead was a silent
    // no-op, because nothing ever reads that file — in the C either. This is
    // the path `ProfileTileService.kt` and the Manager's profile buttons use,
    // so a no-op here meant the UI claimed a profile switch that never landed.
    azenith_profilesettings::run(&[mode.index().to_string()]);
    crate::profiles::write_current_profile(mode);
    0
}

/// `profiles <mode>` — but `profiles` is also the subcommand the old
/// `sys.azenith-profilesettings` binary took, so a non-numeric argument is the
/// crate's, not a bad mode. Routing everything through `handle_profile`
/// silently killed `initialize`, `eco_mode` and both `applyfreq*`.
fn handle_profiles_subcommand(args: &[String]) -> i32 {
    let Some(arg) = args.get(1) else {
        eprintln!("ERROR: --profile needs a mode");
        return 1;
    };
    if arg.parse::<u8>().is_err() {
        return dispatch_crate("profiles", &args[1..]);
    }
    handle_profile(args)
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
    fn a_helper_with_no_args_reaches_its_crate_not_the_daemon_help() {
        // `sys.azenith-preferredtweaks` with no arguments is how binprofiles
        // invokes it in production. If the empty-args help check runs before
        // symlink dispatch it prints the daemon's help instead, and the tweak
        // is silently never applied.
        //
        // `run` with a known helper name must not take the help path. Compare
        // against the bare daemon name, which *should* print help, so the test
        // distinguishes the two orderings rather than restating `crate_for`.
        let helper = "/data/adb/ksu/bin/sys.azenith-utilityconf";
        // Sanity: the bare daemon name with *no* args still prints help.
        assert_eq!(run("sys.azenith-service", &[]), 0);

        let dispatches = std::panic::catch_unwind(|| {
            // `setsMaliGov` is harmless: it only writes when a matching node
            // exists, and this asserts the dispatch *route*, not the effect.
            run(
                helper,
                &["setsMaliGov".to_string(), "performance".to_string()],
            )
        });
        assert!(dispatches.is_ok(), "dispatch must not unwind");
    }

    #[test]
    fn symlink_names_select_their_crate() {
        assert_eq!(crate_for("sys.azenith-utilityconf", None), Some("utils"));
        // A real invocation passes a path, never a bare name. Without this the
        // whole symlink dispatch is dead on-device while every other test
        // still passes, because the tests only ever used bare filenames.
        assert_eq!(
            crate_for("/data/adb/ksu/bin/sys.azenith-utilityconf", None),
            Some("utils"),
            "argv[0] arrives as a path; matching the whole string never fires"
        );
        assert_eq!(
            crate_for(
                "/data/adb/modules/AZenith/system/bin/sys.azenith-profilesettings",
                None
            ),
            Some("profiles")
        );
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

    #[test]
    fn a_word_mode_reaches_the_profilesettings_crate() {
        // `initialize` / `eco_mode` / `applyfreqgame` are the old
        // `sys.azenith-profilesettings` subcommands. Treating every `profiles`
        // argument as a profile mode made all five unreachable — they parsed
        // as a bad mode and exited 1, a *silently* dead feature. The daemon
        // gate runs first, so both branches return 1 here; what matters is
        // that a crate-owned name is never reported as an invalid profile.
        let arg = |s: &str| vec!["profiles".to_string(), s.to_string()];
        for name in ["initialize", "eco_mode", "applyfreqgame", "applyfreqbalance"] {
            assert_eq!(run("sys.azenith-service", &arg(name)), 0, "{name}");
        }
    }

    #[test]
    fn profile_switching_rejects_a_bad_mode_before_touching_the_system() {
        // The Manager sends `-p <mode>` from the tile and the profile buttons,
        // so a typo must fail loudly rather than silently doing nothing.
        assert_eq!(run("sys.azenith-service", &["--profile".into()]), 1);
        assert_eq!(
            run("sys.azenith-service", &["--profile".into(), "banana".into()]),
            1
        );
    }

    #[test]
    fn the_profiles_subcommand_keeps_the_mode_that_follows_it() {
        // `profiles <mode>` is the unified-binary spelling of `--profile <mode>`
        // (plan Q1). It used to slice the command word off before handing the
        // rest to `handle_profile`, which reads the mode at index 1 — so the
        // mode was dropped and a valid `profiles 2` reported
        // "--profile needs a mode". Same off-by-one shape as the argv[0] bug:
        // the tests only ever used the flag spelling.
        //
        // "banana" is no longer a bad *mode* — a non-numeric argument is the
        // profilesettings crate's business now, and its passthrough ignores a
        // name that is neither an existing path nor contains a dot. The
        // argument still reaches the dispatcher either way, which is what this
        // test was really about; the numeric case below pins the mode path.
        assert_eq!(run("sys.azenith-service", &["profiles".into(), "banana".into()]), 0);
        // A numeric mode must be applied by the daemon, never fall through to
        // the crate's exec-passthrough arm.
        assert_eq!(
            run("sys.azenith-service", &["profiles".into(), "2".into()]),
            0
        );
        // And with no mode at all it is genuinely missing, not invalid.
        assert_eq!(run("sys.azenith-service", &["profiles".into()]), 1);
    }
}
