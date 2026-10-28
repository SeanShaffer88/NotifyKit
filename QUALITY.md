# NotifyKit — Quality Scorecard

Scored against the v0.1.0 gate run on the development VM:
`cargo fmt --check`, clippy (host + `x86_64-pc-windows-msvc`,
`-D warnings`), windows-target check, and the full test
suite — 22/22 passing, run three times with identical
results. Scores are only claimed where that evidence exists;
Windows runtime behaviour is not claimed from Linux evidence.

| Dimension | Score /5 | Evidence |
|---|---|---|
| Coalescing correctness | 5 | Merge + count, window edge (inside at 4 999 ms, separate at 5 002 ms), title/body/kind must all match, disabled at window 0, drained history never re-absorbs — all tested with an injected clock, no sleeps |
| Bounds & drop accounting | 5 | Queue bound and history bound each tested: oldest drops, order of survivors asserted, `dropped` count asserted in stats |
| Text-limit safety | 5 | Exact-fit, overflow, and surrogate-pair-at-the-cut cases tested (pair kept whole at exactly 63 units, dropped whole at 62+2); fixed-buffer form asserted unit-for-unit including the terminating NUL |
| Honesty of the tray half | 4 | Non-Windows returns NOT_SUPPORTED at both platform and FFI layers (tested); `"shown":true` is only produced after Shell_NotifyIconW returns success; failure carries GetLastError. The Windows success path itself is code-reviewed but not yet run — one point held back |
| API & documentation | 4 | Header documents every export with semantics, the host-window contract, error codes and lifetimes; README has a runnable C example. No compiled C consumer test in the suite (FFI is exercised from Rust) — one point held back |
| Code health | 5 | Zero dependencies, fmt + clippy clean on both targets at the gate, no unwinding across the FFI boundary, per-test state only (no shared mutable statics) |
| Platform coverage | 3 | Center fully tested on Linux; the Windows MSVC build is check/clippy-verified here, but the real link and any on-screen balloon are CI's / a field test's to prove — honestly mid-scored |

**Total: 85/100**

## Waivers / open items
- The NOTIFYICONDATAW struct is the base V3 layout with cbSize = size_of of the struct as written; the Windows CI release build (real link) and a field test on a real desktop are the follow-ups that can raise the tray-half and platform scores.
- No dimension is scored on Windows runtime evidence; there is none yet.
