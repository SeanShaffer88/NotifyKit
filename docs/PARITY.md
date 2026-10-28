# NotifyKit — Parity (spec vs implementation)

Evidence: the Rust test suite in `src/lib.rs`, `src/ffi.rs`,
`src/platform.rs` and `src/json.rs` (22 tests, all passing at
the v0.1.0 gate) plus the header contract in
`include/notifykit.h`. "Verified" below means a named test
exercises the behaviour. The Windows tray calls cannot run
on the development VM; their rows say exactly how far the
evidence goes.

| Spec feature | Status | Evidence |
|---|---|---|
| Center: enqueue(title, body, kind) with JSON in/out | Verified | `enqueue_json_validation`, FFI `ffi_center_end_to_end` |
| Coalescing: repeats of same title+body+kind inside the window merge into one entry with a count | Verified | `coalescing_merges_repeats_and_counts` (count reaches 3, one pending entry, stats shown 1 / coalesced 2) |
| Coalescing window expiry separates repeats into new entries | Verified | `coalescing_window_expiry_separates` (fake clock: merges at +4 999 ms, separate at +5 002 ms) |
| Coalescing requires same kind; window 0 disables it; history never coalesces | Verified | `coalescing_needs_same_title_body_and_kind`, `coalescing_disabled_at_zero_window`, `history_does_not_coalesce` |
| Bounded queue + bounded history, oldest-drop | Verified | `queue_bound_drops_oldest_and_counts`, `history_bound_drops_oldest_and_counts` (survivor order asserted) |
| poll_events drains pending entries into history as JSON | Verified | `poll_drains_and_history_keeps_entries` (second poll empty, history keeps the entry) |
| Stats: shown / coalesced / dropped counts + sizes | Verified | asserted inside the coalescing and bounds tests and FFI end-to-end |
| Config validation: defaults, ranges, unknown keys rejected, duplicate keys a parse error | Verified | `center_config_validation`, `tray_and_show_parsing`, JSON engine `rejects_garbage` (duplicate key case) |
| JSON parser edge cases (surrogates, depth cap, trailing garbage, bad numbers) | Verified | json.rs tests: `parses_scalars_and_surrogates`, `rejects_garbage`, `depth_cap` |
| Truncation to the real buffers (title 63, body 255, tooltip 127 visible UTF-16 units) on char boundaries, surrogate pairs never split | Verified | `truncation_exact_fit_and_overflow`, `truncation_never_splits_a_surrogate_pair` (incl. the fixed-buffer `utf16z` form and its terminating NUL) |
| `"truncated":true` reported in the result JSON | Partial | The flags are computed before the platform call and written into the `no_tray_show` / `no_tray_set_tooltip` results; the code path is reviewed, but the success result can only be produced on Windows, where it is not yet run — not claimed as verified |
| Tray create via NIM_ADD with host HWND as integer (0 = icon-only), stock icons via LoadIconW | Implemented, not run | `platform.rs` win module: hand-declared shell32/user32/kernel32 bindings with #[link] on every block; cbSize = size_of of the base V3 NOTIFYICONDATAW layout. Compiles + clippy-clean for `x86_64-pc-windows-msvc` at the gate; the real link is CI's, an on-screen icon is a field test's |
| Tray show via NIM_MODIFY + NIF_INFO with severity by kind and timeout | Implemented, not run | Same module (`show_balloon`); severity mapping app=NONE, info=INFO, warn=WARNING, error=ERROR; timeout 0 → 10 000 ms default |
| Tray set_tooltip via NIM_MODIFY + NIF_TIP; remove via NIM_DELETE; remove frees the handle only on success | Implemented, not run | Same module (`set_tooltip`, `delete_icon`); FFI keeps the handle alive on a failed remove, as the header documents |
| Tray calls on non-Windows return NOT_SUPPORTED, never fake success | Verified | platform `tray_calls_are_not_supported_off_windows`, FFI `ffi_tray_is_not_supported_off_windows` (create/show/set_tooltip/remove + argument errors still winning over the platform check) |
| Header documents every export | Done | `include/notifykit.h` (ownership, error contract, host-window contract, callback message id, text limits, handle lifetime) |

## Known gaps (also in README "Limitations")
- v0.1 shows classic balloon tips only; WinRT toasts are roadmap item 1.
- No tray context-menu helper, no click/dismiss event routing yet (roadmap items 2–3).
- Stock icons only; custom icon files are roadmap item 4.
- The center is in-memory; persistence is roadmap item 5.
