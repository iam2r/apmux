---
apmux: patch
---

Refresh a running Codex `app-server` after a switch, so `/model` stops serving the catalog the daemon read at startup. Codex's TUI re-attaches to that daemon, so Ctrl+C plus a new session kept the old provider and model rows until the daemon restarted. apmux now restarts the daemon after a successful live write — but only when one is already running (pid file plus an `app-server daemon version` probe); it never starts one. The TUI status line says the app-server was restarted, and `apmux use` prints the same instead of the generic "restart to apply" hint. A failed restart falls back to that hint with the reason.
