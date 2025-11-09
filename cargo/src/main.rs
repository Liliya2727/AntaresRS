use std::fs;
use std::process::{Command, Output, exit};
use std::thread::sleep;
use std::time::Duration;
use std::os::unix::process::ExitStatusExt;
use std::fs::OpenOptions;
use std::io::Write;
use std::time;
use chrono::Local;
use nix::unistd::Pid;
use nix::fcntl;
use nix::sys::signal;
use fs2::FileExt;
use std::fs::File;
use std::sync::atomic::{AtomicI32, Ordering};
use std::io::Read;
use std::path::Path;
const LOCK_FILE: &str = "/sdcard/AZenith/.lock";
const NOTIFY_TITLE: &str = "AZenith";
const LOG_FILE: &str = "/sdcard/AZenith/debug/AZenith.log";
const GAMELIST: &str = "/sdcard/AZenith/gamelist/gamelist.txt";
static MLBB_PID: AtomicI32 = AtomicI32::new(0);
#[derive(Debug, PartialEq)]
enum MLBBState {
    NotRunning,
    Running,
    Background,
}

#[derive(PartialEq, Eq)]
enum ProfileMode {
    Perfcommon,
    Performance,
    Balanced,
    Powersave,
}
#[repr(i8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profiler {
    Perfcommon = 0,
    Performance = 1,
    Balanced = 2,
    Powersave = 3,
}

#[derive(Debug)]
enum LogLevel {
    Info,
    Warn,
    Error,
    Debug,
}
impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Info => write!(f, "I"),
            LogLevel::Warn => write!(f, "W"),
            LogLevel::Error => write!(f, "E"),
            LogLevel::Debug => write!(f, "D"),
        }
    }
}
fn timestamp() -> String {
    let now = Local::now();
    format!("{}", now.format("%Y-%m-%d %H:%M:%S%.3f"))
}

/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : notify
// Inputs             : notify
// Returns            : None
// Description        : Notify
/////////////////////////////////////////////////////////////////////////////////////////
fn notify(message: &str) {
    let cmd = format!(
        "/system/bin/cmd notification post \
        -t '{}' \
        -i file:///data/local/tmp/module_icon.png \
        -I file:///data/local/tmp/module_icon.png \
        '{}' '{}'",
        NOTIFY_TITLE, NOTIFY_TITLE, message
    );

    let status = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .status();

    if let Err(e) = status {
        eprintln!("Failed to send notification: {}", e);
    }
}

/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : apply_profile
// Inputs             : command - shell command to execute
// Returns            : run profile modes
/////////////////////////////////////////////////////////////////////////////////////////
fn apply_profile(profile: Profiler) {
    let profile_val = profile as i8;
    let _ = Command::new("sys.azenithnonr-profilesettings")
        .arg(profile_val.to_string())
        .status();
}

/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : log_zenith
// Inputs             : command - shell command to execute
// Returns            : Log daemon processes
/////////////////////////////////////////////////////////////////////////////////////////

fn log_zenith(level: LogLevel, message: &str) {
    let line = format!("{} {} AZenith: {}\n", timestamp(), level, message);

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_FILE)
    {
        if let Err(e) = file.write_all(line.as_bytes()) {
            eprintln!("Failed to write log: {}", e);
        }
    } else {
        eprintln!("Failed to open log file: {}", LOG_FILE);
    }
}
/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : execute_command
// Inputs             : command - shell command to execute
// Returns            : Pointer to the dynamically allocated output of the command
//                      variadic arguments - Additional arguments for command
/////////////////////////////////////////////////////////////////////////////////////////
fn execute_command(cmd: &str) -> Option<String> {
    if let Ok(output) = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
    {
        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }
    }
    None
}

/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : showtoast
// Inputs             : message
// Description        : Show toast if change game profiles
/////////////////////////////////////////////////////////////////////////////////////////
fn show_toast(message: &str) {
    let cmd = format!(
        "/system/bin/am start -a android.intent.action.MAIN \
         -e toasttext '{}' \
         -n bellavita.toast/.MainActivity >/dev/null 2>&1",
        message
    );

    if let Err(e) = Command::new("sh").arg("-c").arg(&cmd).status() {
        eprintln!("Failed to show toast: {}", e);
    }
}

