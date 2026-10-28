# NotifyKit

## TL;DR

NotifyKit is one small DLL that handles "a program needs to tell the user something" in two ways. Half of it is a waiting-room for notifications: your program drops messages in, and if the same message keeps arriving ("build finished, 3 warnings" five times in a row) it politely merges the repeats into one entry with a counter instead of shouting five times. The other half talks to the real Windows system tray: it registers a tray icon and pops up the classic balloon tips using only built-in Windows icons — or, since v0.2, modern Windows toast notifications if you select the toast backend, no extra files, no dependencies.

```c
#include "notifykit.h"
#include <stdio.h>

int main(void) {
    /* The center: queue, coalescing, history - every platform. */
    NoCenter *center = NULL;
    no_create("{\"coalesce_window_ms\":5000}", &center);
    char *out = NULL;
    no_enqueue(center, "{\"title\":\"Build done\",\"body\":\"3 warnings\"}", &out);
    no_free_string(out);
    no_enqueue(center, "{\"title\":\"Build done\",\"body\":\"3 warnings\"}", &out);
    printf("%s\n", out);   /* {"id":1,"coalesced":true,"count":2,...} */
    no_free_string(out);
    no_poll_events(center, &out);
    printf("%s\n", out);   /* one event, "count":2 */
    no_free_string(out);
    no_close(center);

    /* The tray: a real icon + balloon on Windows. */
    NoTray *tray = NULL;
    if (no_tray_create("{\"hwnd\":0,\"tooltip\":\"My app\"}", &tray) == NO_OK) {
        no_tray_show(tray,
                     "{\"title\":\"Build done\",\"body\":\"3 warnings\"}",
                     &out);
        printf("%s\n", out);  /* {"shown":true,...} */
        no_free_string(out);
        no_tray_remove(tray);
    }
    return 0;
}
```

## Key features

- **Coalescing with a window.** A repeat of the same title + body + kind arriving within the configured window (default 5 seconds) merges into the pending entry and raises its count, instead of adding another entry. After the window, the same message starts a fresh entry. A repeat of an entry that was already drained to history never merges backwards into it.
- **Bounded everything, drops counted.** The pending queue (default 64) and the history (default 256) are bounded. When a bound is hit the oldest entry drops out — and the drop is counted in `no_stats` under `dropped`, never silent.
- **A center that works everywhere.** The queue/history/stats half is pure logic with identical behaviour on any platform, for hosts that draw their own notification UI.
- **A real tray icon on Windows.** `no_tray_create` registers an icon with Shell_NotifyIconW (NIM_ADD); `no_tray_show` displays a balloon (NIM_MODIFY + NIF_INFO); `no_tray_set_tooltip` updates the tooltip; `no_tray_remove` deletes the icon (NIM_DELETE). Icons are the Windows stock icons loaded with LoadIconW — no icon files to ship.
- **A WinRT toast backend (v0.2).** `no_tray_set_backend(handle, "toast")` switches a tray handle from classic balloons to modern toast notifications — the kind that persist in Action Center. The toast carries the same title and body as a balloon show, in full (no fixed-buffer truncation), built as a ToastGeneric XML document with all markup characters escaped. Toasts are shown under your app's own AppUserModelID: pass it as `"app_id"` in the tray config. There is no borrowed identity — without an `app_id`, a toast show is an argument error, not a toast shown as some other app. The balloon backend stays the default and the fallback for hosts that cannot use WinRT.
- **The window belongs to the host.** The tray half takes the host's window handle (HWND) as a plain integer in the config JSON. The host creates and owns its window as usual; tray callbacks arrive there as message 0x0464 (WM_USER+100). Passing `"hwnd":0` registers an icon-only icon: show/tooltip/remove work, callbacks have nowhere to go.
- **Real text limits, reported honestly.** The classic buffers are fixed: title 63 visible UTF-16 units, body 255, tooltip 127. Longer text is truncated on a character boundary — a surrogate pair is never split — and the result JSON says `"truncated":true` (per field) instead of pretending the whole text was shown.
- **No fake success.** On non-Windows platforms the tray calls return NO_ERR_NOT_SUPPORTED. On Windows, a show call only reports `"shown":true` when Shell_NotifyIconW itself succeeded; failures return NO_ERR_PLATFORM with the GetLastError value in the message.
- **Zero dependencies.** Hand-rolled JSON (duplicate keys in one object are a parse error, by design), hand-declared Win32 bindings, small C ABI.

