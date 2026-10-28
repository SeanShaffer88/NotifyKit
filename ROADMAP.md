# NotifyKit Roadmap

## v0.2.0 (current)
The WinRT toast backend (former roadmap item 1), shipped as a real implementation: `no_tray_set_backend(handle, "balloon"|"toast")` selects the show mechanism per tray handle, the balloon stays the default. A toast show builds a ToastGeneric XML document from the same title/body as a balloon show (pure, fully tested builder: markup escaped, XML-unrepresentable control characters stripped), loads it into a WinRT XmlDocument, and shows it through a toast notifier bound to the `"app_id"` AppUserModelID from the tray config — no app_id, no toast (an argument error, never a borrowed identity). combase.dll is resolved dynamically at runtime, so there are still no dependencies and no new link requirements. **Runtime is Unverified**: the Windows path is windows-target checked/clippy-clean at the gate, but the gate does not link and no toast has yet been shown on a real desktop — that field test is the follow-up, along with CI's real Windows link.

## v0.1.0
Two halves behind one C ABI. The notification center (all platforms): a bounded pending queue plus bounded history with windowed coalescing of repeats, poll/history/stats as JSON, drops always counted. The Windows tray half: one tray icon per handle via Shell_NotifyIconW — NIM_ADD with a host-supplied HWND passed as an integer (or 0 for an icon-only icon), NIM_MODIFY + NIF_INFO classic balloons, NIF_TIP tooltip updates, NIM_DELETE removal, stock icons only. Real fixed-buffer text limits (title 63, body 255, tooltip 127 visible UTF-16 units) enforced by char-boundary truncation that the result JSON reports. Non-Windows tray calls return NOT_SUPPORTED rather than faking success.

## Next, in order
1. **Tray menu helper**: a small declarative context-menu API for the tray icon (items, separators, checked state, selection delivered through the existing callback message), so hosts do not each hand-roll TrackPopupMenu.
2. **Balloon click / dismiss events surfaced in the center**: route NIN_BALLOONUSERCLICK and friends into the center as events the host can poll alongside notifications, for hosts that want one event stream.
3. **Custom icon from file or memory**: an optional per-handle icon loaded from an .ico path or buffer, validated before use, alongside the stock icons.
4. **Center persistence option**: an opt-in spill of queue/history to a file so a restarted host can recover undelivered notifications; off by default (v0.1 is deliberately in-memory).
5. **Toast actions and images**: extend the v0.2 toast backend (title + body only today) with buttons and images in the toast XML, behind the same show-request shape.

## Explicitly not planned
- A notification scheduler or reminder engine: NotifyKit shows what the host asks, when the host asks; timing policy belongs to the host.
- Sounds: the classic balloon API's optional sound flag stays off; a notification library that makes noise unasked is a nuisance, and hosts can play their own.
- Cross-platform tray icons: the tray half is a Windows feature. Other platforms' tray APIs differ enough that pretending they are the same call would produce exactly the fake-success behaviour this library refuses.
