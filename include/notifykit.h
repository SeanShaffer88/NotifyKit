/* NotifyKit.dll - tray notifications and balloon alerts
 * without WinRT, behind a small C ABI. Two halves:
 *
 * 1. A notification CENTER (all platforms): a bounded queue
 *    plus a bounded history with coalescing. The host renders
 *    notifications itself; the center decides when a repeat
 *    of the same notification merges into the pending entry
 *    (raising its count) instead of piling up.
 *
 * 2. A Windows TRAY half: one real tray icon per handle
 *    (Shell_NotifyIconW), with classic balloon tips - or,
 *    since v0.2, modern WinRT toast notifications when the
 *    handle's backend is set to "toast" with
 *    no_tray_set_backend(). On non-Windows platforms the
 *    tray calls return NO_ERR_NOT_SUPPORTED - they never
 *    report a notification as shown when no platform call
 *    happened.
 *
 * Strings: all strings are UTF-8, NUL-terminated.
 *
 * Ownership: every char* result written through an
 * out-pointer is caller-owned and must be freed with
 * no_free_string(). no_version() and no_last_error() return
 * borrowed strings - do NOT free them.
 *
 * Errors: functions return NO_OK (0) on success or a
 * NO_ERR_* code; on failure the out-pointer is set to NULL
 * and no_last_error() / no_last_error_code() describe the
 * failure on the calling thread.
 *
 * Threading: handles are opaque and thread-safe - calls on
 * one handle are serialized internally. no_close() frees a
 * center handle; a successful no_tray_remove() frees a tray
 * handle. Using a handle after it is freed is a caller bug.
 */
#ifndef NOTIFYKIT_H
#define NOTIFYKIT_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#if defined(_WIN32) && defined(NO_BUILD)
#define NO_API __declspec(dllexport)
#elif defined(_WIN32)
#define NO_API __declspec(dllimport)
#else
#define NO_API
#endif

#define NO_OK 0
#define NO_ERR_INVALID_ARG 1
#define NO_ERR_NOT_SUPPORTED 2
#define NO_ERR_NOT_FOUND 3
#define NO_ERR_ACCESS_DENIED 4
#define NO_ERR_PLATFORM 5
#define NO_ERR_INTERNAL 6

typedef struct NoCenter NoCenter;
typedef struct NoTray NoTray;

/* Library version, e.g. "0.2.0". Borrowed string; do not free. */
NO_API const char *no_version(void);

/* Last error message on this thread ("" when none). Borrowed. */
NO_API const char *no_last_error(void);

/* Last NO_ERR_* code on this thread (NO_OK when none). */
NO_API int no_last_error_code(void);

/* Free a caller-owned string returned by this library.
 * NULL is accepted and does nothing. */
NO_API void no_free_string(char *p);

/* ------------------------------------------------------------------ */
/* Notification center (all platforms)                                 */
/* ------------------------------------------------------------------ */

/* Create a notification center.
 *
 * config_json: NULL or one JSON object:
 *   {"max_queue":N,          (default 64, 1..100000)
 *    "max_history":N,        (default 256, 1..100000)
 *    "coalesce_window_ms":N} (default 5000, 0..3600000;
 *                             0 disables coalescing)
 * Unknown keys are rejected; duplicate keys in one object
 * are a parse error.
 *
 * On success *out_handle receives the handle and NO_OK is
 * returned; on failure *out_handle is NULL. */
NO_API int no_create(const char *config_json, NoCenter **out_handle);

/* Enqueue one notification:
 *   {"title":"...", "body":"...", "kind":"info"}
 * title and body are required strings; kind is one of
 * "app"|"info"|"warn"|"error" (default "info").
 *
 * Coalescing: if a PENDING entry with the same title, body
 * and kind was last seen within the configured window, the
 * repeat merges into it instead of creating an entry.
 *
 * *out_json receives caller-owned result JSON:
 *   {"id":N, "coalesced":bool, "count":N, "pending":N}
 * "count" is how many enqueues the entry now represents. */
NO_API int no_enqueue(NoCenter *handle, const char *item_json,
                      char **out_json);

/* Drain all pending entries. They move to history (bounded:
 * the oldest history entries drop out and are counted in
 * "dropped").
 *
 * *out_json receives caller-owned JSON:
 *   {"events":[{"id":N, "title":"...", "body":"...",
 *               "kind":"info", "count":N,
 *               "first_ms":N, "last_ms":N}, ...],
 *    "count":N}
 * first_ms / last_ms are milliseconds from the center's
 * clock (system wall clock by default). */
NO_API int no_poll_events(NoCenter *handle, char **out_json);

/* Read history without draining it:
 *   {"entries":[ ... same entry shape as no_poll_events ... ],
 *    "count":N}
 * Oldest first. */
NO_API int no_history(NoCenter *handle, char **out_json);

/* Counters:
 *   {"shown":N, "coalesced":N, "dropped":N,
 *    "pending":N, "history":N}
 * shown = distinct notifications accepted; coalesced =
 * repeats merged away; dropped = entries lost to the queue
 * or history bounds (oldest first). The bounds drop openly
 * and are always counted - nothing vanishes silently. */
NO_API int no_stats(NoCenter *handle, char **out_json);

