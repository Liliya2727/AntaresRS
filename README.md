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
| `binder.log`, `debug` | log file and debug-level flag |

Profile switching:

- a gamelist app in the foreground -> **1 performance**
- no game and battery saver on -> **3 eco**
- otherwise -> **2 balanced**
- screen off while a game is running keeps performance for 10 seconds (grace period), then drops

Each change writes the API files and runs `sys.azenith-profilesettings <mode>`.
