# apmux

Lightweight local tool to **switch providers** for Claude Code, Codex, OpenCode, and Pi Coding Agent, with rotating local backups and WebDAV or GitHub Gist cloud backup. No GUI. Run `apmux` with no arguments for the TUI, or use subcommands in scripts.

Out of scope: proxy, MCP management, skills, sessions/usage, OAuth account managers, daemon, Gemini CLI.

[简体中文](README_ZH.md)

## Install

Linux / macOS:

```bash
curl -fsSL https://github.com/iam2r/apmux/releases/latest/download/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://github.com/iam2r/apmux/releases/latest/download/install.ps1 | iex
```

The Unix installer puts `apmux` in `~/.local/bin` (override with `APMUX_INSTALL_DIR`). If that directory is not on `PATH`, it appends a managed block to `~/.bashrc` / `~/.zshrc` / fish config. Windows installs to `%LOCALAPPDATA%\apmux\bin` and adds it to the user `Path`. Linux downloads are static musl builds. After install, `apmux update` replaces this binary from GitHub Releases (`apmux update --check` only reports).

```bash
# specific version
curl -fsSL https://github.com/iam2r/apmux/releases/latest/download/install.sh | bash -s -- v0.1.0
APMUX_SKIP_PATH=1 bash install.sh   # install only; do not edit shell rc
```

### Releases

