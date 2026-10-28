//! Platform layer: the only place Windows APIs are touched.
//!
//! On Windows this is the tray half of NotifyKit: one tray
//! icon per `Tray` handle, registered with
//! Shell_NotifyIconW (shell32), balloons shown with
//! NIM_MODIFY + NIF_INFO, icons loaded from the user32 stock
//! set with LoadIconW — no external icon files. Since v0.2
//! a handle can instead show WinRT toasts (the `win::winrt`
//! module; combase.dll is loaded dynamically, so there are
//! no new link dependencies).
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

use crate::{Backend, Kind, TrayConfig, ERR_NOT_SUPPORTED};

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
    /// AppUserModelID from the tray config ("" = none);
    /// required by the toast backend, ignored by balloons.
    pub app_id: String,
    /// Which show mechanism this handle uses. Defaults to
    /// the classic balloon; changed by `no_tray_set_backend`.
    pub backend: Backend,
}

impl Tray {
    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn set_backend(&mut self, backend: Backend) {
        self.backend = backend;
    }
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
        app_id: cfg.app_id.clone(),
        backend: Backend::Balloon,
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

/// Select which show mechanism a tray handle uses. This is
/// pure handle state on Windows; elsewhere it is a tray
/// call and returns NOT_SUPPORTED like the rest.
#[cfg(windows)]
pub fn tray_set_backend(tray: &mut Tray, backend: Backend) -> PResult<()> {
    tray.set_backend(backend);
    Ok(())
}

#[cfg(not(windows))]
pub fn tray_set_backend(_tray: &mut Tray, _backend: Backend) -> PResult<()> {
    not_supported("tray backends")
}

/// Show a WinRT toast for a tray handle whose backend is
/// `Backend::Toast`. `title` and `body` are the full,
/// untruncated show-request text — toasts have no
/// fixed-buffer limits; the XML is built (and escaped) by
/// [`crate::build_toast_xml`]. Requires the handle's
/// app_id: a toast is delivered under that AppUserModelID
/// and there is no honest fallback identity to borrow.
#[cfg(windows)]
pub fn tray_show_toast(tray: &Tray, title: &str, body: &str) -> PResult<()> {
    win::show_toast(tray, title, body)
}

#[cfg(not(windows))]
pub fn tray_show_toast(_tray: &Tray, _title: &str, _body: &str) -> PResult<()> {
    not_supported("WinRT toasts")
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
        // Used only to resolve combase.dll entry points for
        // the WinRT toast path at runtime — NotifyKit links
        // nothing WinRT-specific.
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *mut c_void;
        fn FreeLibrary(h_lib_module: *mut c_void) -> i32;
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

    pub fn show_toast(tray: &Tray, title: &str, body: &str) -> PResult<()> {
        if tray.app_id.is_empty() {
            return Err((
                crate::ERR_INVALID_ARG,
                "an app_id is required for toasts: set \"app_id\" in the \
                 no_tray_create config to the host's AppUserModelID"
                    .to_string(),
            ));
        }
        let xml = crate::build_toast_xml(title, body);
        winrt::show_toast(&tray.app_id, &xml)
    }

    /// Minimal hand-rolled WinRT activation for one toast.
    ///
    /// combase.dll is loaded at runtime and every entry
    /// point is resolved with GetProcAddress, so the DLL
    /// gains no WinRT import library and no new link
    /// dependency (this also keeps the crate's
    /// zero-dependency rule: the `windows` crate is not
    /// used). All interface IIDs and vtable layouts below
    /// were verified against the `windows` crate's
    /// generated bindings (windows 0.62.2,
    /// `windows_core::imp::define_interface!` and the
    /// `*_Vtbl` structs in Windows::UI::Notifications and
    /// Windows::Data::Xml::Dom): every WinRT interface
    /// vtable starts with IInspectable (IUnknown's
    /// QueryInterface / AddRef / Release, then GetIids /
    /// GetRuntimeClassName / GetTrustLevel), and the
    /// interface's own methods follow in declaration order.
    ///
    /// Windows runtime behaviour of this module is
    /// UNVERIFIED: it is compiled and clippy-checked for
    /// the Windows target at the gate, but the gate does
    /// not link and nothing here has run on a real desktop.
    pub mod winrt {
        use super::Guid;
        use crate::{ERR_INVALID_ARG, ERR_PLATFORM};
        use std::os::raw::c_void;

        type HResult = i32;
        /// HSTRING is an opaque handle-sized value.
        type HString = *mut c_void;

        const S_OK: HResult = 0;
        const S_FALSE: HResult = 1;
        /// RPC_E_CHANGED_MODE: the calling thread was
        /// already initialized with a different apartment
        /// model. RoInitialize reports it as a failure
        /// code, but the thread IS initialized (by its
        /// owner, who also owns uninitializing it), so the
        /// toast flow can proceed.
        const RPC_E_CHANGED_MODE: HResult = 0x8001_0106u32 as HResult;
        /// RO_INIT_MULTITHREADED for RoInitialize.
        const RO_INIT_MULTITHREADED: i32 = 1;

        // Interface IIDs — source for each: the `windows`
        // crate 0.62.2 generated bindings,
        // define_interface! invocations in
        // src/Windows/UI/Notifications/mod.rs and
        // src/Windows/Data/Xml/Dom/mod.rs.
        /// IToastNotificationManagerStatics
        /// 50ac103f-d235-4598-bbef-98fe4d1a3ad4
        const IID_MANAGER_STATICS: Guid = Guid {
            data1: 0x50ac_103f,
            data2: 0xd235,
            data3: 0x4598,
            data4: [0xbb, 0xef, 0x98, 0xfe, 0x4d, 0x1a, 0x3a, 0xd4],
        };
        /// IToastNotificationFactory
        /// 04124b20-82c6-4229-b109-fd9ed4662b53
        const IID_TOAST_FACTORY: Guid = Guid {
            data1: 0x0412_4b20,
            data2: 0x82c6,
            data3: 0x4229,
            data4: [0xb1, 0x09, 0xfd, 0x9e, 0xd4, 0x66, 0x2b, 0x53],
        };
        /// IToastNotifier 75927b93-03f3-41ec-91d3-6e5bac1b38e7
        /// (its IID is not needed at runtime — Show is
        /// called on the pointer CreateToastNotifierWithId
        /// returns — but is recorded here as the interface
        /// whose vtable layout IToastNotifierVtbl mirrors:
        /// Show is its first method, slot 6.)
        #[allow(dead_code)]
        const IID_TOAST_NOTIFIER: Guid = Guid {
            data1: 0x7592_7b93,
            data2: 0x03f3,
            data3: 0x41ec,
            data4: [0x91, 0xd3, 0x6e, 0x5b, 0xac, 0x1b, 0x38, 0xe7],
        };
        /// IXmlDocument f7f3a506-1e87-42d6-bcfb-b8c809fa5494
        const IID_XML_DOCUMENT: Guid = Guid {
            data1: 0xf7f3_a506,
            data2: 0x1e87,
            data3: 0x42d6,
            data4: [0xbc, 0xfb, 0xb8, 0xc8, 0x09, 0xfa, 0x54, 0x94],
        };
        /// IXmlDocumentIO 6cd0e74e-ee65-4489-9ebf-ca43e87ba637
        const IID_XML_DOCUMENT_IO: Guid = Guid {
            data1: 0x6cd0_e74e,
            data2: 0xee65,
            data3: 0x4489,
            data4: [0x9e, 0xbf, 0xca, 0x43, 0xe8, 0x7b, 0xa6, 0x37],
        };

        // ---- vtable layouts (verified as noted above) ----

        type QueryInterfaceFn =
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult;
        type AddRefFn = unsafe extern "system" fn(*mut c_void) -> u32;
        type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;

        #[repr(C)]
        struct IUnknownVtbl {
            query_interface: QueryInterfaceFn,
            add_ref: AddRefFn,
            release: ReleaseFn,
        }

        /// IInspectable = IUnknown + GetIids +
        /// GetRuntimeClassName + GetTrustLevel. The three
        /// IInspectable methods are never called here; the
        /// slots exist so the offsets of the interface
        /// methods that follow are right.
        #[repr(C)]
        struct IInspectableVtbl {
            base: IUnknownVtbl,
            get_iids: usize,
            get_runtime_class_name: usize,
            get_trust_level: usize,
        }

        /// IXmlDocumentIO: LoadXml (slot 6),
        /// LoadXmlWithSettings (7), SaveToFileAsync (8).
        #[repr(C)]
        struct IXmlDocumentIoVtbl {
            base: IInspectableVtbl,
            load_xml: unsafe extern "system" fn(*mut c_void, HString) -> HResult,
            load_xml_with_settings: usize,
            save_to_file_async: usize,
        }

        /// IToastNotificationFactory: CreateToastNotification
        /// (slot 6, its only method).
        #[repr(C)]
        struct IToastNotificationFactoryVtbl {
            base: IInspectableVtbl,
            create_toast_notification:
                unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HResult,
        }

        /// IToastNotificationManagerStatics:
        /// CreateToastNotifier (slot 6),
        /// CreateToastNotifierWithId (7),
        /// GetTemplateContent (8).
        #[repr(C)]
        struct IToastNotificationManagerStaticsVtbl {
            base: IInspectableVtbl,
            create_toast_notifier: usize,
            create_toast_notifier_with_id:
                unsafe extern "system" fn(*mut c_void, HString, *mut *mut c_void) -> HResult,
            get_template_content: usize,
        }

        /// IToastNotifier: Show (slot 6), Hide (7),
        /// Setting (8), AddToSchedule (9),
        /// RemoveFromSchedule (10),
        /// GetScheduledToastNotifications (11). Only Show
        /// is called; the vtable is used only up to it.
        #[repr(C)]
        struct IToastNotifierVtbl {
            base: IInspectableVtbl,
            show: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HResult,
        }

        // ---- dynamically resolved combase entry points ----

        type RoInitializeFn = unsafe extern "system" fn(i32) -> HResult;
        type RoUninitializeFn = unsafe extern "system" fn();
        type RoActivateInstanceFn = unsafe extern "system" fn(HString, *mut *mut c_void) -> HResult;
        type RoGetActivationFactoryFn =
            unsafe extern "system" fn(HString, *const Guid, *mut *mut c_void) -> HResult;
        type WindowsCreateStringFn =
            unsafe extern "system" fn(*const u16, u32, *mut HString) -> HResult;
        type WindowsDeleteStringFn = unsafe extern "system" fn(HString) -> HResult;

        struct Api {
            ro_initialize: RoInitializeFn,
            ro_uninitialize: RoUninitializeFn,
            ro_activate_instance: RoActivateInstanceFn,
            ro_get_activation_factory: RoGetActivationFactoryFn,
            windows_create_string: WindowsCreateStringFn,
            windows_delete_string: WindowsDeleteStringFn,
        }

        fn hr_err(what: &str, hr: HResult) -> (i32, String) {
            (
                ERR_PLATFORM,
                format!("{what} failed (HRESULT 0x{:08X})", hr as u32),
            )
        }

        fn check(what: &str, hr: HResult) -> super::PResult<()> {
            if hr < 0 {
                Err(hr_err(what, hr))
            } else {
                Ok(())
            }
        }

        fn wide_z(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }

        /// Load combase.dll and resolve every entry point
        /// the toast flow needs. On success returns the Api
        /// table and the module handle (the caller frees
        /// the library after the call completes).
        fn load_api() -> super::PResult<(Api, *mut c_void)> {
            // SAFETY: LoadLibraryW on a fixed literal name;
            // the returned handle is owned by the caller
            // and freed with FreeLibrary exactly once.
            let module = unsafe { super::LoadLibraryW(wide_z("combase.dll").as_ptr()) };
            if module.is_null() {
                // SAFETY: GetLastError takes no arguments.
                let gle = unsafe { super::GetLastError() };
                return Err((
                    ERR_PLATFORM,
                    format!("LoadLibraryW(combase.dll) failed (GetLastError={gle})"),
                ));
            }
            // SAFETY: `name` is a NUL-terminated ASCII
            // literal; the resolved address is only ever
            // transmuted to the documented signature of the
            // named combase export.
            unsafe fn resolve<T>(module: *mut c_void, name: &'static str) -> super::PResult<T> {
                let mut bytes = name.as_bytes().to_vec();
                bytes.push(0);
                let addr = super::GetProcAddress(module, bytes.as_ptr());
                if addr.is_null() {
                    return Err((
                        ERR_PLATFORM,
                        format!("GetProcAddress(combase.dll, {name}) returned null"),
                    ));
                }
                Ok(std::mem::transmute_copy(&addr))
            }
            // SAFETY: every name below is a documented
            // combase.dll export and T is its documented
            // signature (see the type aliases above).
            let api = unsafe {
                Api {
                    ro_initialize: resolve(module, "RoInitialize")?,
                    ro_uninitialize: resolve(module, "RoUninitialize")?,
                    ro_activate_instance: resolve(module, "RoActivateInstance")?,
                    ro_get_activation_factory: resolve(module, "RoGetActivationFactory")?,
                    windows_create_string: resolve(module, "WindowsCreateString")?,
                    windows_delete_string: resolve(module, "WindowsDeleteString")?,
                }
            };
            Ok((api, module))
        }

        /// Show one toast. `app_id` is the host's
        /// AppUserModelID (never empty — the caller checks);
        /// `xml` is a complete toast document from
        /// `crate::build_toast_xml`. Success is returned
        /// only when IToastNotifier::Show itself succeeded.
        pub fn show_toast(app_id: &str, xml: &str) -> super::PResult<()> {
            if app_id.is_empty() {
                return Err((
                    ERR_INVALID_ARG,
                    "an app_id is required for toasts".to_string(),
                ));
            }
            let (api, module) = load_api()?;
            // SAFETY: show_inner upholds the COM contract
            // (every pointer it obtains is released, every
            // HSTRING deleted, RoUninitialize called iff
            // this call initialized the thread).
            let result = unsafe { show_inner(&api, app_id, xml) };
            // SAFETY: `module` is the handle load_api
            // returned; freed exactly once, after all
            // resolved pointers have been used for the
            // last time inside show_inner.
            unsafe { super::FreeLibrary(module) };
            result
        }

        unsafe fn show_inner(api: &Api, app_id: &str, xml: &str) -> super::PResult<()> {
            let mut initialized_by_us = false;
            let mut hstrings: Vec<HString> = Vec::new();
            let mut objects: Vec<*mut c_void> = Vec::new();
            let result = run(
                api,
                app_id,
                xml,
                &mut initialized_by_us,
                &mut hstrings,
                &mut objects,
            );
            // Release in reverse acquisition order, on
            // every path out of run().
            for &p in objects.iter().rev() {
                release(p);
            }
            for &hs in &hstrings {
                // SAFETY: each HSTRING was created by
                // WindowsCreateString in run() and is
                // deleted exactly once here.
                (api.windows_delete_string)(hs);
            }
            if initialized_by_us {
                // SAFETY: pairs with the RoInitialize call
                // in run() that returned S_OK / S_FALSE.
                (api.ro_uninitialize)();
            }
            result
        }

        /// Release one interface pointer through its
        /// IUnknown vtable slot. Every WinRT interface
        /// vtable begins with IInspectable, whose first
        /// field is the IUnknown vtable, so this layout
        /// prefix is valid for all of them.
        unsafe fn release(p: *mut c_void) {
            if !p.is_null() {
                let vtbl = *(p as *const *const IUnknownVtbl);
                ((*vtbl).release)(p);
            }
        }

        unsafe fn query_interface(
            p: *mut c_void,
            iid: &Guid,
            what: &str,
        ) -> super::PResult<*mut c_void> {
            let vtbl = *(p as *const *const IUnknownVtbl);
            let mut out: *mut c_void = std::ptr::null_mut();
            let hr = ((*vtbl).query_interface)(p, iid, &mut out);
            check(what, hr)?;
            if out.is_null() {
                return Err((ERR_PLATFORM, format!("{what} returned a null pointer")));
            }
            Ok(out)
        }

        unsafe fn make_hstring(
            api: &Api,
            s: &str,
            hstrings: &mut Vec<HString>,
        ) -> super::PResult<HString> {
            let units: Vec<u16> = s.encode_utf16().collect();
            let mut hs: HString = std::ptr::null_mut();
            let hr = (api.windows_create_string)(units.as_ptr(), units.len() as u32, &mut hs);
            check("WindowsCreateString", hr)?;
            hstrings.push(hs);
            Ok(hs)
        }

        #[allow(clippy::too_many_arguments)]
        unsafe fn run(
            api: &Api,
            app_id: &str,
            xml: &str,
            initialized_by_us: &mut bool,
            hstrings: &mut Vec<HString>,
            objects: &mut Vec<*mut c_void>,
        ) -> super::PResult<()> {
            // 1. Initialize WinRT on this thread (MTA).
            //    RPC_E_CHANGED_MODE means the thread is
            //    already initialized with another model by
            //    its owner: proceed, but do not uninit.
            let hr = (api.ro_initialize)(RO_INIT_MULTITHREADED);
            if hr == S_OK || hr == S_FALSE {
                *initialized_by_us = true;
            } else if hr != RPC_E_CHANGED_MODE {
                return Err(hr_err("RoInitialize", hr));
            }

            // 2. Activate an empty XmlDocument and load
            //    the toast XML into it.
            let hs_class = make_hstring(api, "Windows.Data.Xml.Dom.XmlDocument", hstrings)?;
            let mut inspectable: *mut c_void = std::ptr::null_mut();
            let hr = (api.ro_activate_instance)(hs_class, &mut inspectable);
            check("RoActivateInstance(XmlDocument)", hr)?;
            if inspectable.is_null() {
                return Err((
                    ERR_PLATFORM,
                    "RoActivateInstance(XmlDocument) returned a null pointer".to_string(),
                ));
            }
            objects.push(inspectable);

            let xml_doc = query_interface(
                inspectable,
                &IID_XML_DOCUMENT,
                "QueryInterface(IXmlDocument)",
            )?;
            objects.push(xml_doc);
            let xml_io = query_interface(
                inspectable,
                &IID_XML_DOCUMENT_IO,
                "QueryInterface(IXmlDocumentIO)",
            )?;
            objects.push(xml_io);

            let hs_xml = make_hstring(api, xml, hstrings)?;
            let io_vtbl = *(xml_io as *const *const IXmlDocumentIoVtbl);
            let hr = ((*io_vtbl).load_xml)(xml_io, hs_xml);
            check("IXmlDocumentIO::LoadXml", hr)?;

            // 3. Create the toast notification from the
            //    document via its activation factory.
            let hs_toast =
                make_hstring(api, "Windows.UI.Notifications.ToastNotification", hstrings)?;
            let mut factory: *mut c_void = std::ptr::null_mut();
            let hr = (api.ro_get_activation_factory)(hs_toast, &IID_TOAST_FACTORY, &mut factory);
            check("RoGetActivationFactory(ToastNotification)", hr)?;
            if factory.is_null() {
                return Err((
                    ERR_PLATFORM,
                    "RoGetActivationFactory(ToastNotification) returned a null pointer".to_string(),
                ));
            }
            objects.push(factory);

            let factory_vtbl = *(factory as *const *const IToastNotificationFactoryVtbl);
            let mut toast: *mut c_void = std::ptr::null_mut();
            let hr = ((*factory_vtbl).create_toast_notification)(factory, xml_doc, &mut toast);
            check("IToastNotificationFactory::CreateToastNotification", hr)?;
            if toast.is_null() {
                return Err((
                    ERR_PLATFORM,
                    "CreateToastNotification returned a null pointer".to_string(),
                ));
            }
            objects.push(toast);

            // 4. Get a notifier bound to the host's
            //    app_id and show the toast.
            let hs_mgr = make_hstring(
                api,
                "Windows.UI.Notifications.ToastNotificationManager",
                hstrings,
            )?;
            let mut statics: *mut c_void = std::ptr::null_mut();
            let hr = (api.ro_get_activation_factory)(hs_mgr, &IID_MANAGER_STATICS, &mut statics);
            check("RoGetActivationFactory(ToastNotificationManager)", hr)?;
            if statics.is_null() {
                return Err((
                    ERR_PLATFORM,
                    "RoGetActivationFactory(ToastNotificationManager) returned a null pointer"
                        .to_string(),
                ));
            }
            objects.push(statics);

            let hs_app = make_hstring(api, app_id, hstrings)?;
            let statics_vtbl = *(statics as *const *const IToastNotificationManagerStaticsVtbl);
            let mut notifier: *mut c_void = std::ptr::null_mut();
            let hr =
                ((*statics_vtbl).create_toast_notifier_with_id)(statics, hs_app, &mut notifier);
            check("CreateToastNotifierWithId", hr)?;
            if notifier.is_null() {
                return Err((
                    ERR_PLATFORM,
                    "CreateToastNotifierWithId returned a null pointer".to_string(),
                ));
            }
            objects.push(notifier);

            let notifier_vtbl = *(notifier as *const *const IToastNotifierVtbl);
            let hr = ((*notifier_vtbl).show)(notifier, toast);
            check("IToastNotifier::Show", hr)?;
            Ok(())
        }
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
            app_id: String::new(),
            backend: Backend::Balloon,
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
        assert_eq!(
            tray_set_backend(&mut tray, Backend::Toast).unwrap_err().0,
            ERR_NOT_SUPPORTED
        );
        assert_eq!(
            tray_show_toast(&tray, "t", "b").unwrap_err().0,
            ERR_NOT_SUPPORTED
        );
    }

    #[test]
    fn tray_backend_state_defaults_to_balloon_and_switches() {
        // The backend state itself is pure handle state:
        // it defaults to the classic balloon and switches
        // exactly as set, even though applying it (a real
        // tray) is Windows-only.
        let mut tray = Tray {
            hwnd: 0,
            uid: 1,
            icon: Kind::App,
            tooltip: String::new(),
            app_id: "Contoso.MyApp".to_string(),
            backend: Backend::default(),
        };
        assert_eq!(tray.backend(), Backend::Balloon);
        tray.set_backend(Backend::Toast);
        assert_eq!(tray.backend(), Backend::Toast);
        assert_eq!(tray.backend().as_str(), "toast");
        tray.set_backend(Backend::Balloon);
        assert_eq!(tray.backend(), Backend::Balloon);
    }
}
