//! C ABI surface — everything declared in include/notifykit.h.
//! Conventions (house standard):
//! - Functions return a NO_OK / NO_ERR_* code; JSON results
//!   are written through an out-pointer as caller-owned
//!   strings — free them with `no_free_string`.
//! - Failures set the out-pointer to NULL and set the
//!   thread-local last error (`no_last_error`).
//! - Nothing unwinds across the FFI boundary (catch_unwind
//!   everywhere; release profile also sets panic = "abort").
//! - Handles are opaque and thread-safe: each handle
//!   serializes its calls on an internal mutex. `no_close` /
//!   a successful `no_tray_remove` free the handle; using it
//!   afterwards is a caller bug.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::panic::catch_unwind;
use std::sync::Mutex;

use crate::platform::{self, Tray};
use crate::{
    Center, BALLOON_BODY_MAX_UNITS, BALLOON_TITLE_MAX_UNITS, ERR_INTERNAL, ERR_INVALID_ARG, NO_OK,
    TOOLTIP_MAX_UNITS,
};

/// Opaque center handle (declared as `NoCenter` in the
/// header).
pub struct NoCenter {
    inner: Mutex<Center>,
}

/// Opaque tray handle (declared as `NoTray` in the header).
pub struct NoTray {
    inner: Mutex<Tray>,
}

thread_local! {
    static LAST_ERROR: RefCell<(i32, CString)> =
        RefCell::new((0, CString::new("").unwrap()));
}

pub fn set_error(code: i32, msg: &str) {
    let clean: String = msg.chars().filter(|&c| c != '\0').collect();
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = (
            code,
            CString::new(clean).unwrap_or_else(|_| CString::new("error").unwrap()),
        );
    });
}

fn clear_error() {
    set_error(NO_OK, "");
}

fn fail(code: i32, msg: &str) -> c_int {
    set_error(code, msg);
    code as c_int
}

fn out_string(out: *mut *mut c_char, text: String) -> c_int {
    match CString::new(text) {
        Ok(s) => {
            unsafe { *out = s.into_raw() };
            clear_error();
            NO_OK as c_int
        }
        Err(_) => fail(ERR_INTERNAL, "result string contained a NUL byte"),
    }
}

fn guard<F: FnOnce() -> c_int>(f: F) -> c_int {
    match catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(code) => code,
        Err(_) => fail(ERR_INTERNAL, "internal panic caught at the FFI boundary"),
    }
}

fn cstr<'a>(ptr: *const c_char, what: &str) -> crate::NoResult<&'a str> {
    if ptr.is_null() {
        return Err((ERR_INVALID_ARG, format!("{what} must not be null")));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| (ERR_INVALID_ARG, format!("{what} must be valid UTF-8")))
}

fn opt_cstr<'a>(ptr: *const c_char, what: &str) -> crate::NoResult<Option<&'a str>> {
    if ptr.is_null() {
        Ok(None)
    } else {
        cstr(ptr, what).map(Some)
    }
}

fn center_ref(handle: *mut NoCenter) -> Result<&'static NoCenter, c_int> {
    if handle.is_null() {
        Err(fail(ERR_INVALID_ARG, "center handle must not be null"))
    } else {
        Ok(unsafe { &*handle })
    }
}

fn tray_ref(handle: *mut NoTray) -> Result<&'static NoTray, c_int> {
    if handle.is_null() {
        Err(fail(ERR_INVALID_ARG, "tray handle must not be null"))
    } else {
        Ok(unsafe { &*handle })
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[no_mangle]
pub extern "C" fn no_version() -> *const c_char {
    static VERSION_C: &[u8] = b"0.1.0\0";
    VERSION_C.as_ptr() as *const c_char
}

#[no_mangle]
pub extern "C" fn no_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().1.as_ptr())
}

#[no_mangle]
pub extern "C" fn no_last_error_code() -> c_int {
    LAST_ERROR.with(|e| e.borrow().0) as c_int
}

/// # Safety (caller contract): `p` is null or a string this
/// library returned and has not been freed yet. Documented
/// in the header.
#[no_mangle]
pub extern "C" fn no_free_string(p: *mut c_char) {
    if !p.is_null() {
        unsafe {
            drop(CString::from_raw(p));
        }
    }
}