/* Free a center handle. Returns NO_OK, or
 * NO_ERR_INVALID_ARG for a NULL handle. */
NO_API int no_close(NoCenter *handle);

/* ------------------------------------------------------------------ */
/* Windows tray half (NO_ERR_NOT_SUPPORTED elsewhere)                  */
/* ------------------------------------------------------------------ */

/* Register a tray icon (Shell_NotifyIconW NIM_ADD).
 *
 * config_json: NULL or one JSON object, every key optional:
 *   {"hwnd":N,        host window handle as an integer
 *                     (HWND); default 0 - see below
 *    "uid":N,         icon identifier (uID); default 1
 *    "tooltip":"...", initial tooltip; truncated to 127
 *                     UTF-16 units if longer
 *    "icon":"app|info|warn|error",  stock icon; default "app"
 *    "app_id":"..."}  AppUserModelID for the WinRT toast
 *                     backend (see no_tray_set_backend);
 *                     not used by the balloon backend
 *
 * The WINDOW HANDLE belongs to the host: the host creates
 * and owns its window as usual and passes its HWND as an
 * integer. The icon is registered against that window, and
 * tray callback messages (mouse events, balloon clicks)
 * are delivered to it as message 0x0464 (WM_USER+100) with
 * wParam = uid. With "hwnd":0 the icon is registered
 * icon-only, identified by uid: it can be shown, re-tooltipped
 * and removed through this API, but no callback messages
 * can be delivered anywhere.
 *
 * Icons are the Windows stock icons (IDI_APPLICATION /
 * IDI_ASTERISK / IDI_EXCLAMATION / IDI_HAND) loaded with
 * LoadIconW - no external files.
 *
 * On success *out_handle receives the handle; on failure
 * *out_handle is NULL and the code says why
 * (NO_ERR_PLATFORM carries the GetLastError value in the
 * message). A returned handle exists only if NIM_ADD
 * actually succeeded. */
NO_API int no_tray_create(const char *config_json, NoTray **out_handle);

/* Select how this tray handle shows notifications:
 * "balloon" (the default) is the classic balloon tip;
 * "toast" is a modern WinRT toast notification. The
 * selection is per handle and can be changed at any time.
 *
 * Toasts are shown under the AppUserModelID given as
 * "app_id" in the no_tray_create config. Windows only
 * delivers a toast for an app identity the host has
 * registered - in practice the host app's installer or
 * shortcut must carry that AppUserModelID (e.g. a Start
 * menu shortcut with the System.AppUserModel.ID property
 * set to the same string). If the handle has no app_id,
 * a toast show fails with NO_ERR_INVALID_ARG rather than
 * borrowing some other app's identity.
 *
 * name must be exactly "balloon" or "toast"; anything
 * else is NO_ERR_INVALID_ARG. On non-Windows platforms
 * this returns NO_ERR_NOT_SUPPORTED, like the other tray
 * calls. */
NO_API int no_tray_set_backend(NoTray *handle, const char *name);

/* Show a notification on the tray icon, using the
 * handle's selected backend (see no_tray_set_backend):
 * the classic balloon tip (NIM_MODIFY with NIF_INFO), or
 * a WinRT toast when the backend is "toast".
 *
 * For the BALLOON backend the text limits below apply and
 * "kind" / "timeout_ms" are honoured. For the TOAST
 * backend the title and body are sent in full (escaped
 * into the toast XML), the truncation flags in the
 * result are always false, and "kind" / "timeout_ms"
 * have no toast equivalent and are ignored.
 *
 * show_json: one JSON object:
 *   {"title":"...",       required; balloon title
 *    "body":"...",        required; balloon body
 *    "kind":"info",       app|info|warn|error; default "info";
 *                         selects the balloon severity icon
 *    "timeout_ms":N}      default 0 = 10000 ms; the OS may
 *                         clamp the real display time
 *
 * Text limits are the real fixed-buffer limits of the
 * classic API: the title buffer holds 64 UTF-16 units
 * including the NUL (63 visible), the body 256 (255
 * visible). Longer text is truncated on a character
 * boundary (a surrogate pair is never split) BEFORE the
 * call, and *out_json reports exactly what was shown:
 *   {"shown":true, "title_truncated":bool,
 *    "body_truncated":bool}
 * "shown":true is written only when the platform call
 * itself succeeded (Shell_NotifyIconW for a balloon,
 * IToastNotifier::Show for a toast). */
NO_API int no_tray_show(NoTray *handle, const char *show_json,
                        char **out_json);

/* Replace the tray icon's tooltip (NIM_MODIFY with
 * NIF_TIP). The tooltip buffer holds 128 UTF-16 units
 * including the NUL (127 visible); longer text is
 * truncated on a character boundary first.
 *
 * *out_json receives caller-owned JSON:
 *   {"tooltip":"...the text actually set...",
 *    "truncated":bool} */
NO_API int no_tray_set_tooltip(NoTray *handle, const char *tooltip,
                               char **out_json);

/* Remove the tray icon (NIM_DELETE) and free the handle.
 * On failure the handle stays valid (the code and
 * no_last_error() say why) so the caller can retry; on
 * success the handle is gone and must not be used again. */
NO_API int no_tray_remove(NoTray *handle);

#ifdef __cplusplus
}
#endif
#endif
