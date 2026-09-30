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

//! Android system properties over direct FFI.
//!
//! The old crates spawned `getprop`/`setprop` once per read — a fork+exec per
//! property, dozens per profile apply. `__system_property_*` is a stable bionic
//! symbol and needs no child process at all.
//!
//! Signature source of truth is the NDK header, not `libc` and not the plan:
//! `__system_property_get` is **deprecated** and takes only two arguments, and
//! `__system_property_read_callback` is the supported reader. Every signature
//! below was read out of
//! `sysroot/usr/include/sys/system_properties.h` before being written.

#[cfg(target_os = "android")]
use std::ffi::{CStr, CString};

#[cfg(target_os = "android")]
mod ffi {
    //! bionic's property and logcat entry points.
    //!
    //! Declared here rather than taken from `libc`: `libc` only exposes
    //! `__system_property_get`/`_set` under `target_os = "android"` (so the
    //! crate cannot be host-tested with them), and it does **not** expose
    //! `__system_log_write` or `__system_property_read_callback` at all.

    use std::os::raw::{c_char, c_int, c_void};

    pub type PropCallback = unsafe extern "C" fn(
        cookie: *mut c_void,
        name: *const c_char,
        value: *const c_char,
        serial: u32,
    );

    unsafe extern "C" {
        pub fn __system_property_set(name: *const c_char, value: *const c_char) -> c_int;
        pub fn __system_property_find(name: *const c_char) -> *const c_void;
        pub fn __system_property_read_callback(
            pi: *const c_void,
            callback: PropCallback,
            cookie: *mut c_void,
        );
        pub fn __system_property_foreach(
            callback: unsafe extern "C" fn(pi: *const c_void, cookie: *mut c_void),
            cookie: *mut c_void,
        ) -> c_int;

        pub fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }
}

/// Writes one line to logcat.
///
/// Kept next to the other bionic bindings so there is exactly one `extern` block
/// in the crate. `text` is a plain string — bionic never applies `printf`
/// semantics to it, which is the whole reason this wrapper exists: the old
/// `thermalcore` passed its message as the *format* to `__android_log_print`,
/// so any `%` in a package name became a garbage read.
///
/// # Safety
/// `tag` and `text` must be valid NUL-terminated C strings.
#[cfg(target_os = "android")]
pub unsafe fn logcat_write(
    prio: i32,
    tag: *const std::os::raw::c_char,
    text: *const std::os::raw::c_char,
) {
    // SAFETY: forwarded from this function's own contract — the caller
    // guarantees both pointers are valid NUL-terminated strings.
    unsafe {
        ffi::__android_log_write(prio, tag, text);
    }
}

/// Reads a property, returning `""` when unset — matching the C daemon's
/// `__system_property_get` contract, where every caller compares against `""`.
#[cfg(target_os = "android")]
pub fn getprop(name: &str) -> String {
    let Ok(key) = CString::new(name) else {
        return String::new();
    };
    // SAFETY: `key` is a valid NUL-terminated string. A null return means "no
    // such property", which is the documented `find` contract — not an error.
    let pi = unsafe { ffi::__system_property_find(key.as_ptr()) };
    if pi.is_null() {
        return String::new();
    }

    // The callback writes through the cookie; `&mut Option<String>` is
    // self-referential only for the duration of this call, and bionic invokes
    // the callback synchronously, so no value outlives the borrow.
    let mut out: Option<String> = None;
    // SAFETY: bionic calls this exactly once, synchronously, with valid
    // NUL-terminated `name`/`value` per the header's contract.
    unsafe extern "C" fn read_cb(
        cookie: *mut std::os::raw::c_void,
        _name: *const std::os::raw::c_char,
        value: *const std::os::raw::c_char,
        _serial: u32,
    ) {
        let slot = unsafe { &mut *(cookie.cast::<Option<String>>()) };
        // SAFETY: header guarantees `value` is a valid NUL-terminated string.
        let v = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        if !v.is_empty() {
            *slot = Some(v);
        }
    }

    // SAFETY: `pi` is non-null and live, `read_cb` matches `PropCallback`, and
    // the cookie is a valid `&mut Option<String>` for the whole call.
    unsafe {
        ffi::__system_property_read_callback(pi, read_cb, (&mut out as *mut Option<String>).cast());
    }
    out.unwrap_or_default()
}