// ---------------------------------------------------------------------------
// Notification center
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn no_create(config_json: *const c_char, out_handle: *mut *mut NoCenter) -> c_int {
    guard(|| {
        if out_handle.is_null() {
            return fail(ERR_INVALID_ARG, "out_handle must not be null");
        }
        unsafe { *out_handle = std::ptr::null_mut() };
        match opt_cstr(config_json, "config_json").and_then(crate::parse_center_config) {
            Ok(cfg) => {
                let boxed = Box::new(NoCenter {
                    inner: Mutex::new(Center::new(cfg)),
                });
                unsafe { *out_handle = Box::into_raw(boxed) };
                clear_error();
                NO_OK as c_int
            }
            Err((code, msg)) => fail(code, &msg),
        }
    })
}

#[no_mangle]
pub extern "C" fn no_enqueue(
    handle: *mut NoCenter,
    item_json: *const c_char,
    out_json: *mut *mut c_char,
) -> c_int {
    guard(|| {
        if out_json.is_null() {
            return fail(ERR_INVALID_ARG, "out_json must not be null");
        }
        unsafe { *out_json = std::ptr::null_mut() };
        let center = match center_ref(handle) {
            Ok(c) => c,
            Err(code) => return code,
        };
        match cstr(item_json, "item_json") {
            Ok(text) => match lock(&center.inner).enqueue_json(text) {
                Ok(result) => out_string(out_json, result),
                Err((code, msg)) => fail(code, &msg),
            },
            Err((code, msg)) => fail(code, &msg),
        }
    })
}

#[no_mangle]
pub extern "C" fn no_poll_events(handle: *mut NoCenter, out_json: *mut *mut c_char) -> c_int {
    guard(|| {
        if out_json.is_null() {
            return fail(ERR_INVALID_ARG, "out_json must not be null");
        }
        unsafe { *out_json = std::ptr::null_mut() };
        match center_ref(handle) {
            Ok(center) => {
                let text = lock(&center.inner).poll_events();
                out_string(out_json, text)
            }
            Err(code) => code,
        }
    })
}

#[no_mangle]
pub extern "C" fn no_history(handle: *mut NoCenter, out_json: *mut *mut c_char) -> c_int {
    guard(|| {
        if out_json.is_null() {
            return fail(ERR_INVALID_ARG, "out_json must not be null");
        }
        unsafe { *out_json = std::ptr::null_mut() };
        match center_ref(handle) {
            Ok(center) => {
                let text = lock(&center.inner).history_json();
                out_string(out_json, text)
            }
            Err(code) => code,
        }
    })
}

#[no_mangle]
pub extern "C" fn no_stats(handle: *mut NoCenter, out_json: *mut *mut c_char) -> c_int {
    guard(|| {
        if out_json.is_null() {
            return fail(ERR_INVALID_ARG, "out_json must not be null");
        }
        unsafe { *out_json = std::ptr::null_mut() };
        match center_ref(handle) {
            Ok(center) => {
                let text = lock(&center.inner).stats_json();
                out_string(out_json, text)
            }
            Err(code) => code,
        }
    })
}

#[no_mangle]
pub extern "C" fn no_close(handle: *mut NoCenter) -> c_int {
    guard(|| {
        if handle.is_null() {
            return fail(ERR_INVALID_ARG, "center handle must not be null");
        }
        unsafe { drop(Box::from_raw(handle)) };
        clear_error();
        NO_OK as c_int
    })
}

// ---------------------------------------------------------------------------
// Windows tray half
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn no_tray_create(
    config_json: *const c_char,
    out_handle: *mut *mut NoTray,
) -> c_int {
    guard(|| {
        if out_handle.is_null() {
            return fail(ERR_INVALID_ARG, "out_handle must not be null");
        }
        unsafe { *out_handle = std::ptr::null_mut() };
        let cfg = match opt_cstr(config_json, "config_json").and_then(crate::parse_tray_config) {
            Ok(c) => c,
            Err((code, msg)) => return fail(code, &msg),
        };
        match platform::tray_create(&cfg) {
            Ok(tray) => {
                let boxed = Box::new(NoTray {
                    inner: Mutex::new(tray),
                });
                unsafe { *out_handle = Box::into_raw(boxed) };
                clear_error();
                NO_OK as c_int
            }
            Err((code, msg)) => fail(code, &msg),
        }
    })
}

