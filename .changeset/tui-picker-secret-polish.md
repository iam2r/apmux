---
apmux: patch
---

Three TUI form/picker fixes. Space on a filtered model list (`/gpt` + Space) now picks the highlighted row in one gesture instead of needing Enter to leave the filter first; the typed filter stays on screen. Toggle lists — model pickers, catalog flags, and snippet checkboxes — draw a `[✓]` check where they used to print `[x]`, keeping the empty `[ ]` box visible so every row still reads as togglable. And clearing a stored secret (API key, sync password, gist token) leaves an empty field you can type into rather than snapping back to `(keep current)`; confirming it empty still keeps the stored value, so an emptied field is never written as a blank key.
