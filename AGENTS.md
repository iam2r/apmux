# AGENTS.md

Guidance for Codex, Claude, Grok, and other coding agents working in this repo.

apmux switches providers for Claude Code, Codex, OpenCode, and Pi Coding Agent: one JSON store, per-app live config writes, rotating local backups, WebDAV cloud sync, a TUI, and `apmux import` for migrating from cc-switch. No GUI, no daemon.

## Commands

```bash
cargo test                              # all suites must pass
cargo fmt                               # before every commit
cargo clippy --all-targets -- -D warnings
```

## Branching, releases, and hooks

- Branch model: `develop` is the integration branch — **all PRs target it**; `main` is release-only (version bump + CHANGELOG written by the bot). Branch protection blocks direct pushes and non-gate PRs to `main`; the owner may fast-forward `main` to `develop` for non-release content (docs/CI/tooling) — `git push origin develop:main`, fast-forward only.
- PR policy (enforced by a CI gate): PRs targeting `main` are rejected (bot Release PRs excepted); external PRs must not add `.changeset/` files.
- Releases are maintainer-driven: when cutting a release, add `.changeset/<name>.md` (frontmatter `apmux: minor|patch|major`) **on `develop`** and push — Knope consumes it into a Release PR (`release` → `main`). Merge it with a **merge commit** (not squash/rebase). The push to `main` tags `apmux/v{version}` only when that tag does not already exist, dispatches the installer build, and back-merges `main` into `develop`. A non-release push to `main` (docs/CI fast-forward) syncs develop and does not rebuild assets. Never bump versions by hand; release history lives in `CHANGELOG.md`.
- Commits follow conventional style (`type(scope): lowercase subject`), enforced by commitlint in a commit-msg hook. Local hooks (install via `pnpm install`, Node 20+): commit-msg commitlint, pre-commit rustfmt on staged files via lint-staged, pre-push `cargo clippy -D warnings`; pnpm is the only supported package manager (`packageManager` is pinned, npm/yarn installs are rejected). Rust-only contributors can enable degraded hooks via `git config core.hooksPath .husky/_`. Details: `CONTRIBUTING.md`, `knope.toml`, `commitlint.config.mjs`.

Tests must never touch host configs. Use `Paths::for_test` (or set `APMUX_CONFIG_DIR`); never write `~/.claude`, `~/.codex`, `~/.config/opencode`, or `~/.pi`.

## TUI verification (Herdr, not tmux)

tmux is uninstalled on this machine. Verify TUI behavior through Herdr, the terminal workspace manager (`HERDR_ENV=1` inside its panes):

```bash
herdr tab create --label apmux-verify --cwd .        # returns pane_id / tab_id
herdr pane run <pane_id> "./target/debug/apmux"      # launch + submit Enter atomically
herdr pane wait-output --match "switch app" <pane_id> # wait for a render
herdr pane send-keys <pane_id> j                     # key-combo syntax: j, enter, esc, ctrl+c…
herdr pane read <pane_id>                            # capture screen text
herdr tab close <tab_id>
```

`pane run/send-keys/wait-output/read` address any pane regardless of occupant; `agent *` variants only work for recognized agents.

## Architecture

- `main.rs` — clap CLI; every command loads through `load_store()`.
- `store.rs` — **JSON SSOT** (`~/.apmux/store.json`). IndexMap keeps insertion order; official rows are pinned first. Unknown-version stores are rejected.
- `paths.rs` — all directory resolution (`APMUX_CONFIG_DIR` override).
- `adapter/` — registry of four adapters (Claude, Codex, OpenCode, Pi). Each declares its form fields, quick toggles, model UI (catalog vs slots), snippet syntax, apply/remove logic, slot clearing, and live-config rescue.
- `switch.rs` — the only path that writes live configs (`use_provider`); TUI Enter calls the same function as the CLI.
- `settings.rs` — apps mode (auto/manual) + language. Auto detection = CLI binary on `PATH`, nothing else.
- `backup.rs`, `cloud.rs`, `webdav.rs`, `gist.rs` — snapshot rotation and cloud sync: WebDAV under the built-in namespace `apmux-sync`, GitHub Gist as one secret gist per user. Both backends store the same `manifest.json` + `store.json` pair; `cloud::Remote` (trait) carries the shared conflict/manifest/backup protocol, `gist.rs` is CLI-only for now.
- `tui/` — ratatui app. Copy goes through `i18n.rs` (English default, zh optional); never hardcode Chinese in widgets; never `eprintln` inside the TUI.
- `import.rs` — `apmux import`: one-shot cc-switch migration (providers + WebDAV credentials) into the store; never writes live configs.
- `try_launch.rs` — `apmux try <PROVIDER>`: launch a real CLI against a provider with a throwaway config dir (official override env var per app); live configs are never read or written, temp dir removed on exit.
- `update.rs` — self-update from GitHub Releases with SHA-256 verification.

## Core invariants

- **Owned-field merge**: only fields apmux owns are overwritten in live files; unknown user keys survive every apply. Snippets merge first, owned fields win.
- **Slot key = provider display name** (`Provider::slot_key()`): written as Codex `[model_providers."Name"]`, OpenCode `provider."Name"`, Pi `providers."Name"`. `Store::slot_keys` remembers what each app's live config currently holds so a switch can retire the old table.
- **Official rows** (`claude-official`, `codex-official`) are seeded idempotently, pinned to the top, and cannot be edited or deleted. Switching to one hands the CLI back to its native login (strip apmux-owned env keys / remove the injected provider block and `OPENAI_API_KEY`; OAuth tokens are never touched).
- **First run rescue**: when `store.json` doesn't exist, `rescue_from_live` adopts hand-configured providers from each agent's live files (base URL/key/model/catalog) instead of making users re-enter them. Nothing is written if nothing is found.
- **Codex catalog rows** are cloned from the embedded native template (`codex_native_responses.json`) with stored fields overlaid. Never strip `base_instructions` — Codex refuses catalog files without it. Empty catalogs omit the file and the `model_catalog_json` key.
- **Snippets**: stored as JSON always; the editor surface syntax is per adapter (JSON everywhere except Codex, which edits TOML sections via toml_edit).
- **Secrets**: masked in output unless `APMUX_SHOW_SECRETS=1`. New live files get mode 0600; existing modes are kept.
- **Rename**: everything machine-read lives in `src/name.rs`; a rename touches it plus `Cargo.toml`, install scripts, and prose docs.

## Scope

In: provider switching for the four CLIs, local backups, WebDAV sync, cc-switch import, self-update, TUI.

Out: proxies, MCP/skills/prompt management, sessions, OAuth managers, daemons, other agent CLIs.