#[no_mangle]
pub extern "C" fn no_tray_show(
    handle: *mut NoTray,
    show_json: *const c_char,
    out_json: *mut *mut c_char,
) -> c_int {
    guard(|| {
        if out_json.is_null() {
            return fail(ERR_INVALID_ARG, "out_json must not be null");
        }
        unsafe { *out_json = std::ptr::null_mut() };
        let tray = match tray_ref(handle) {
            Ok(t) => t,
            Err(code) => return code,
        };
        let req = match cstr(show_json, "show_json").and_then(crate::parse_show_request) {
            Ok(r) => r,
            Err((code, msg)) => return fail(code, &msg),
        };
        // Truncate BEFORE the platform call and report the
        // flags in the result: the text the shell displays
        // is exactly the text described here.
        let (title, title_truncated) = crate::truncate_utf16(&req.title, BALLOON_TITLE_MAX_UNITS);
        let (body, body_truncated) = crate::truncate_utf16(&req.body, BALLOON_BODY_MAX_UNITS);
        let tray = lock(&tray.inner);
        match platform::tray_show(&tray, &title, &body, req.kind, req.timeout_ms) {
            Ok(()) => {
                let result = crate::json::obj(vec![
                    ("shown", crate::json::b(true)),
                    ("title_truncated", crate::json::b(title_truncated)),
                    ("body_truncated", crate::json::b(body_truncated)),
                ])
                .to_json();
                out_string(out_json, result)
            }
            Err((code, msg)) => fail(code, &msg),
        }
    })
}

#[no_mangle]
pub extern "C" fn no_tray_set_tooltip(
    handle: *mut NoTray,
    tooltip: *const c_char,
    out_json: *mut *mut c_char,
) -> c_int {
    guard(|| {
        if out_json.is_null() {
            return fail(ERR_INVALID_ARG, "out_json must not be null");
        }
        unsafe { *out_json = std::ptr::null_mut() };
        let tray = match tray_ref(handle) {
            Ok(t) => t,
            Err(code) => return code,
        };
        let text = match cstr(tooltip, "tooltip") {
            Ok(t) => t,
            Err((code, msg)) => return fail(code, &msg),
        };
        let (tip, truncated) = crate::truncate_utf16(text, TOOLTIP_MAX_UNITS);
        let mut tray = lock(&tray.inner);
        match platform::tray_set_tooltip(&mut tray, &tip) {
            Ok(()) => {
                let result = crate::json::obj(vec![
                    ("tooltip", crate::json::s(&tip)),
                    ("truncated", crate::json::b(truncated)),
                ])
                .to_json();
                out_string(out_json, result)
            }
            Err((code, msg)) => fail(code, &msg),
        }
    })
}

