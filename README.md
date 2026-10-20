Antares RS is a standalone implementation of AZenith, rewritten in Rust with a Binder Monitor approach.

Developed for RomIntegratedAZenith

## ArchDaemon (binder)

Profile logic lives in `archdaemon/` and mirrors the AZenith daemon, but reads app state
straight from Binder (IProcessObserver) instead of an app-companion status file.

Config root: `/data/adb/.config/AntaresRS/`

| Path | Description |
| --- | --- |
| `gamelist/gamelist.txt` | game list, one package name per line (`#` comments allowed) |
| `API/current_profile` | current profile mode (`1` performance, `2` balanced, `3` eco) |
| `API/gameinfo` | `<pkg> <pid> <uid>` + `Time: HH:MM:SS`, or `NULL 0 0` |
| `active_app`, `cur_prof`, `state.toml` | active package, profile mode, full daemon state |
| `AZenith.log`, `AZenithVerbose.log`, `debug` | shared log files and debug-level flag |

Profile switching:

- a gamelist app in the foreground -> **1 performance**
- no game and battery saver on -> **3 eco**
- otherwise -> **2 balanced**
- screen off while a game is running keeps performance for 10 seconds (grace period), then drops

Each change writes the API files and runs the profiler in-process:
`sys.azenith-service profilesettings <mode>`.

## Single binary

`sys.azenith-service` is the only executable the module ships. The companion
binaries that used to sit next to it are subcommands now, and each one still
runs in its own process (the daemon re-execs `/proc/self/exe`, so there is no
`PATH` lookup and no module path baked in):

| Was | Now | Notes |
| --- | --- | --- |
| `sys.azenith-profilesettings <mode>` | `sys.azenith-service profilesettings <mode>` | `0/initialize`, `1/performance_profile`, `2/balanced_profile`, `3/eco_mode`, `applyfreqbalance`, `applyfreqgame` |
| `sys.azenith-utilityconf <fn>` | `sys.azenith-service utilityconf <fn>` | `setsgov`, `setsIO`, `setsMaliGov`, `setthermalcore`, `checkmalipath`, `FSTrim`, `enableDND`, `disableDND`, `setrefreshrates`, `restartservice`, `setrender` |
| `sys.azenith-preferencedtweaks [apply]` | `sys.azenith-service preferencedtweaks [apply]` | the full preference set runs on `apply`, `prefs`, `preferenced` or no action |
| `sys.azenith-preloadbin -v -t -m <size> <path>` | `sys.azenith-service preload -v -t -m <size> <path>` | Rust rewrite of the vmtouch subset, output shape unchanged |

The sources live in `archdaemon/src/cmd/`: `settings/` (the former
`binprofiles` crate), `utility/` (former `binutils`), `prefs/` (former
`binpreferenced`) and `preload/` (former `preloadbin`, `-t -v -m` only). The
daemon itself is `archdaemon/src/daemon/`, and everything shared (log writer,
property/fs/shell helpers) is `archdaemon/src/common/`.

### Log tags

Every line names its owner with a dotted component, so one file can be read
without guessing which part of the service wrote it:

```text
2026-10-21 00:25:08.582 INFO [cmd.utility.renderer] resetting renderers to default
2026-10-21 00:25:08.519 ERROR [common.fs] set_permissions failed path=/cpu/cpu0/cpufreq/scaling_governor
```

`daemon.*` is the binder engine, `cmd.<command>.<subsystem>` a companion command
(`cmd.settings.cpu`, `cmd.prefs.walt`, `cmd.preload`, ...), and `common.*` the
shared helpers. The daemon and every subcommand write through the one writer in
`archdaemon/src/common/log.rs`.
