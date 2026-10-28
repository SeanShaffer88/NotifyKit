//! Platform layer: the only place Windows APIs are touched.
//!
//! On Windows this is the tray half of NotifyKit: one tray
//! icon per `Tray` handle, registered with
//! Shell_NotifyIconW (shell32), balloons shown with
//! NIM_MODIFY + NIF_INFO, icons loaded from the user32 stock
//! set with LoadIconW — no WinRT, no external icon files.
//!
//! On non-Windows targets everything returns
//! ERR_NOT_SUPPORTED so the pure logic and FFI contract stay
//! testable anywhere. Nothing here reports success for a call
//! that did not happen.
//!
//! Every extern block MUST carry #[link(name = "...")] naming
//! its import library (shell32 / user32 / kernel32). Without
//! it the build fails at LINK time with LNK2019 — check and
//! clippy cannot catch it; CI's real build can.

use crate::{Kind, TrayConfig, ERR_NOT_SUPPORTED};

pub type PResult<T> = Result<T, (i32, String)>;

pub fn not_supported<T>(what: &str) -> PResult<T> {
    Err((
        ERR_NOT_SUPPORTED,
        format!("{what} is only available on Windows"),
    ))
}

/// Default balloon timeout used when a show request passes
/// timeout_ms = 0.
pub const DEFAULT_TIMEOUT_MS: u32 = 10_000;

/// Callback message id sent to the host window for tray
/// events (WM_USER + 100) when a tray icon is created with a
/// non-zero hwnd. Documented in the header.
pub const TRAY_CALLBACK_MSG: u32 = 0x0400 + 100;

/// One registered tray icon. Created only by a successful
/// NIM_ADD on Windows; on other platforms `tray_create`
/// fails, so a `Tray` value is never handed out there.
#[derive(Debug)]
pub struct Tray {
    pub hwnd: u64,
    pub uid: u32,
    pub icon: Kind,
    pub tooltip: String,
}

/// Register a tray icon (NIM_ADD) from `cfg`. The tooltip is
/// truncated to the szTip budget by the caller-visible
/// [`crate::truncate_utf16`] rule; the returned `Tray`
/// records the tooltip actually registered.
#[cfg(windows)]
pub fn tray_create(cfg: &TrayConfig) -> PResult<Tray> {
    let tray = Tray {
        hwnd: cfg.hwnd,
        uid: cfg.uid,
        icon: cfg.icon,
        tooltip: crate::truncate_utf16(&cfg.tooltip, crate::TOOLTIP_MAX_UNITS).0,
    };
    win::add_icon(&tray)?;
    Ok(tray)
}

#[cfg(not(windows))]
pub fn tray_create(_cfg: &TrayConfig) -> PResult<Tray> {
    not_supported("tray icons")
}

/// Show a balloon on an existing tray icon (NIM_MODIFY with
/// NIF_INFO). `title` and `body` must already be truncated
/// to the balloon budgets — the FFI truncates and reports
/// the flags; this layer copies them into the fixed arrays
/// via `utf16z`, which re-applies the same bound, so the
/// struct can never overflow.
#[cfg(windows)]
pub fn tray_show(tray: &Tray, title: &str, body: &str, kind: Kind, timeout_ms: u32) -> PResult<()> {
    win::show_balloon(tray, title, body, kind, timeout_ms)
}

#[cfg(not(windows))]
pub fn tray_show(
    _tray: &Tray,
    _title: &str,
    _body: &str,
    _kind: Kind,
    _timeout_ms: u32,
) -> PResult<()> {
    not_supported("tray balloons")
}

/// Replace the tray icon's tooltip (NIM_MODIFY with
/// NIF_TIP). `tooltip` must already be truncated to the
/// szTip budget (the FFI does it and reports the flag).
#[cfg(windows)]
pub fn tray_set_tooltip(tray: &mut Tray, tooltip: &str) -> PResult<()> {
    win::set_tooltip(tray, tooltip)?;
    tray.tooltip = tooltip.to_string();
    Ok(())
}

#[cfg(not(windows))]
pub fn tray_set_tooltip(_tray: &mut Tray, _tooltip: &str) -> PResult<()> {
    not_supported("tray tooltips")
}