## The API in one screen

| Call | What it does |
|---|---|
| `no_create(config_json, &handle)` | Create a notification center (queue/history/coalescing config) |
| `no_enqueue(handle, item_json, &out)` | Add a notification; result says whether it merged and its count |
| `no_poll_events(handle, &out)` | Drain pending entries as JSON (they move to history) |
| `no_history(handle, &out)` | Read history without draining |
| `no_stats(handle, &out)` | shown / coalesced / dropped counters + current sizes |
| `no_close(handle)` | Free a center handle |
| `no_tray_create(config_json, &handle)` | Register a tray icon (Windows; host HWND as integer, or 0 = icon-only; `"app_id"` for the toast backend) |
| `no_tray_set_backend(handle, name)` | Select the show backend for a handle: `"balloon"` (default) or `"toast"` |
| `no_tray_show(handle, show_json, &out)` | Show a notification via the handle's backend; result reports shown + per-field truncation (always false for toasts) |
| `no_tray_set_tooltip(handle, tooltip, &out)` | Replace the tooltip; result reports the text set + truncation |
| `no_tray_remove(handle)` | Delete the icon and free the handle |
| `no_version` / `no_last_error` / `no_last_error_code` / `no_free_string` | Version, thread-local error, string ownership |

All functions return NO_OK (0) or a NO_ERR_* code; JSON results come back through out-pointers as caller-owned strings.

## Build

```sh
cargo build --release   # notify_kit.dll / .so / .a + include/notifykit.h
cargo test              # full test suite
```

Rust 1.80+, edition 2021, no dependencies.

## Limitations, honestly

- **Toasts need a registered app identity.** Windows delivers a toast only for an AppUserModelID the host app has registered — in practice, the host app must have its AppUserModelID registered, e.g. via its installer or a Start menu shortcut carrying that AppUserModelID, for Windows to deliver the toast. NotifyKit takes the ID as `"app_id"` in the tray config and shows the toast under exactly that identity; it never substitutes another app's. Toast buttons and images are not exposed yet — a toast show sends title + body text only. The toast runtime path is new in v0.2 and is **Unverified on a real desktop** until a field test shows one (the code is windows-target checked and clippy-clean at the gate; the gate does not link or run it).
- **Balloons honour the user's settings.** On current Windows versions the classic balloon is rendered by the system in its own style and honours the user's notification settings — a user who has disabled app notifications will not see balloons, and the API cannot detect or override that.
- **The tray half is Windows-only.** On other platforms it returns NO_ERR_NOT_SUPPORTED; the center half works everywhere.
- **No callback delivery without a host window.** With `"hwnd":0` there is no window to receive tray mouse/balloon events. Hosts that need clicks must pass a real HWND and handle message 0x0464 in their own window procedure.
- **Timeouts are requests.** `timeout_ms` is passed to the shell, but Windows reserves the right to clamp the actual balloon display time.
- **Text is truncated, not wrapped or paged.** Text past the classic limits is cut (on a character boundary) and reported as truncated; splitting a long message into several balloons is the host's choice, not done automatically.
- **One icon per tray handle, one balloon at a time.** Showing a new balloon replaces the one currently displayed for that icon — that is how the classic API behaves.
- **The center is in-memory.** Queue and history live for the lifetime of the handle; nothing is persisted to disk.

## Tests

26 tests: coalescing (merges + count, window edge inside/outside, kind/body/title must match, disabled at window 0, history does not coalesce), queue and history bounds with oldest-drop and drop counts, poll draining, config/item/tray/show JSON validation (including duplicate keys as parse errors), UTF-16 truncation (exact fit, overflow, surrogate pair never split at the cut, fixed-buffer form), toast XML construction (exact ToastGeneric shape, all five markup characters escaped, unrepresentable control characters stripped), backend selection (parse + defaults + per-handle state), tray `app_id` config parsing, the JSON engine itself, FFI end-to-end for the center, FFI argument validation, and — on non-Windows — the tray half returning NO_ERR_NOT_SUPPORTED at both the platform and FFI layers. The Windows tray calls are exercised by the Windows CI build at link level; an on-screen balloon or toast is verified only by a field test on a real desktop.

## License

MIT. Part of the WinDevKit family — see `AGENTS.md` for build discipline and `ROADMAP.md` for what is next.