/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : .lock
// Inputs             : none
// Returns            : none
// Description        : Checks if Another instance is running
/////////////////////////////////////////////////////////////////////////////////////////
fn acquire_lock() -> Option<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .open(LOCK_FILE)
        .ok()?;

    if file.try_lock_exclusive().is_ok() {
        Some(file)
    } else {
        None
    }
}

/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : handle_mlbb
// Inputs             : gamestart - Game package name
// Returns            : MLBBState (enum)
// Description        : Checks if Mobile Legends: Bang Bang IS actually running
//                      on foreground, not in the background.
/////////////////////////////////////////////////////////////////////////////////////////
fn handle_mlbb(gamestart: &str) -> MLBBState {
  
    if !gamestart.contains("com.mobile.legends") {
        MLBB_PID.store(0, Ordering::Relaxed);
        return MLBBState::NotRunning;
    }

    let pid = MLBB_PID.load(Ordering::Relaxed);
    if pid != 0 {
        if signal::kill(Pid::from_raw(pid), None).is_ok() {
            return MLBBState::Running;
        }
        MLBB_PID.store(0, Ordering::Relaxed);
    }

    let mlbb_proc = format!("{}:UnityKillsMe", gamestart);

    if let Some(pid_str) = execute_command(&format!("pidof {}", mlbb_proc)) {
        if let Some(first_pid) = pid_str.split_whitespace().next() {
            if let Ok(parsed) = first_pid.parse::<i32>() {
                MLBB_PID.store(parsed, Ordering::Relaxed);
                println!("[AZenith] Boosting MLBB process {}", mlbb_proc);
                return MLBBState::Running;
            }
        }
    }
    MLBBState::Background
}

/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : get_low_power_state
// Inputs             : None
/////////////////////////////////////////////////////////////////////////////////////////
fn is_low_power(val: &str) -> bool {
    val == "1" || val.eq_ignore_ascii_case("true")
}
fn get_low_power_state() -> bool {
    if let Some(low_power) = execute_command("/system/bin/settings get global low_power") {
        return is_low_power(&low_power);
    }

    if let Some(low_power) = execute_command(
        "dumpsys power | grep -Eo 'mSettingBatterySaverEnabled=true|mSettingBatterySaverEnabled=false' | awk -F'=' '{print $2}'",
    ) {
        return is_low_power(&low_power);
    }

    false
}

/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : getpid
// Returns            : pid (pid_t) - PID of process
// Description        : Fetch PID from a process name.
/////////////////////////////////////////////////////////////////////////////////////////
fn get_pid(name: &str) -> i32 {
    let mut tracked_pid: i32 = 0;

    let entries = match fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return 0,
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let file_name = entry.file_name();
        let pid_str = file_name.to_string_lossy();

        // Check if directory name is all digits (valid PID)
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        // Build /proc/<pid>/cmdline path
        let cmdline_path = format!("/proc/{}/cmdline", pid_str);

        // Read cmdline
        let mut file = match fs::File::open(&cmdline_path) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let mut cmdline = Vec::new();
        if file.read_to_end(&mut cmdline).is_err() || cmdline.is_empty() {
            continue;
        }

        // Replace NUL bytes with spaces
        for byte in &mut cmdline {
            if *byte == 0 {
                *byte = b' ';
            }
        }

        let cmdline_str = String::from_utf8_lossy(&cmdline);

        // Substring match (same as your C version)
        if cmdline_str.contains(name) {
            if let Ok(pid_val) = pid_str.parse::<i32>() {
                if tracked_pid == 0 || pid_val < tracked_pid {
                    tracked_pid = pid_val;
                }
            }
        }
    }

    tracked_pid
}

/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : get_gamestart
// Inputs             : None
// Description        : Searches for the currently visible application that matches
//                      any package name listed in gamelist.
//                      This helps identify if a specific game is running in the foreground.
//                      Uses dumpsys to retrieve visible apps and filters by packages
//                      listed in Gamelist.
/////////////////////////////////////////////////////////////////////////////////////////
fn get_gamestart() -> Option<String> {
    let cmd = format!(
        "dumpsys window visible-apps | grep 'package=.* ' | grep -Eo -f {}",
        GAMELIST
    );

    let output = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let result = String::from_utf8_lossy(&output.stdout);
    result.lines().next().map(|s| s.trim().to_string())
}