/// Remove the tray icon (NIM_DELETE). Consumes the handle;
/// the FFI frees it only when this returns Ok.
#[cfg(windows)]
pub fn tray_remove(tray: &Tray) -> PResult<()> {
    win::delete_icon(tray)
}

#[cfg(not(windows))]
pub fn tray_remove(_tray: &Tray) -> PResult<()> {
    not_supported("tray icons")
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::ERR_PLATFORM;
    use std::os::raw::c_void;

    // NOTIFYICONDATAW message ids.
    const NIM_ADD: u32 = 0x0000_0000;
    const NIM_MODIFY: u32 = 0x0000_0001;
    const NIM_DELETE: u32 = 0x0000_0002;
    // uFlags bits.
    const NIF_MESSAGE: u32 = 0x0000_0001;
    const NIF_ICON: u32 = 0x0000_0002;
    const NIF_TIP: u32 = 0x0000_0004;
    const NIF_INFO: u32 = 0x0000_0010;
    // dwInfoFlags balloon severities.
    const NIIF_NONE: u32 = 0x0000_0000;
    const NIIF_INFO: u32 = 0x0000_0001;
    const NIIF_WARNING: u32 = 0x0000_0002;
    const NIIF_ERROR: u32 = 0x0000_0003;
    // Stock icon ids (MAKEINTRESOURCE values for LoadIconW).
    const IDI_APPLICATION: usize = 32512;
    const IDI_ERROR: usize = 32513; // IDI_HAND
    const IDI_WARNING: usize = 32515; // IDI_EXCLAMATION
    const IDI_INFORMATION: usize = 32516; // IDI_ASTERISK

    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)] // fields are read by the shell, not by Rust
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    /// NOTIFYICONDATAW, base V3 layout (the version with
    /// szInfo/szInfoTitle/dwInfoFlags and guidItem, WITHOUT
    /// the Vista-era hBalloonIcon tail). cbSize is
    /// size_of::<Self>() — the documented way to tell the
    /// shell which struct version is being passed. The text
    /// arrays are exactly the buffer sizes the pure layer's
    /// budgets describe (64 / 256 / 128 units incl. NUL).
    #[repr(C)]
    #[allow(dead_code)] // fields are read by the shell, not by Rust
    struct NotifyIconDataW {
        cb_size: u32,
        h_wnd: *mut c_void,
        u_id: u32,
        u_flags: u32,
        u_callback_message: u32,
        h_icon: *mut c_void,
        sz_tip: [u16; 128],
        dw_state: u32,
        dw_state_mask: u32,
        sz_info: [u16; 256],
        u_timeout: u32,
        sz_info_title: [u16; 64],
        dw_info_flags: u32,
        guid_item: Guid,
    }

    impl NotifyIconDataW {
        fn new(tray: &Tray) -> NotifyIconDataW {
            NotifyIconDataW {
                cb_size: std::mem::size_of::<NotifyIconDataW>() as u32,
                h_wnd: tray.hwnd as *mut c_void,
                u_id: tray.uid,
                u_flags: 0,
                u_callback_message: 0,
                h_icon: std::ptr::null_mut(),
                sz_tip: [0; 128],
                dw_state: 0,
                dw_state_mask: 0,
                sz_info: [0; 256],
                u_timeout: 0,
                sz_info_title: [0; 64],
                dw_info_flags: 0,
                guid_item: Guid {
                    data1: 0,
                    data2: 0,
                    data3: 0,
                    data4: [0; 8],
                },
            }
        }
    }

    #[link(name = "shell32")]
    extern "system" {
        fn Shell_NotifyIconW(dw_message: u32, lp_data: *mut NotifyIconDataW) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn LoadIconW(h_instance: *mut c_void, lp_icon_name: *const u16) -> *mut c_void;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }

    fn stock_icon(kind: Kind) -> *mut c_void {
        let id = match kind {
            Kind::App => IDI_APPLICATION,
            Kind::Info => IDI_INFORMATION,
            Kind::Warn => IDI_WARNING,
            Kind::Error => IDI_ERROR,
        };
        // SAFETY: LoadIconW with a NULL instance and a
        // MAKEINTRESOURCE stock id is the documented way to
        // load a shared stock icon; the returned handle is
        // owned by the system and must not be destroyed —
        // this code never destroys it.
        unsafe { LoadIconW(std::ptr::null_mut(), id as *const u16) }
    }

    fn info_flags(kind: Kind) -> u32 {
        match kind {
            Kind::App => NIIF_NONE,
            Kind::Info => NIIF_INFO,
            Kind::Warn => NIIF_WARNING,
            Kind::Error => NIIF_ERROR,
        }
    }

    fn copy_into<const N: usize>(dst: &mut [u16; N], text: &str) {
        let buf = crate::utf16z(text, N);
        dst.copy_from_slice(&buf);
    }

    fn notify(message: u32, data: &mut NotifyIconDataW, what: &str) -> PResult<()> {
        // SAFETY: `data` is a fully initialized V3 struct
        // whose cb_size matches its real size; the shell
        // reads/writes only within it for the call's
        // duration.
        let ok = unsafe { Shell_NotifyIconW(message, data) };
        if ok != 0 {
            Ok(())
        } else {
            // SAFETY: GetLastError takes no arguments.
            let gle = unsafe { GetLastError() };
            Err((
                ERR_PLATFORM,
                format!("Shell_NotifyIconW({what}) failed (GetLastError={gle})"),
            ))
        }
    }

    pub fn add_icon(tray: &Tray) -> PResult<()> {
        let icon = stock_icon(tray.icon);
        if icon.is_null() {
            // SAFETY: GetLastError takes no arguments.
            let gle = unsafe { GetLastError() };
            return Err((
                ERR_PLATFORM,
                format!("LoadIconW for the tray icon failed (GetLastError={gle})"),
            ));
        }
        let mut data = NotifyIconDataW::new(tray);
        data.u_flags = NIF_ICON | NIF_TIP;
        if tray.hwnd != 0 {
            data.u_flags |= NIF_MESSAGE;
            data.u_callback_message = TRAY_CALLBACK_MSG;
        }
        data.h_icon = icon;
        copy_into(&mut data.sz_tip, &tray.tooltip);
        notify(NIM_ADD, &mut data, "NIM_ADD")
    }

    pub fn show_balloon(
        tray: &Tray,
        title: &str,
        body: &str,
        kind: Kind,
        timeout_ms: u32,
    ) -> PResult<()> {
        let mut data = NotifyIconDataW::new(tray);
        data.u_flags = NIF_INFO;
        copy_into(&mut data.sz_info, body);
        copy_into(&mut data.sz_info_title, title);
        data.u_timeout = if timeout_ms == 0 {
            DEFAULT_TIMEOUT_MS
        } else {
            timeout_ms
        };
        data.dw_info_flags = info_flags(kind);
        notify(NIM_MODIFY, &mut data, "NIM_MODIFY balloon")
    }

    pub fn set_tooltip(tray: &Tray, tooltip: &str) -> PResult<()> {
        let mut data = NotifyIconDataW::new(tray);
        data.u_flags = NIF_TIP;
        copy_into(&mut data.sz_tip, tooltip);
        notify(NIM_MODIFY, &mut data, "NIM_MODIFY tooltip")
    }

    pub fn delete_icon(tray: &Tray) -> PResult<()> {
        let mut data = NotifyIconDataW::new(tray);
        notify(NIM_DELETE, &mut data, "NIM_DELETE")
    }
}

#[cfg(test)]
#[cfg(not(windows))]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn tray_calls_are_not_supported_off_windows() {
        let cfg = TrayConfig::default();
        assert_eq!(tray_create(&cfg).unwrap_err().0, ERR_NOT_SUPPORTED);
        let mut tray = Tray {
            hwnd: 0,
            uid: 1,
            icon: Kind::App,
            tooltip: String::new(),
        };
        assert_eq!(
            tray_show(&tray, "t", "b", Kind::Info, 0).unwrap_err().0,
            ERR_NOT_SUPPORTED
        );
        assert_eq!(
            tray_set_tooltip(&mut tray, "x").unwrap_err().0,
            ERR_NOT_SUPPORTED
        );
        assert_eq!(tray_remove(&tray).unwrap_err().0, ERR_NOT_SUPPORTED);
    }
}
