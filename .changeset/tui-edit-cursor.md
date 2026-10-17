---
apmux: patch
---

Keep the provider-list cursor on the row you just edited. The TUI focused rows by display name, which a rename changes, so saving an edit of any provider other than the current one moved the cursor to the current provider instead of staying put.