#[no_mangle]
pub extern "C" fn no_tray_remove(handle: *mut NoTray) -> c_int {
    guard(|| {
        if handle.is_null() {
            return fail(ERR_INVALID_ARG, "tray handle must not be null");
        }
        let tray = unsafe { &*handle };
        let result = {
            let tray = lock(&tray.inner);
            platform::tray_remove(&tray)
        };
        match result {
            Ok(()) => {
                unsafe { drop(Box::from_raw(handle)) };
                clear_error();
                NO_OK as c_int
            }
            // On failure the handle stays valid so the
            // caller can retry or inspect the error.
            Err((code, msg)) => fail(code, &msg),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ERR_INVALID_ARG;
    #[cfg(not(windows))]
    use crate::ERR_NOT_SUPPORTED;

    fn cs(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    fn take_string(p: *mut c_char) -> String {
        assert!(!p.is_null());
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string();
        no_free_string(p);
        s
    }

    #[test]
    fn ffi_center_end_to_end() {
        let mut handle: *mut NoCenter = std::ptr::null_mut();
        assert_eq!(
            no_create(cs("{\"max_queue\":4}").as_ptr(), &mut handle),
            NO_OK as c_int
        );
        assert!(!handle.is_null());
        let mut out: *mut c_char = std::ptr::null_mut();
        assert_eq!(
            no_enqueue(
                handle,
                cs("{\"title\":\"Build done\",\"body\":\"3 warnings\"}").as_ptr(),
                &mut out
            ),
            NO_OK as c_int
        );
        let v = crate::json::parse(&take_string(out)).unwrap();
        assert_eq!(v.get("coalesced").unwrap().as_bool(), Some(false));
        assert_eq!(
            no_enqueue(
                handle,
                cs("{\"title\":\"Build done\",\"body\":\"3 warnings\"}").as_ptr(),
                &mut out
            ),
            NO_OK as c_int
        );
        let v = crate::json::parse(&take_string(out)).unwrap();
        assert_eq!(v.get("count").unwrap().as_i64(), Some(2));

        assert_eq!(no_poll_events(handle, &mut out), NO_OK as c_int);
        let v = crate::json::parse(&take_string(out)).unwrap();
        assert_eq!(v.get("count").unwrap().as_i64(), Some(1));

        assert_eq!(no_history(handle, &mut out), NO_OK as c_int);
        let v = crate::json::parse(&take_string(out)).unwrap();
        assert_eq!(v.get("count").unwrap().as_i64(), Some(1));

        assert_eq!(no_stats(handle, &mut out), NO_OK as c_int);
        let v = crate::json::parse(&take_string(out)).unwrap();
        assert_eq!(v.get("shown").unwrap().as_i64(), Some(1));
        assert_eq!(v.get("coalesced").unwrap().as_i64(), Some(1));
        assert_eq!(no_last_error_code(), NO_OK as c_int);

        assert_eq!(no_close(handle), NO_OK as c_int);
    }

    #[test]
    fn ffi_center_bad_args() {
        let mut handle: *mut NoCenter = std::ptr::null_mut();
        // Bad config JSON: parse-level failure, handle stays null.
        assert_eq!(
            no_create(cs("{\"max_queue\":0}").as_ptr(), &mut handle),
            ERR_INVALID_ARG as c_int
        );
        assert!(handle.is_null());
        assert_eq!(no_last_error_code(), ERR_INVALID_ARG as c_int);
        // NULL config is legal (all defaults).
        assert_eq!(no_create(std::ptr::null(), &mut handle), NO_OK as c_int);
        assert!(!handle.is_null());
        let mut out: *mut c_char = std::ptr::null_mut();
        // Null handle / null item / malformed item.
        assert_eq!(
            no_enqueue(std::ptr::null_mut(), cs("{}").as_ptr(), &mut out),
            ERR_INVALID_ARG as c_int
        );
        assert!(out.is_null());
        assert_eq!(
            no_enqueue(handle, std::ptr::null(), &mut out),
            ERR_INVALID_ARG as c_int
        );
        assert_eq!(
            no_enqueue(handle, cs("{\"title\":\"T\"}").as_ptr(), &mut out),
            ERR_INVALID_ARG as c_int
        );
        assert_eq!(no_close(std::ptr::null_mut()), ERR_INVALID_ARG as c_int);
        no_free_string(std::ptr::null_mut()); // must be a no-op
        assert_eq!(no_close(handle), NO_OK as c_int);
    }

    #[cfg(not(windows))]
    #[test]
    fn ffi_tray_is_not_supported_off_windows() {
        let mut tray: *mut NoTray = std::ptr::null_mut();
        assert_eq!(
            no_tray_create(cs("{\"tooltip\":\"x\"}").as_ptr(), &mut tray),
            ERR_NOT_SUPPORTED as c_int
        );
        assert!(tray.is_null());
        assert_eq!(no_last_error_code(), ERR_NOT_SUPPORTED as c_int);
        // Invalid arguments still win over the platform check.
        let mut out: *mut c_char = std::ptr::null_mut();
        assert_eq!(
            no_tray_show(std::ptr::null_mut(), cs("{}").as_ptr(), &mut out),
            ERR_INVALID_ARG as c_int
        );
        assert_eq!(
            no_tray_set_tooltip(std::ptr::null_mut(), cs("x").as_ptr(), &mut out),
            ERR_INVALID_ARG as c_int
        );
        assert_eq!(
            no_tray_remove(std::ptr::null_mut()),
            ERR_INVALID_ARG as c_int
        );
        // A malformed show request is INVALID_ARG, not a
        // tray success — nothing is ever faked.
        assert_eq!(
            no_tray_create(cs("{\"icon\":\"shiny\"}").as_ptr(), &mut tray),
            ERR_INVALID_ARG as c_int
        );
    }
}