Releases are maintainer-driven, still powered by **changeset files** (via
[Knope](https://knope.tech)), not commit messages:

1. All development happens on the `develop` branch (`main` is release-only).
   Contributors never add change files — maintainers write them when deciding
   to cut a release: `.changeset/<name>.md` on `develop`:

   ```markdown
   ---
   apmux: minor        # minor | patch | major
   ---
   One-line summary for the changelog.
   ```

2. Pushing change files to `develop` makes CI consume them and open a
   **Release PR** (`release` → `main`) that bumps `Cargo.toml` and `CHANGELOG.md`.
3. Merge the Release PR with a merge commit (not squash/rebase). The push
   to `main` tags `apmux/vX.Y.Z` if that tag is new, builds installers,
   and back-merges `main` into `develop`. No local tooling required.

From source (Rust stable):

```bash
git clone https://github.com/iam2r/apmux.git
cd apmux
cargo install --path .
# or: cargo build --release   → target/release/apmux
```

Config lives in `$HOME/.apmux` (`store.json`, `webdav.json`, `gist.json`, `backups/`). Override with `APMUX_CONFIG_DIR`.

## Migrate from cc-switch

`apmux import` copies Claude / Codex / OpenCode providers from `~/.cc-switch/cc-switch.db` into `store.json`, and WebDAV credentials from `~/.cc-switch/settings.json` into `webdav.json` (`baseUrl` only; cc-switch's `remoteRoot` / `cc-switch-sync` is ignored). Skips Gemini, Grok, and official empties. Does **not** write live CLI files, MKCOL, or pull the remote snapshot. Pi and other apps already in the store are kept. Existing `webdav.json` is left alone unless you pass `--force`.

```bash
apmux import --dry-run
apmux import
apmux list
apmux use <name>
```

Default is merge (skip existing ids). `--force` overwrites colliding ids, `current` for imported apps, and `webdav.json` (timestamp backup first). Keys are masked in the report.

## Usage

```bash
apmux                              # TUI
apmux list [--app <app>] [--json]
apmux current [--app <app>] [--json]
apmux use <name> [--app <app>]
apmux add --app <app> --name <name> --base-url <url> --api-key <key> \
        [--model <id>] [--extra key=value]... [--apply-snippet]
apmux edit <name> [--app] [--name] [--base-url] [--api-key] \
        [--model <id> | --clear-model] [--extra key=value]... \
        [--apply-snippet | --no-apply-snippet]
apmux snippet <name> [--app <app>] [--set '<json>' | --clear]
apmux delete <name> [--app] [--yes]
apmux backup [--name <name>]
apmux restore <name> [--yes] [--no-apply]
apmux backups
apmux sync setup --backend webdav --url <webdav-root> --username <user> --password <pass>
apmux sync setup --backend gist --token <github-token> [--gist <id-or-url>]
apmux sync push [--backend webdav|gist] [--force]
apmux sync pull [--backend webdav|gist] [--force]
apmux sync status [--backend webdav|gist]
apmux import [--db <path>] [--settings <path>] [--dry-run] [--force]
apmux update [--version <tag>]
apmux update --check [--json]
```

`<app>`: `claude` / `codex` / `opencode` / `pi`. Exit codes: `0` success, `1` user/validation error, `2` I/O or network.

`list` / `current` mask API keys by default (first 4 + `…` + last 4; shorter than 8 → all `*`). `--json` is also masked. `APMUX_SHOW_SECRETS=1` prints full keys — **dangerous**; they land in scrollback and CI logs. Local debugging only.

The CLI is non-interactive (no prompts). Missing required flags exit non-zero. Use the TUI for interactive editing.

## Language

English is the default for the TUI, docs, and clap help.

| Source | Effect |
|--------|--------|
| `--lang en` / `--lang zh` | Explicit (wins) |
| `APMUX_LANG` | Same values (`en`, `zh`, `zh_CN`, …) |
| TUI Settings (`g`) | Saved in `settings.json` |
| `LANG` / `LC_ALL` | `zh*` → Chinese TUI; anything else → English |

CLI clap help stays English so scripts and issue reports stay greppable. TUI strings go through `src/i18n.rs`.

## TUI keys

Bare `apmux` opens the TUI. `?` shows help for the current page.

Provider list:

| Key | Action |
|-----|--------|
| `[` `]` or Tab | Previous / next app |
| `j` `k` or ↑ ↓ | Move in the list |
| `Enter` | Switch provider |
| `a` | Add |
| `e` | Edit |
| `d` | Delete (confirm) |
| `b` | Backup now (timestamp) |
| `r` | Backups page |
| `s` | Sync page |
| `g` | Settings (language, auto/manual app detection) |
| `?` | Help |
| `q` / `Esc` | Quit / close overlay |

Forms: `Tab` / ↑ ↓ change field, Space cycles options **or fetches the model list** on the Model field, Enter/Space on **Catalog**, **Slots**, or **Snippet** opens that editor, Enter on other fields submits, Esc cancels. Secrets are masked; leave empty on edit to keep the current value. Leave an optional model empty to clear it; a required model can’t be empty.

Catalog apps (OpenCode, Pi, Codex): Space on Model fetches, Space toggles rows, Enter opens a catalog editor (`id` / `label` / `context_window` / `max_tokens`; Codex has no max-tokens column). Claude: the Slots field opens the table; Space picks one id for the focused slot; `a` copies that id onto the other slots including default.

**Snippet** on the add/edit form is per-provider JSON. **Built-in checkboxes** (Claude hide-attribution / Teammates / Tool Search / effort / auto-upgrade; Codex Goal mode) sit above the JSON body and compose it. Opt in with **Apply snippet**, or `apmux add/edit --apply-snippet`. The snippet merges first; owned fields win. `apmux snippet <name>` prints/sets/clears that provider’s JSON.

**Official rows** (`claude-official`, `codex-official`) are built in and seeded automatically: switching to one hands the CLI back to its native subscription (Claude Code login / ChatGPT login). They cannot be edited or deleted — pick one and press Enter to switch.

Backups: Enter restores (confirm), `b` snapshots now, Esc returns.

Sync: `e` setup, `p` push, `u` pull. While a job runs, a static **Syncing…** overlay is shown (no spinner); keys other than `q` are ignored.

Settings (`g`): Space/Enter changes the focused row. App detection defaults to **auto**: only a CLI found on `PATH` counts — leftover config folders do not count. Switch to **manual** to show/hide each app. Language is saved in `$APMUX_CONFIG_DIR/settings.json`.

## WebDAV

`--url` is the WebDAV root. apmux always stores files under the built-in namespace `apmux-sync`. The TUI shows that namespace on its own row; it cannot be edited.

```bash
apmux sync setup \
  --url 'https://webdav.example.com/' \
  --username 'you' \
  --password '<password>'
```

`--url` / `--username` / `--password` are always required. Non-localhost `http://` is rejected. Setup MKCOLs `{url}/apmux-sync` and stores **the root URL you submitted**.

TUI: `s` → `e` (Sync panel, WebDAV tab), fill URL / user / password. Namespace is a separate read-only row (`apmux-sync`).

`push` / `pull` take a timestamp backup before overwriting the store. On conflict, push is refused; `pull` or `--force`. `status` never prints the password.

Credentials: `$APMUX_CONFIG_DIR/webdav.json`, mode `0600`.

## GitHub Gist

The same `store.json` + `manifest.json` pair, stored in a **secret gist** instead of a WebDAV folder.

```bash
apmux sync setup --backend gist --token '<github-token>' [--gist <id-or-url>]
apmux sync push --backend gist [--force]
apmux sync pull --backend gist [--force]
apmux sync status --backend gist
```

`setup` creates a gist seeded with your current local store, or finds an existing one by the sync-format marker in its description — so a second machine discovers the same gist. `--gist <id-or-url>` pins a specific gist and skips the search. `--backend` defaults to `webdav` on every `sync` subcommand; push/pull/status also accept it, so both backends work side by side.

The token needs **Gists read/write** (a fine-grained PAT with that single permission is ideal). Gists are always created secret; `status` never prints the token. Credentials: `$APMUX_CONFIG_DIR/gist.json`, mode `0600`.

TUI: the Sync panel on the Data page has WebDAV/Gist tabs (`Tab` switches); `e` sets up the active backend, `p` pushes, `u` pulls.

`push` / `pull` behave exactly like the WebDAV backend: timestamp backup before overwrite, conflict refusal, `--force` to override.

## Project `opencode.json` shadowing

apmux writes **global** live files only:

| App | Global live |
|-----|-------------|
| Claude | `~/.claude/settings.json` (directory must already exist) |
| Codex | `~/.codex/config.toml` + `auth.json` |
| OpenCode | `~/.config/opencode/opencode.json` |
| Pi | `~/.pi/agent/models.json` + `settings.json` |

OpenCode itself prefers a **project** `opencode.json` / `opencode.jsonc`, which can shadow the global file. apmux does not scan or rewrite project files. Pi: project `.pi/` is never touched.

If the target CLI is not initialized (resolved config dir missing), apmux records `current` but **does not** create the directory or write live files.

## Do not run concurrent writers

v1 has **no** `store.json` lock. Two parallel `apmux use` / `edit` / `delete` / `restore` / `sync push|pull` processes can lose updates (atomic rename only prevents torn writes).

## Secrets and permissions

- `store.json`, `webdav.json`, backups: Unix `chmod 0600` after every save; apmux dir `0700`.
- Secret live files (Claude `settings.json`, Codex `auth.json`, OpenCode `opencode.json`, Pi `models.json`): new files `0600`; existing mode kept.
- TUI/CLI mask API keys by default. Logs never record `api_key`, passwords, or Authorization headers.

## Test isolation

Tests inject tempfile `Paths` and must not write the host:

- `~/.claude`
- `~/.codex`
- `~/.config/opencode`
- `~/.pi`
- real `~/.apmux`

`cargo test` panics if an apply would hit those paths and `Paths.home` is not a tempfile. Do not use process `HOME` as the main isolation mechanism.

```bash
cargo test
cargo fmt
cargo clippy --all-targets -- -D warnings
```

`0600` assertions are `#[cfg(unix)]`; Windows skips mode checks.

## Windows

Prefer the PowerShell one-liner above. GitHub Actions: PRs get a policy gate plus `fmt` + `clippy` + `test` on `ubuntu-latest` and `test` on Windows; the release bot tags `apmux/vX.Y.Z` and publishes Linux/macOS/Windows release assets (a manual `v*` tag push triggers the same build).

On Windows:

- Atomic write is still same-dir tmp + rename (replaces an existing target).
- `chmod 0600` / `0700` **failures are ignored**; v1 does not set Windows ACLs.
- Tests inject `Paths`; they do not rely on process `HOME` / `USERPROFILE`.

## Contributing

Bug reports, feature requests, and PRs are welcome — see
[CONTRIBUTING.md](CONTRIBUTING.md) for the workflow. All PRs target the
`develop` branch; CI runs the checks automatically, and you don't need to
add change files.

## License

MIT.
