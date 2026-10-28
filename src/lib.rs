//! NotifyKit — pure logic core. Platform access lives in
//! `platform`; the C ABI lives in `ffi`. Keep this module
//! OS-independent and tested.
//!
//! Two halves:
//! - The notification *center* (this module): a queue + history
//!   with coalescing, for hosts that render their own UI. Pure
//!   logic, identical behaviour on every platform.
//! - The Windows tray half (`platform`): a real tray icon and
//!   classic balloon tips via Shell_NotifyIconW. On non-Windows
//!   targets those calls return ERR_NOT_SUPPORTED — they never
//!   fake success.

pub mod ffi;
pub mod json;
pub mod platform;

pub const VERSION: &str = "0.1.0";
pub const VERSION_MAJOR: u32 = 0;
pub const VERSION_MINOR: u32 = 1;
pub const VERSION_PATCH: u32 = 0;

// Error codes (mirror include/notifykit.h).
pub const NO_OK: i32 = 0;
pub const ERR_INVALID_ARG: i32 = 1;
pub const ERR_NOT_SUPPORTED: i32 = 2;
pub const ERR_NOT_FOUND: i32 = 3;
pub const ERR_ACCESS_DENIED: i32 = 4;
pub const ERR_PLATFORM: i32 = 5;
pub const ERR_INTERNAL: i32 = 6;

pub type NoResult<T> = Result<T, (i32, String)>;

fn invalid<T>(msg: impl Into<String>) -> NoResult<T> {
    Err((ERR_INVALID_ARG, msg.into()))
}

/// Escape a string for inclusion inside JSON double quotes.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Notification kinds
// ---------------------------------------------------------------------------

/// Notification kind. The same four values name the tray icon
/// kind in the tray half: `app` is the program's own icon
/// (IDI_APPLICATION), the others map to the matching Windows
/// stock icon / balloon severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    Info,
    Warn,
    Error,
}