/// Host-side stand-in so the crate still compiles and its tests still run off-device.
#[cfg(not(target_os = "android"))]
pub fn getprop(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

/// Writes a property. `ro.*` is refused by the kernel for a non-init caller —
/// same as `setprop`, same as before.
#[cfg(target_os = "android")]
pub fn setprop(name: &str, value: &str) -> bool {
    let (Ok(key), Ok(val)) = (CString::new(name), CString::new(value)) else {
        return false;
    };
    // SAFETY: both are valid NUL-terminated strings; bionic copies both.
    unsafe { ffi::__system_property_set(key.as_ptr(), val.as_ptr()) == 0 }
}

/// Host-side no-op. Properties do not exist off-device.
#[cfg(not(target_os = "android"))]
pub fn setprop(_name: &str, _value: &str) -> bool {
    false
}

/// Walks every property on the device, passing each name to `f`.
///
/// Built on the documented `__system_property_foreach` + `read_callback` pair;
/// `libc` exposes `foreach` but not the reader, so both are declared above.
#[cfg(target_os = "android")]
pub fn foreach_prop(mut f: impl FnMut(&str)) {
    // Accumulates names in place of allocating per property — this runs on every
    // daemon start over the whole property area.
    let mut sink: Vec<String> = Vec::new();

    // SAFETY: signature matches bionic's; the cookie is valid for the call.
    unsafe extern "C" fn each_cb(
        pi: *const std::os::raw::c_void,
        cookie: *mut std::os::raw::c_void,
    ) {
        let names = unsafe { &mut *(cookie.cast::<Vec<String>>()) };
        // SAFETY: per the header, this invokes cb once with valid strings.
        unsafe extern "C" fn read_cb(
            cookie: *mut std::os::raw::c_void,
            name: *const std::os::raw::c_char,
            value: *const std::os::raw::c_char,
            _serial: u32,
        ) {
            let _ = value;
            let names = unsafe { &mut *(cookie.cast::<Vec<String>>()) };
            // SAFETY: header guarantees a valid NUL-terminated `name`.
            let n = unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned();
            names.push(n);
        }
        unsafe {
            ffi::__system_property_read_callback(pi, read_cb, (names as *mut Vec<String>).cast())
        };
    }

    // SAFETY: `pi` comes from bionic itself and the cookie outlives the call.
    unsafe { ffi::__system_property_foreach(each_cb, (&mut sink as *mut Vec<String>).cast()) };
    for name in sink {
        f(&name);
    }
}

/// Host-side no-op: there is no property area to walk.
#[cfg(not(target_os = "android"))]
pub fn foreach_prop(_f: impl FnMut(&str)) {}

/// `true` only for the literal string `"true"`.
///
/// This is one property convention among several — the C macro `IS_TRUE`
/// (`AZenith.h:75`) and `thermalcore` both key off `"true"`, while most other
/// keys use `"1"`/`"0"`. Use [`getprop_bool_1`] for those.
pub fn is_true(value: &str) -> bool {
    value == "true"
}

/// Reads a property using the `"1"`/`"0"` boolean convention.
pub fn getprop_bool_1(name: &str) -> bool {
    getprop(name) == "1"
}

/// Deletes a property. bionic has no delete API, so this goes through `resetprop`,
/// which is what the C `PropValidator` already shelled out to.
pub fn resetprop(name: &str, value: &str) {
    let mut cmd = std::process::Command::new("resetprop");
    if value.is_empty() {
        cmd.arg("--delete");
    }
    cmd.arg(name);
    if !value.is_empty() {
        cmd.arg(value);
    }
    let _ = cmd.status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_property_reads_empty_not_a_panic() {
        let got = getprop("persist.sys.azenith.this.key.does.not.exist");
        assert!(got.is_empty(), "expected empty, got {got:?}");
    }

    #[test]
    fn is_true_matches_only_the_literal_true() {
        // The C macro compares against "true" only; "1" is a *different*
        // property convention and must not satisfy it.
        assert!(is_true("true"));
        assert!(!is_true("1"));
        assert!(!is_true("True"));
        assert!(!is_true(""));
    }

    #[test]
    fn foreach_is_callable_off_device() {
        // The host stub must be a no-op rather than a link error, otherwise the
        // crate's tests cannot run off-device at all.
        let mut n = 0;
        foreach_prop(|_| n += 1);
        #[cfg(not(target_os = "android"))]
        assert_eq!(n, 0);
    }
}
