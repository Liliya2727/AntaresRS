#[cfg(target_os = "android")]
use std::ffi::CStr;
use std::ffi::CString;
use super::constants::PROP_AUDIT_LOGS_ENABLED;

// ============================================================================
// FFI BINDINGS - Android System Properties and Logging
// ============================================================================
//
// These are gated to `target_os = "android"` on purpose. `#[link]` is a
// crate-wide attribute, so leaving it unconditional made every *host* build of
// this crate fail at link time with "unable to find library -llog" as soon as
// the crate joined the workspace. On the host the property reads fall back to
// the `getprop` binary and log output is dropped; on Android both are native.

pub const ANDROID_LOG_DEBUG: c_int = 3;
pub const ANDROID_LOG_INFO:  c_int = 4;
pub const ANDROID_LOG_WARN:  c_int = 5;
pub const ANDROID_LOG_ERROR: c_int = 6;

use std::os::raw::c_int;
#[cfg(target_os = "android")]
use std::os::raw::{ c_char, c_uchar };

#[cfg(target_os = "android")]
#[link(name = "log")]
unsafe extern "C" {
    pub fn __android_log_print(prio: c_int, tag: *const c_char, fmt: *const c_char, ...) -> c_int;
}

#[cfg(target_os = "android")]
#[link(name = "c")]
unsafe extern "C" {
    pub fn __system_property_get(name: *const c_uchar, value: *mut c_uchar) -> c_int;
}

// ============================================================================
// LOGGING UTILITIES
// ============================================================================

/// The one property reader in this crate.
///
/// `utils::get_system_property` used to carry its own copy of this FFI call.
/// Two copies meant two places to keep the buffer size and the empty-value
/// rule in sync; now there is one, and both callers go through it.
///
/// Returns `None` when the property is unset or empty, which is what the
/// bionic API reports as a zero-length read.
pub fn read_property(key: &str) -> Option<String> {
    #[cfg(not(target_os = "android"))]
    {
        let out = std::process::Command::new("getprop")
            .arg(key)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        return if out.is_empty() { None } else { Some(out) };
    }

    #[cfg(target_os = "android")]
    {
        use std::os::raw::{ c_char, c_uchar };
        let prop_name = CString::new(key).ok()?;
        let mut value = [0u8; PROP_VALUE_MAX];

        let len = unsafe {
            __system_property_get(
                prop_name.as_ptr() as *const c_uchar,
                value.as_mut_ptr() as *mut c_uchar,
            )
        };
        if len <= 0 {
            return None;
        }
        // SAFETY: `__system_property_get` wrote at most `PROP_VALUE_MAX`
        // bytes into `value` and is NUL-terminated, so the scan stops at the
        // first zero — which is always in bounds.
        let s = unsafe { CStr::from_ptr(value.as_ptr() as *const c_char) }
            .to_string_lossy()
            .into_owned();
        if s.is_empty() { None } else { Some(s) }
    }
}

/// bionic's PROP_VALUE_MAX. A read of any greater length is truncated, not
/// overflowed, but reading into a larger buffer would lie about the API.
#[cfg(target_os = "android")]
const PROP_VALUE_MAX: usize = 92;


pub struct Logger {
    pub tag: CString,
    pub debug_enabled: bool,
    pub audit_logs_enabled: bool,
}

impl Logger {
    pub fn new() -> Self {
        let tag = CString::new("RianixiaThermalCore").unwrap();
        let debug_enabled = Self::check_bool_property(
            "persist.sys.rianixia.thermalcore-debug",
            false
        );
        let audit_logs_enabled = Self::check_bool_property(PROP_AUDIT_LOGS_ENABLED, false);
        Logger { tag, debug_enabled, audit_logs_enabled }
    }

    pub fn check_bool_property(prop_name_str: &str, default: bool) -> bool {
        match read_property(prop_name_str).as_deref() {
            Some("true") | Some("1") => true,
            Some("false") | Some("0") => false,
            _ => default,
        }
    }

    pub fn log(&self, level: c_int, msg: &str) {
        if !self.debug_enabled && level == ANDROID_LOG_DEBUG {
            return;
        }

        let c_msg = CString::new(msg).unwrap_or_else(|_|
            CString::new("invalid log message").unwrap()
        );
        // `%` in the message is data, not a conversion spec. It used to be
        // passed as the format string, so any log line containing a percent
        // sign read out of bounds and printed garbage — a live bug, not a
        // theoretical one, since these messages carry temperatures and paths.
        let fmt = CString::new("%s").unwrap();
        #[cfg(target_os = "android")]
        unsafe {
            __android_log_print(level, self.tag.as_ptr(), fmt.as_ptr(), c_msg.as_ptr());
        }
        #[cfg(not(target_os = "android"))]
        let _ = (&fmt, &c_msg, level);
    }

    pub fn debug(&self, msg: &str) {
        self.log(ANDROID_LOG_DEBUG, msg);
    }
    pub fn info(&self, msg: &str) {
        self.log(ANDROID_LOG_INFO, msg);
    }
    pub fn warn(&self, msg: &str) {
        self.log(ANDROID_LOG_WARN, msg);
    }
    pub fn error(&self, msg: &str) {
        self.log(ANDROID_LOG_ERROR, msg);
    }
}
