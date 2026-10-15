# Contributing to apmux

Thanks for your interest in contributing! This guide walks you through the
workflow. You never touch version numbers or the changelog — and you don't
need to decide when releases happen. Maintainers cut releases from the
`develop` branch when they see fit.

## Branch model

| Branch    | Purpose                                                                 |
| --------- | ----------------------------------------------------------------------- |
| `develop` | Integration branch. **All PRs target it.**                               |
| `main`    | Release-only (version bumps + changelog, written by the release bot).    |

Do not open PRs against `main` — they will be redirected to `develop`.

## Quick start

```bash
git clone https://github.com/iam2r/apmux.git   # or your fork
cd apmux
git switch develop
cargo test          # unit tests
cargo fmt --check   # formatting (CI enforces)
cargo clippy --all-targets -- -D warnings   # lints (CI enforces)
```

CI runs on every PR targeting `develop`: `fmt` + `clippy` + `test` on Linux,
and `test` on Windows. Please run the same locally before pushing.

## Making changes

1. Fork the repo and create a branch from `develop`:
   `git checkout -b fix/my-fix` or `feat/my-feature`.
2. Keep PRs small and focused — one fix or feature per PR.
3. Commit messages follow conventional style:
   `fix(tui): accept bracketed paste`, `feat(models): ...`.
   Type + optional scope, lowercase subject. Commit messages are for humans;
   releases are driven by maintainer-added change files, not by commits.
4. Push and open a PR against **`develop`**. No change file needed —
   maintainers decide if/when your change ships and write the changelog entry.
5. A maintainer reviews and merges. First-time contributors from forks: CI
   starts after a maintainer approves the workflow run ("Approve and run
   workflows" button on the PR).

### PR rules enforced by CI

A policy gate runs on every PR and fails with a clear message when:

- the PR targets `main` instead of `develop` (only the bot's Release PR may
  target `main`);
- the bot Release PR (`head=release`) targets something other than `main`;
- the PR adds `.changeset/` files (external contributors — that's the
  maintainer's job at release time).

Failing the gate blocks the merge, so fix the branch target or remove the
change files and push again.

## For maintainers: cutting a release

1. Review what's unreleased on `develop` (merged PRs since the last tag).
2. Add one change file per logical change to `.changeset/`:

   ```markdown
   ---
   apmux: patch
   ---
   One-line summary that goes into the changelog.
   ```

   `patch` = fixes · `minor` = features/flags/UI additions · `major` =
   breaking (discuss first). See [knope.toml](knope.toml).
3. Push to `develop`. The release bot takes over:
   consumes the change files → bumps `Cargo.toml` / `Cargo.lock` and updates
   `CHANGELOG.md` on a `release` branch → opens a Release PR (`release` →
   `main`). Merge it with a **merge commit** (not squash/rebase). The push
   to `main` tags `apmux/vX.Y.Z` (skipped if the tag already exists),
   builds binaries, and back-merges `main` into `develop`.

Hotfixes also go through `develop` (push the fix + change file there — it's
the fastest path to a release). `main` never takes feature work.

## Local git hooks (optional but recommended)

The repo ships git hooks. They install automatically when you run
`pnpm install` (Node 20+ and pnpm required):

- **commit-msg**: conventional-commit validation via commitlint
  (`type(scope): subject`, lowercase subject, header ≤ 100 chars).
- **pre-commit**: formats staged `*.rs` files via rustfmt and re-stages them.
- **pre-push**: runs `cargo clippy --all-targets -- -D warnings`.

Rust-only contributors without Node can still enable the hooks with
`git config core.hooksPath .husky/_`: commit-msg validation degrades to a
no-op and pre-commit falls back to a non-mutating `cargo fmt --all --
--check`. CI runs the same checks regardless, so hooks are a convenience,
not the enforcement point.

## Reporting bugs / requesting features

Please use the issue templates:

- **Bug report**: include your OS/terminal, apmux version (`apmux --version`
  or the installed tag), and steps to reproduce. TUI bugs: mention your
  terminal emulator and whether it's over SSH.
- **Feature request**: describe the problem you're trying to solve, not just
  the solution.

## Platform notes

apmux is a cross-platform TUI (macOS / Linux / Windows). When touching input
handling, rendering, or paths, consider Windows behavior — CI runs the test
suite there. Add tests for parser-level fixes (see `src/speedtest.rs` tests
for examples of deterministic probing).

## License

By contributing you agree that your contributions are licensed under the
same license as the project (see [LICENSE](LICENSE)).