/////////////////////////////////////////////////////////////////////////////////////////
// Function Name      : MAIN FUNCTIONS
// Inputs             : None
// Description        : DAEMONIZE SERVICE
/////////////////////////////////////////////////////////////////////////////////////////
fn main() {

    // Check program to run as root
    unsafe {
        if libc::getuid() != 0 {
            eprintln!("\x1b[31mERROR:\x1b[0m Please run this program as root");
            std::process::exit(1);
        }
    }

    let _lock_file = acquire_lock().unwrap_or_else(|| {
        eprintln!("\x1b[31mERROR:\x1b[0m Another instance of the daemon is already running!");
        std::process::exit(1);
    });

    // Daemonize service
    unsafe {
        if libc::daemon(0, 0) != 0 {
            log_zenith(LogLevel::Error, "Unable to daemonize service");
            std::process::exit(1);
        }
    }

    // Initialize variables
    let mut gamestart: Option<String> = None;
    let mut game_pid: i32 = 0;
    let mut checkprofile: bool = false;
    let mut cur_mode = ProfileMode::Perfcommon;
    let mut mlbb_is_running = MLBBState::NotRunning;
    let mut notify_if_running = false;

    // Clear old logs
    execute_command("rm -f /sdcard/AZenith/debug/AZenith.log");

    // Run Daemon
    let pid = std::process::id();
    log_zenith(LogLevel::Info, &format!("Daemon Started with PID {}", pid));
    notify("Initializing...");
    cur_mode = ProfileMode::Perfcommon;
    apply_profile(Profiler::Perfcommon);

    loop {
        sleep(Duration::from_secs(5));

        // Only fetch gamestart when user not in-game
        if gamestart.is_none() {
            gamestart = get_gamestart();
        } else if game_pid != 0 && unsafe { libc::kill(game_pid, 0) } == -1 {
            log_zenith(LogLevel::Info, "Game exited, resetting profile...");

            game_pid = 0;
            gamestart = get_gamestart();
            checkprofile = true;
        }

            if let Some(ref game) = gamestart {
            mlbb_is_running = handle_mlbb(game);
        }

        if gamestart.is_some() && mlbb_is_running != MLBBState::Background {
            // Bail out if we’re already in performance profile
            if !checkprofile && cur_mode == ProfileMode::Performance {
                continue;
            }

            // Get PID and check if the game is the real running program
            // Handle weird behavior of MLBB
            game_pid = if mlbb_is_running == MLBBState::Running {
                MLBB_PID.load(Ordering::Relaxed)
            } else if let Some(ref g) = gamestart {
                get_pid(g)
            } else {
                0
            };
            if game_pid == 0 {
                log_zenith(LogLevel::Error, "Unable to fetch PID");
                gamestart = None;
                continue;
            }
            
            cur_mode = ProfileMode::Performance;
            checkprofile = false;
            let game_name = gamestart.as_deref().unwrap_or("unknown");
            log_zenith(LogLevel::Info,&format!("Applying performance profile for {}", game_name));
            show_toast("Applying Performance Profile");
            apply_profile(Profiler::Performance);
        } else if get_low_power_state() {
            // Bail out if we already on powersave profile
            if cur_mode == ProfileMode::Powersave {
                continue;
            }
            cur_mode = ProfileMode::Powersave;
            checkprofile = false;
            log_zenith(LogLevel::Info, "Applying eco mode");
            show_toast("Applying Eco Mode");
            apply_profile(Profiler::Powersave);
        } else {
            // Bail out if we already on balanced profile
            if cur_mode == ProfileMode::Balanced {
                continue;
            }
            cur_mode = ProfileMode::Balanced;
            checkprofile = false;
            log_zenith(LogLevel::Info, "Applying balanced profile");
            show_toast("Applying Balanced Profile");
            if !notify_if_running {
                notify("AZenith is running successfully");
                notify_if_running = true;
            }
            apply_profile(Profiler::Balanced);
        }
    }
}