impl Kind {
    pub fn parse(s: &str) -> NoResult<Kind> {
        match s {
            "app" => Ok(Kind::App),
            "info" => Ok(Kind::Info),
            "warn" => Ok(Kind::Warn),
            "error" => Ok(Kind::Error),
            other => invalid(format!(
                "unknown kind \"{other}\" (want app|info|warn|error)"
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::App => "app",
            Kind::Info => "info",
            Kind::Warn => "warn",
            Kind::Error => "error",
        }
    }
}

// ---------------------------------------------------------------------------
// UTF-16 text limits (shared with the tray half)
// ---------------------------------------------------------------------------

/// Total sizes of the classic tray text buffers, in UTF-16
/// code units INCLUDING the terminating NUL: szInfoTitle is
/// 64 units, szInfo 256, szTip 128 — so the visible budgets
/// are 63, 255 and 127 characters respectively.
pub const BALLOON_TITLE_MAX_UNITS: usize = 64;
pub const BALLOON_BODY_MAX_UNITS: usize = 256;
pub const TOOLTIP_MAX_UNITS: usize = 128;

/// Truncate `s` so it fits a fixed UTF-16 buffer of
/// `max_units_incl_nul` code units INCLUDING the terminating
/// NUL. Returns the (possibly shortened) string and whether
/// truncation happened. Cuts only on a char boundary: a
/// surrogate pair is a single Rust `char` of 2 units, so it
/// is kept whole or dropped whole — never split.
pub fn truncate_utf16(s: &str, max_units_incl_nul: usize) -> (String, bool) {
    let budget = max_units_incl_nul.saturating_sub(1);
    let mut used = 0usize;
    let mut end = s.len();
    let mut truncated = false;
    for (i, c) in s.char_indices() {
        let w = c.len_utf16();
        if used + w > budget {
            end = i;
            truncated = true;
            break;
        }
        used += w;
    }
    if truncated {
        (s[..end].to_string(), true)
    } else {
        (s.to_string(), false)
    }
}

/// Encode `s` as a NUL-terminated UTF-16 buffer of exactly
/// `capacity_units` code units (zero-padded), truncating on
/// a char boundary via [`truncate_utf16`]. The tray half
/// copies these buffers into the fixed Win32 arrays, so the
/// result can never overflow them.
pub fn utf16z(s: &str, capacity_units: usize) -> Vec<u16> {
    let (text, _) = truncate_utf16(s, capacity_units);
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    buf.resize(capacity_units, 0);
    buf
}

// ---------------------------------------------------------------------------
// Notification center (pure)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub kind: Kind,
    /// How many enqueues this entry represents (1 = just
    /// itself; coalesced repeats raise it).
    pub count: u64,
    pub first_ms: u64,
    pub last_ms: u64,
}

impl Entry {
    pub fn to_json(&self) -> json::Value {
        json::obj(vec![
            ("id", json::n(self.id)),
            ("title", json::s(&self.title)),
            ("body", json::s(&self.body)),
            ("kind", json::s(self.kind.as_str())),
            ("count", json::n(self.count)),
            ("first_ms", json::n(self.first_ms)),
            ("last_ms", json::n(self.last_ms)),
        ])
    }
}

#[derive(Clone, Debug)]
pub struct CenterConfig {
    /// Max pending entries in the queue (default 64).
    pub max_queue: usize,
    /// Max entries retained in history (default 256).
    pub max_history: usize,
    /// Repeats of the same title+body+kind arriving within
    /// this many ms of the pending entry's last occurrence
    /// merge into it (default 5000; 0 disables coalescing).
    pub coalesce_window_ms: u64,
}

impl Default for CenterConfig {
    fn default() -> Self {
        CenterConfig {
            max_queue: 64,
            max_history: 256,
            coalesce_window_ms: 5000,
        }
    }
}

/// Parse a center config JSON object. `None` and `"{}"` both
/// mean "all defaults". Unknown keys are rejected — a typo'd
/// key silently ignored is how configuration bugs hide.
pub fn parse_center_config(text: Option<&str>) -> NoResult<CenterConfig> {
    let mut cfg = CenterConfig::default();
    let Some(text) = text else { return Ok(cfg) };
    let v = json::parse(text).map_err(|e| (ERR_INVALID_ARG, format!("config JSON: {e}")))?;
    let obj = v
        .as_object()
        .ok_or_else(|| (ERR_INVALID_ARG, "config must be a JSON object".to_string()))?;
    for (k, val) in obj {
        match k.as_str() {
            "max_queue" => cfg.max_queue = config_usize(val, k, 1, 100_000)?,
            "max_history" => cfg.max_history = config_usize(val, k, 1, 100_000)?,
            "coalesce_window_ms" => cfg.coalesce_window_ms = config_u64(val, k, 0, 3_600_000)?,
            other => return invalid(format!("unknown config key \"{other}\"")),
        }
    }
    Ok(cfg)
}

fn config_usize(v: &json::Value, key: &str, min: usize, max: usize) -> NoResult<usize> {
    let n = v.as_i64().ok_or_else(|| {
        (
            ERR_INVALID_ARG,
            format!("\"{key}\" must be an integer, got {}", v.type_name()),
        )
    })?;
    if n < min as i64 || n > max as i64 {
        return invalid(format!(
            "\"{key}\" must be between {min} and {max}, got {n}"
        ));
    }
    Ok(n as usize)
}

fn config_u64(v: &json::Value, key: &str, min: u64, max: u64) -> NoResult<u64> {
    let n = v.as_i64().ok_or_else(|| {
        (
            ERR_INVALID_ARG,
            format!("\"{key}\" must be an integer, got {}", v.type_name()),
        )
    })?;
    if n < min as i64 || n > max as i64 {
        return invalid(format!(
            "\"{key}\" must be between {min} and {max}, got {n}"
        ));
    }
    Ok(n as u64)
}

fn system_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The pure notification center: a bounded pending queue plus
/// a bounded history, with coalescing of repeats. Not
/// internally synchronized — the FFI handle wraps it in a
/// mutex.
pub struct Center {
    cfg: CenterConfig,
    queue: std::collections::VecDeque<Entry>,
    history: std::collections::VecDeque<Entry>,
    next_id: u64,
    shown: u64,
    coalesced: u64,
    dropped: u64,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl Center {
    pub fn new(cfg: CenterConfig) -> Center {
        Center::with_clock(cfg, Box::new(system_now_ms))
    }

    /// A center with an injectable clock (milliseconds since
    /// any fixed epoch — only differences matter). Used by the
    /// test suite to exercise the coalescing window without
    /// sleeping, and available to hosts that run on their own
    /// time source.
    pub fn with_clock(cfg: CenterConfig, now: Box<dyn Fn() -> u64 + Send + Sync>) -> Center {
        Center {
            cfg,
            queue: std::collections::VecDeque::new(),
            history: std::collections::VecDeque::new(),
            next_id: 1,
            shown: 0,
            coalesced: 0,
            dropped: 0,
            now,
        }
    }

    /// Enqueue one notification from its JSON form:
    /// `{"title":"...","body":"...","kind":"info"}` — title
    /// and body required strings, kind optional (default
    /// "info"). Unknown keys are rejected.
    ///
    /// If a pending entry with the same title, body and kind
    /// was last seen within the coalesce window, the repeat
    /// merges into it (its count rises) instead of adding a
    /// new entry. Returns the result JSON:
    /// `{"id":N,"coalesced":bool,"count":N,"pending":N}`.
    pub fn enqueue_json(&mut self, text: &str) -> NoResult<String> {
        let v = json::parse(text).map_err(|e| (ERR_INVALID_ARG, format!("item JSON: {e}")))?;
        let obj = v
            .as_object()
            .ok_or_else(|| (ERR_INVALID_ARG, "item must be a JSON object".to_string()))?;
        let mut title: Option<String> = None;
        let mut body: Option<String> = None;
        let mut kind = Kind::Info;
        for (k, val) in obj {
            match k.as_str() {
                "title" => {
                    title = Some(
                        val.as_str()
                            .ok_or_else(|| {
                                (ERR_INVALID_ARG, "\"title\" must be a string".to_string())
                            })?
                            .to_string(),
                    )
                }
                "body" => {
                    body = Some(
                        val.as_str()
                            .ok_or_else(|| {
                                (ERR_INVALID_ARG, "\"body\" must be a string".to_string())
                            })?
                            .to_string(),
                    )
                }
                "kind" => {
                    let s = val.as_str().ok_or_else(|| {
                        (ERR_INVALID_ARG, "\"kind\" must be a string".to_string())
                    })?;
                    kind = Kind::parse(s)?;
                }
                other => return invalid(format!("unknown item key \"{other}\"")),
            }
        }
        let title = title.ok_or_else(|| (ERR_INVALID_ARG, "\"title\" is required".to_string()))?;
        let body = body.ok_or_else(|| (ERR_INVALID_ARG, "\"body\" is required".to_string()))?;
        Ok(self.enqueue(&title, &body, kind).to_json())
    }

    /// Enqueue one notification. See `enqueue_json` for the
    /// coalescing rule. Returns the entry it landed on (new or
    /// merged).
    pub fn enqueue(&mut self, title: &str, body: &str, kind: Kind) -> EnqueueOutcome {
        let now = (self.now)();
        if self.cfg.coalesce_window_ms > 0 {
            if let Some(e) = self.queue.iter_mut().find(|e| {
                e.title == title
                    && e.body == body
                    && e.kind == kind
                    && now.saturating_sub(e.last_ms) <= self.cfg.coalesce_window_ms
            }) {
                e.count += 1;
                e.last_ms = now;
                self.coalesced += 1;
                return EnqueueOutcome {
                    id: e.id,
                    coalesced: true,
                    count: e.count,
                    pending: self.queue.len() as u64,
                };
            }
        }
        let entry = Entry {
            id: self.next_id,
            title: title.to_string(),
            body: body.to_string(),
            kind,
            count: 1,
            first_ms: now,
            last_ms: now,
        };
        self.next_id += 1;
        self.shown += 1;
        // Bounded queue: a full queue drops its OLDEST pending
        // entry to make room. The drop is counted, never
        // silent.
        if self.queue.len() >= self.cfg.max_queue {
            self.queue.pop_front();
            self.dropped += 1;
        }
        let outcome = EnqueueOutcome {
            id: entry.id,
            coalesced: false,
            count: 1,
            pending: self.queue.len() as u64 + 1,
        };
        self.queue.push_back(entry);
        outcome
    }

    /// Drain all pending entries: they move to history
    /// (bounded — oldest history entries drop out, counted)
    /// and are returned as `{"events":[...],"count":N}`.
    pub fn poll_events(&mut self) -> String {
        let drained: Vec<Entry> = self.queue.drain(..).collect();
        for e in &drained {
            self.history.push_back(e.clone());
        }
        while self.history.len() > self.cfg.max_history {
            self.history.pop_front();
            self.dropped += 1;
        }
        json::obj(vec![
            (
                "events",
                json::Value::Array(drained.iter().map(|e| e.to_json()).collect()),
            ),
            ("count", json::n(drained.len() as u64)),
        ])
        .to_json()
    }

    /// History without draining:
    /// `{"entries":[...],"count":N}`, oldest first.
    pub fn history_json(&self) -> String {
        json::obj(vec![
            (
                "entries",
                json::Value::Array(self.history.iter().map(|e| e.to_json()).collect()),
            ),
            ("count", json::n(self.history.len() as u64)),
        ])
        .to_json()
    }

    /// Counters: `shown` distinct notifications accepted,
    /// `coalesced` repeats merged away, `dropped` entries
    /// lost to the queue/history bounds, plus current sizes.
    pub fn stats_json(&self) -> String {
        json::obj(vec![
            ("shown", json::n(self.shown)),
            ("coalesced", json::n(self.coalesced)),
            ("dropped", json::n(self.dropped)),
            ("pending", json::n(self.queue.len() as u64)),
            ("history", json::n(self.history.len() as u64)),
        ])
        .to_json()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnqueueOutcome {
    pub id: u64,
    pub coalesced: bool,
    pub count: u64,
    pub pending: u64,
}

impl EnqueueOutcome {
    pub fn to_json(&self) -> String {
        json::obj(vec![
            ("id", json::n(self.id)),
            ("coalesced", json::b(self.coalesced)),
            ("count", json::n(self.count)),
            ("pending", json::n(self.pending)),
        ])
        .to_json()
    }
}

// ---------------------------------------------------------------------------
// Tray config / show request parsing (pure; used by the FFI
// before it touches the platform layer)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct TrayConfig {
    /// Host window handle as an integer (HWND). 0 = no host
    /// window: the icon is registered icon-only, identified
    /// by `uid`; no callback messages can be delivered.
    pub hwnd: u64,
    /// Icon identifier (uID) the host uses for this icon.
    pub uid: u32,
    pub tooltip: String,
    pub icon: Kind,
}

impl Default for TrayConfig {
    fn default() -> Self {
        TrayConfig {
            hwnd: 0,
            uid: 1,
            tooltip: String::new(),
            icon: Kind::App,
        }
    }
}

/// Parse a tray config JSON object: `{"hwnd":N,"uid":N,
/// "tooltip":"...","icon":"app|info|warn|error"}` — every
/// key optional. `None` and `"{}"` mean all defaults.
/// Unknown keys are rejected.
pub fn parse_tray_config(text: Option<&str>) -> NoResult<TrayConfig> {
    let mut cfg = TrayConfig::default();
    let Some(text) = text else { return Ok(cfg) };
    let v = json::parse(text).map_err(|e| (ERR_INVALID_ARG, format!("config JSON: {e}")))?;
    let obj = v
        .as_object()
        .ok_or_else(|| (ERR_INVALID_ARG, "config must be a JSON object".to_string()))?;
    for (k, val) in obj {
        match k.as_str() {
            "hwnd" => {
                let n = val
                    .as_i64()
                    .ok_or_else(|| (ERR_INVALID_ARG, "\"hwnd\" must be an integer".to_string()))?;
                if n < 0 {
                    return invalid("\"hwnd\" must be >= 0");
                }
                cfg.hwnd = n as u64;
            }
            "uid" => cfg.uid = config_usize(val, k, 0, 100_000)? as u32,
            "tooltip" => {
                cfg.tooltip = val
                    .as_str()
                    .ok_or_else(|| (ERR_INVALID_ARG, "\"tooltip\" must be a string".to_string()))?
                    .to_string()
            }
            "icon" => {
                let s = val
                    .as_str()
                    .ok_or_else(|| (ERR_INVALID_ARG, "\"icon\" must be a string".to_string()))?;
                cfg.icon = Kind::parse(s)?;
            }
            other => return invalid(format!("unknown config key \"{other}\"")),
        }
    }
    Ok(cfg)
}

#[derive(Clone, Debug)]
pub struct ShowRequest {
    pub title: String,
    pub body: String,
    pub kind: Kind,
    /// Balloon display time in ms; 0 = library default
    /// (10 000). The OS may clamp the real display time.
    pub timeout_ms: u32,
}

/// Parse a tray show request JSON object:
/// `{"title":"...","body":"...","kind":"info",
/// "timeout_ms":N}` — title and body required strings, kind
/// optional (default "info"), timeout_ms optional (default
/// 0). Unknown keys are rejected.
pub fn parse_show_request(text: &str) -> NoResult<ShowRequest> {
    let v = json::parse(text).map_err(|e| (ERR_INVALID_ARG, format!("show JSON: {e}")))?;
    let obj = v.as_object().ok_or_else(|| {
        (
            ERR_INVALID_ARG,
            "show request must be a JSON object".to_string(),
        )
    })?;
    let mut title: Option<String> = None;
    let mut body: Option<String> = None;
    let mut kind = Kind::Info;
    let mut timeout_ms: u32 = 0;
    for (k, val) in obj {
        match k.as_str() {
            "title" => {
                title = Some(
                    val.as_str()
                        .ok_or_else(|| (ERR_INVALID_ARG, "\"title\" must be a string".to_string()))?
                        .to_string(),
                )
            }
            "body" => {
                body = Some(
                    val.as_str()
                        .ok_or_else(|| (ERR_INVALID_ARG, "\"body\" must be a string".to_string()))?
                        .to_string(),
                )
            }
            "kind" => {
                let s = val
                    .as_str()
                    .ok_or_else(|| (ERR_INVALID_ARG, "\"kind\" must be a string".to_string()))?;
                kind = Kind::parse(s)?;
            }
            "timeout_ms" => timeout_ms = config_usize(val, k, 0, 600_000)? as u32,
            other => return invalid(format!("unknown show key \"{other}\"")),
        }
    }
    Ok(ShowRequest {
        title: title.ok_or_else(|| (ERR_INVALID_ARG, "\"title\" is required".to_string()))?,
        body: body.ok_or_else(|| (ERR_INVALID_ARG, "\"body\" is required".to_string()))?,
        kind,
        timeout_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    fn center(cfg: CenterConfig, start: u64) -> (Arc<AtomicU64>, Center) {
        let t = Arc::new(AtomicU64::new(start));
        let t2 = t.clone();
        (
            t,
            Center::with_clock(cfg, Box::new(move || t2.load(Ordering::SeqCst))),
        )
    }

    #[test]
    fn escapes_json() {
        assert_eq!(json_escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn coalescing_merges_repeats_and_counts() {
        let (_t, mut c) = center(CenterConfig::default(), 1_000);
        let a = c.enqueue("Build done", "3 warnings", Kind::Info);
        let b = c.enqueue("Build done", "3 warnings", Kind::Info);
        let d = c.enqueue("Build done", "3 warnings", Kind::Info);
        assert!(!a.coalesced);
        assert!(b.coalesced && d.coalesced);
        assert_eq!(a.id, b.id);
        assert_eq!(d.count, 3);
        assert_eq!(d.pending, 1);
        let stats = json::parse(&c.stats_json()).unwrap();
        assert_eq!(stats.get("shown").unwrap().as_i64(), Some(1));
        assert_eq!(stats.get("coalesced").unwrap().as_i64(), Some(2));
        let events = json::parse(&c.poll_events()).unwrap();
        let ev = events.get("events").unwrap().as_array().unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].get("count").unwrap().as_i64(), Some(3));
    }

    #[test]
    fn coalescing_window_expiry_separates() {
        let cfg = CenterConfig {
            coalesce_window_ms: 5_000,
            ..CenterConfig::default()
        };
        let (t, mut c) = center(cfg, 10_000);
        let a = c.enqueue("T", "B", Kind::Info);
        t.store(14_999, Ordering::SeqCst); // 4 999 ms later: inside
        let b = c.enqueue("T", "B", Kind::Info);
        assert!(b.coalesced, "repeat inside the window must merge");
        t.store(20_001, Ordering::SeqCst); // 5 002 ms after last: outside
        let d = c.enqueue("T", "B", Kind::Info);
        assert!(!d.coalesced, "repeat after the window must be a new entry");
        assert_ne!(a.id, d.id);
        let stats = json::parse(&c.stats_json()).unwrap();
        assert_eq!(stats.get("shown").unwrap().as_i64(), Some(2));
        assert_eq!(stats.get("coalesced").unwrap().as_i64(), Some(1));
        assert_eq!(stats.get("pending").unwrap().as_i64(), Some(2));
    }

    #[test]
    fn coalescing_needs_same_title_body_and_kind() {
        let (_t, mut c) = center(CenterConfig::default(), 0);
        c.enqueue("T", "B", Kind::Info);
        assert!(!c.enqueue("T", "B", Kind::Error).coalesced);
        assert!(!c.enqueue("T", "B2", Kind::Info).coalesced);
        assert!(!c.enqueue("T2", "B", Kind::Info).coalesced);
        let stats = json::parse(&c.stats_json()).unwrap();
        assert_eq!(stats.get("pending").unwrap().as_i64(), Some(4));
    }

    #[test]
    fn coalescing_disabled_at_zero_window() {
        let cfg = CenterConfig {
            coalesce_window_ms: 0,
            ..CenterConfig::default()
        };
        let (_t, mut c) = center(cfg, 0);
        c.enqueue("T", "B", Kind::Info);
        assert!(!c.enqueue("T", "B", Kind::Info).coalesced);
    }

    #[test]
    fn history_does_not_coalesce() {
        let (_t, mut c) = center(CenterConfig::default(), 0);
        c.enqueue("T", "B", Kind::Info);
        c.poll_events(); // entry is now history, not pending
        let out = c.enqueue("T", "B", Kind::Info);
        assert!(
            !out.coalesced,
            "a drained entry must not absorb new repeats"
        );
    }

    #[test]
    fn queue_bound_drops_oldest_and_counts() {
        let cfg = CenterConfig {
            max_queue: 2,
            ..CenterConfig::default()
        };
        let (_t, mut c) = center(cfg, 0);
        c.enqueue("A", "1", Kind::Info);
        c.enqueue("B", "2", Kind::Info);
        c.enqueue("C", "3", Kind::Info);
        let stats = json::parse(&c.stats_json()).unwrap();
        assert_eq!(stats.get("dropped").unwrap().as_i64(), Some(1));
        assert_eq!(stats.get("pending").unwrap().as_i64(), Some(2));
        let events = json::parse(&c.poll_events()).unwrap();
        let ev = events.get("events").unwrap().as_array().unwrap();
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].get("title").unwrap().as_str(), Some("B"));
        assert_eq!(ev[1].get("title").unwrap().as_str(), Some("C"));
    }

    #[test]
    fn history_bound_drops_oldest_and_counts() {
        let cfg = CenterConfig {
            max_history: 2,
            ..CenterConfig::default()
        };
        let (_t, mut c) = center(cfg, 0);
        for i in 0..3 {
            c.enqueue(&format!("T{i}"), "B", Kind::Info);
            c.poll_events();
        }
        let hist = json::parse(&c.history_json()).unwrap();
        let entries = hist.get("entries").unwrap().as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].get("title").unwrap().as_str(), Some("T1"));
        assert_eq!(entries[1].get("title").unwrap().as_str(), Some("T2"));
        let stats = json::parse(&c.stats_json()).unwrap();
        assert_eq!(stats.get("dropped").unwrap().as_i64(), Some(1));
        assert_eq!(stats.get("history").unwrap().as_i64(), Some(2));
    }

    #[test]
    fn poll_drains_and_history_keeps_entries() {
        let (_t, mut c) = center(CenterConfig::default(), 42);
        c.enqueue("A", "1", Kind::Warn);
        let first = json::parse(&c.poll_events()).unwrap();
        assert_eq!(first.get("count").unwrap().as_i64(), Some(1));
        let ev = first.get("events").unwrap().as_array().unwrap();
        assert_eq!(ev[0].get("kind").unwrap().as_str(), Some("warn"));
        assert_eq!(ev[0].get("first_ms").unwrap().as_i64(), Some(42));
        let second = json::parse(&c.poll_events()).unwrap();
        assert_eq!(second.get("count").unwrap().as_i64(), Some(0));
        let hist = json::parse(&c.history_json()).unwrap();
        assert_eq!(hist.get("count").unwrap().as_i64(), Some(1));
    }

    #[test]
    fn center_config_validation() {
        let cfg = parse_center_config(None).unwrap();
        assert_eq!(cfg.max_queue, 64);
        let cfg = parse_center_config(Some(
            "{\"max_queue\":2,\"max_history\":3,\"coalesce_window_ms\":0}",
        ))
        .unwrap();
        assert_eq!(
            (cfg.max_queue, cfg.max_history, cfg.coalesce_window_ms),
            (2, 3, 0)
        );
        assert_eq!(
            parse_center_config(Some("{\"max_queue\":0}"))
                .unwrap_err()
                .0,
            ERR_INVALID_ARG
        );
        assert_eq!(
            parse_center_config(Some("{\"max_queue\":\"x\"}"))
                .unwrap_err()
                .0,
            ERR_INVALID_ARG
        );
        assert!(parse_center_config(Some("{\"bogus\":1}")).is_err());
        assert!(parse_center_config(Some("{\"max_queue\":1,\"max_queue\":2}")).is_err());
        assert!(parse_center_config(Some("[1]")).is_err());
        assert!(parse_center_config(Some("not json")).is_err());
    }

    #[test]
    fn enqueue_json_validation() {
        let (_t, mut c) = center(CenterConfig::default(), 0);
        let out = c.enqueue_json("{\"title\":\"T\",\"body\":\"B\"}").unwrap();
        let v = json::parse(&out).unwrap();
        assert_eq!(v.get("id").unwrap().as_i64(), Some(1));
        assert!(c.enqueue_json("{\"body\":\"B\"}").is_err()); // no title
        assert!(c.enqueue_json("{\"title\":\"T\"}").is_err()); // no body
        assert!(c
            .enqueue_json("{\"title\":\"T\",\"body\":\"B\",\"kind\":\"loud\"}")
            .is_err());
        assert!(c
            .enqueue_json("{\"title\":\"T\",\"body\":\"B\",\"extra\":1}")
            .is_err());
        assert!(c
            .enqueue_json("{\"title\":\"T\",\"title\":\"U\",\"body\":\"B\"}")
            .is_err()); // duplicate key = parse error
    }

    #[test]
    fn tray_and_show_parsing() {
        let cfg = parse_tray_config(Some("{\"hwnd\":1234,\"uid\":7,\"icon\":\"warn\"}")).unwrap();
        assert_eq!(cfg.hwnd, 1234);
        assert_eq!(cfg.uid, 7);
        assert_eq!(cfg.icon, Kind::Warn);
        let dflt = parse_tray_config(None).unwrap();
        assert_eq!((dflt.hwnd, dflt.uid, dflt.icon), (0, 1, Kind::App));
        assert!(parse_tray_config(Some("{\"hwnd\":-1}")).is_err());
        assert!(parse_tray_config(Some("{\"icon\":\"shiny\"}")).is_err());
        let req =
            parse_show_request("{\"title\":\"T\",\"body\":\"B\",\"timeout_ms\":3000}").unwrap();
        assert_eq!(req.timeout_ms, 3000);
        assert_eq!(req.kind, Kind::Info);
        assert!(parse_show_request("{\"title\":\"T\"}").is_err());
        assert!(parse_show_request("{\"title\":\"T\",\"body\":\"B\",\"timeout_ms\":-5}").is_err());
    }

    #[test]
    fn truncation_exact_fit_and_overflow() {
        let exact = "a".repeat(63);
        let (out, tr) = truncate_utf16(&exact, BALLOON_TITLE_MAX_UNITS);
        assert_eq!(out, exact);
        assert!(!tr);
        let over = "a".repeat(64);
        let (out, tr) = truncate_utf16(&over, BALLOON_TITLE_MAX_UNITS);
        assert_eq!(out.chars().count(), 63);
        assert!(tr);
        let (out, tr) = truncate_utf16("hello", TOOLTIP_MAX_UNITS);
        assert_eq!(out, "hello");
        assert!(!tr);
        let buf = utf16z(&"b".repeat(300), BALLOON_BODY_MAX_UNITS);
        assert_eq!(buf.len(), BALLOON_BODY_MAX_UNITS);
        assert_eq!(buf[255], 0, "last unit must be the NUL");
        assert!(buf[..255].iter().all(|&u| u == 'b' as u16));
    }

    #[test]
    fn truncation_never_splits_a_surrogate_pair() {
        // 62 BMP chars + one astral char (2 units) = 64 units of
        // content for a 63-unit visible budget: the astral char
        // does not fit whole, so it is dropped whole.
        let s = format!("{}{}", "a".repeat(62), '\u{1F600}');
        let (out, tr) = truncate_utf16(&s, BALLOON_TITLE_MAX_UNITS);
        assert!(tr);
        assert_eq!(out, "a".repeat(62));
        assert_eq!(out.encode_utf16().count(), 62);
        // 61 + 2 = exactly 63: the pair fits and stays.
        let s = format!("{}{}", "a".repeat(61), '\u{1F600}');
        let (out, tr) = truncate_utf16(&s, BALLOON_TITLE_MAX_UNITS);
        assert!(!tr);
        assert_eq!(out, s);
        assert_eq!(out.encode_utf16().count(), 63);
        // A lone astral char in a 1-visible-unit budget drops out.
        let (out, tr) = truncate_utf16("\u{1F600}", 2);
        assert!(tr);
        assert_eq!(out, "");
        // The fixed-buffer form agrees: no half-pair at the cut.
        let buf = utf16z(&format!("{}{}", "a".repeat(62), '\u{1F600}'), 64);
        assert_eq!(buf[62], 0);
        assert_eq!(buf.len(), 64);
    }
}
