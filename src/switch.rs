use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};

use anyhow::Result;

use crate::adapter::{self, ApplyOutcome};
use crate::paths::Paths;
use crate::store::{AppId, Provider, Store};

pub fn normalize_provider(provider: &mut Provider) {
    if provider.model.as_ref().is_some_and(|m| m.is_empty()) {
        provider.model = None;
    }
}

// Providers are addressed by display name (exact match first, then a
// unique substring). The generated id remains a hidden alias so old
// scripts and imports keep resolving.
pub fn resolve<'a>(store: &'a Store, query: &str, app: Option<AppId>) -> Result<&'a Provider> {
    if query.is_empty() {
        anyhow::bail!("provider name must not be empty");
    }
    if let Some(p) = store.providers.get(query) {
        if app.is_none_or(|a| p.app == a) {
            return Ok(p);
        }
    }
    let exact: Vec<&Provider> = store
        .providers
        .values()
        .filter(|p| p.name == query)
        .filter(|p| app.is_none_or(|a| p.app == a))
        .collect();
    if let [one] = exact.as_slice() {
        return Ok(one);
    }
    let q = query.to_ascii_lowercase();
    let matches: Vec<&Provider> = store
        .providers
        .values()
        .filter(|p| app.is_none_or(|a| p.app == a))
        .filter(|p| p.name.to_ascii_lowercase().contains(&q))
        .collect();
    match matches.as_slice() {
        [] => anyhow::bail!("provider not found: {query}"),
        [one] => Ok(one),
        many => {
            let list = many
                .iter()
                .map(|p| format!("{} ({})", p.name, p.app))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!("ambiguous provider '{query}': {list}; narrow with --app");
        }
    }
}

pub struct AddOpts {
    pub app: AppId,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: Option<String>,
    pub extra: Vec<String>,
    pub catalog: Vec<crate::store::ModelEntry>,
    pub slots: BTreeMap<String, String>,
    pub snippet: Option<serde_json::Value>,
    pub apply_snippet: bool,
}

pub fn add_provider(paths: &Paths, store: &mut Store, opts: AddOpts) -> Result<String> {
    let adapter = adapter::get(opts.app)?;
    let extras = adapter::parse_extras(adapter, &opts.extra)?;
    ensure_unique_name(store, &opts.name, opts.app, None)?;
    let id = generate_id(&opts.name, store)?;
    let mut provider = Provider {
        id: id.clone(),
        name: opts.name,
        app: opts.app,
        base_url: opts.base_url,
        api_key: opts.api_key,
        model: opts.model,
        extras,
        catalog: opts.catalog,
        slots: opts.slots,
        snippet: crate::store::normalize_snippet(opts.snippet),
        apply_snippet: opts.apply_snippet,
        official: false,
    };
    normalize_provider(&mut provider);
    adapter.validate(&provider)?;
    let display = provider.name.clone();
    store.providers.insert(id, provider);
    store.save(paths)?;
    Ok(display)
}

pub struct EditOpts {
    pub query: String,
    pub app: Option<AppId>,
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub clear_model: bool,
    pub extra: Vec<String>,
    pub catalog: Option<Vec<crate::store::ModelEntry>>,
    pub slots: Option<BTreeMap<String, String>>,
    pub snippet: Option<serde_json::Value>,
    pub apply_snippet: Option<bool>,
}

pub fn edit_provider(paths: &Paths, store: &mut Store, opts: EditOpts) -> Result<String> {
    Ok(edit_provider_inner(paths, store, opts, true)?.name)
}

/// What an edit left behind: the row to show, plus the live apply when the
/// edited row was the current one. The id is carried separately from the name
/// because a rename changes the latter but not the former — the TUI needs the
/// id to put the cursor back on the row it just edited.
pub(crate) struct EditResult {
    pub id: String,
    pub name: String,
    pub report: Option<adapter::ApplyReport>,
}

/// Same as [`edit_provider`] but does not `eprintln` skip warnings and reports
/// the edited row's id as well as its display name (TUI).
pub(crate) fn edit_provider_quiet(
    paths: &Paths,
    store: &mut Store,
    opts: EditOpts,
) -> Result<EditResult> {
    edit_provider_inner(paths, store, opts, false)
}

fn edit_provider_inner(
    paths: &Paths,
    store: &mut Store,
    opts: EditOpts,
    stderr_warn: bool,
) -> Result<EditResult> {
    let existing = resolve(store, &opts.query, opts.app)?;
    if existing.official {
        anyhow::bail!(
            "'{}' is the built-in {} official provider and cannot be edited; switch to it instead",
            existing.id,
            existing.app
        );
    }
    let adapter = match adapter::get(existing.app) {
        Ok(adapter) => adapter,
        Err(e) => {
            anyhow::bail!(
                "{e}; delete this provider with `apmux delete {} --yes`",
                existing.id
            );
        }
    };
    let mut provider = existing.clone();
    if let Some(name) = opts.name {
        ensure_unique_name(store, &name, existing.app, Some(&existing.id))?;
        provider.name = name;
    }
    if let Some(base_url) = opts.base_url {
        provider.base_url = base_url;
    }
    if let Some(api_key) = opts.api_key {
        provider.api_key = api_key;
    }
    if opts.clear_model {
        provider.model = None;
    } else if let Some(model) = opts.model {
        provider.model = if model.is_empty() { None } else { Some(model) };
    }
    if !opts.extra.is_empty() {
        let extras = adapter::parse_extras(adapter, &opts.extra)?;
        for (k, v) in extras {
            let quick_extra = adapter
                .quick_items()
                .iter()
                .any(|q| q.extra_key == Some(k.as_str()));
            if quick_extra && matches!(v.as_str(), "" | "false" | "no" | "0") {
                provider.extras.remove(&k);
            } else {
                provider.extras.insert(k, v);
            }
        }
    }
    if let Some(catalog) = opts.catalog {
        provider.catalog = catalog;
    }
    if let Some(slots) = opts.slots {
        provider.slots = slots;
    }
    if let Some(snippet) = opts.snippet {
        provider.snippet = crate::store::normalize_snippet(Some(snippet));
    }
    if let Some(apply_snippet) = opts.apply_snippet {
        provider.apply_snippet = apply_snippet;
    }
    normalize_provider(&mut provider);
    adapter.validate(&provider)?;

    let is_current = store.current.get(&provider.app) == Some(&provider.id);
    let mut after_live = false;
    let mut report = None;
    if is_current {
        let applied = apply_then_remember(paths, adapter, &provider, stderr_warn)?;
        after_live = applied.applied();
        // A rename moves the live slot table under the new display name;
        // drop the stale one so the config never keeps both.
        if after_live {
            let new_key = (!provider.official).then(|| provider.slot_key());
            retire_previous_slot(paths, store, &provider, new_key)?;
        }
        report = Some(applied);
    }

    let id = provider.id.clone();
    let display = provider.name.clone();
    store.providers.insert(id.clone(), provider);
    persist(store, paths, after_live, &format!("re-run apmux edit {id}"))?;
    Ok(EditResult {
        id,
        name: display,
        report,
    })
}

pub fn delete_provider(
    paths: &Paths,
    store: &mut Store,
    query: &str,
    app: Option<AppId>,
    yes: bool,
) -> Result<String> {
    let provider = resolve(store, query, app)?;
    if provider.official {
        anyhow::bail!(
            "'{}' is the built-in {} official provider and cannot be deleted",
            provider.id,
            provider.app
        );
    }
    let id = provider.id.clone();
    let name = provider.name.clone();
    let app_id = provider.app;
    confirm_delete(&id, &name, yes)?;
    store.providers.shift_remove(&id);
    if store.current.get(&app_id) == Some(&id) {
        store.current.remove(&app_id);
    }
    store.save(paths)?;
    Ok(name)
}

/// Switch an app to `query` and report what the live write + reload did.
pub fn use_provider(
    paths: &Paths,
    store: &mut Store,
    query: &str,
    app: Option<AppId>,
) -> Result<(String, adapter::ApplyReport)> {
    use_provider_inner(paths, store, query, app, true)
}

/// Same as [`use_provider`] but does not `eprintln` skip warnings (TUI toast instead).
pub(crate) fn use_provider_quiet(
    paths: &Paths,
    store: &mut Store,
    query: &str,
    app: Option<AppId>,
) -> Result<(String, adapter::ApplyReport)> {
    use_provider_inner(paths, store, query, app, false)
}

fn use_provider_inner(
    paths: &Paths,
    store: &mut Store,
    query: &str,
    app: Option<AppId>,
    stderr_warn: bool,
) -> Result<(String, adapter::ApplyReport)> {
    let existing = resolve(store, query, app)?;
    let mut provider = existing.clone();
    let report = apply_provider_inner(paths, &provider, stderr_warn)?;
    normalize_provider(&mut provider);
    let after_live = report.applied();
    // Live write succeeded: retire the previous slot (the old display-name
    // table in the app's config). Switching to an official row retires it
    // without writing a replacement.
    if after_live {
        let new_key = (!provider.official).then(|| provider.slot_key());
        retire_previous_slot(paths, store, &provider, new_key)?;
    }
    store
        .providers
        .insert(provider.id.clone(), provider.clone());
    store.current.insert(provider.app, provider.id.clone());
    persist(
        store,
        paths,
        after_live,
        &format!("re-run apmux use {}", provider.name),
    )?;
    Ok((provider.name, report))
}

fn apply_provider_inner(
    paths: &Paths,
    provider: &Provider,
    stderr_warn: bool,
) -> Result<adapter::ApplyReport> {
    let adapter = adapter::get(provider.app)?;
    let mut provider = provider.clone();
    normalize_provider(&mut provider);
    adapter.validate(&provider)?;
    log::info!("switch.start app={} id={}", provider.app, provider.id);
    apply_then_remember(paths, adapter, &provider, stderr_warn)
}

/// Re-apply each `current[app]` after restore / pull.
/// Missing ids warn and skip; apply failures are collected and do not abort other apps.
pub(crate) fn reapply_current(paths: &Paths, store: &Store) -> Result<()> {
    reapply_current_inner(paths, store, true)?;
    Ok(())
}

/// Same as [`reapply_current`] but does not `eprintln`. Returns apps that skipped live write.
pub(crate) fn reapply_current_quiet(paths: &Paths, store: &Store) -> Result<Vec<AppId>> {
    reapply_current_inner(paths, store, false)
}

fn reapply_current_inner(paths: &Paths, store: &Store, stderr_warn: bool) -> Result<Vec<AppId>> {
    let mut skipped: Vec<AppId> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for (app, id) in &store.current {
        let Some(provider) = store.providers.get(id) else {
            log::warn!("switch.skip_missing app={app} id={id}");
            if stderr_warn {
                eprintln!("warning: current {app} points to missing provider '{id}'; skipped");
            }
            continue;
        };
        match apply_provider_inner(paths, provider, stderr_warn) {
            Ok(report) if report.applied() => {}
            Ok(_) => skipped.push(*app),
            Err(e) => {
                if stderr_warn {
                    eprintln!("warning: failed to re-apply {app} ({id}): {e:#}");
                }
                failures.push(format!("{app}/{id}: {e}"));
            }
        }
    }
    if failures.is_empty() {
        Ok(skipped)
    } else {
        anyhow::bail!("{}", failures.join("; "))
    }
}

/// After a successful live write under `new_key`, remove the slot table
/// previously occupied by this app (its stored `slot_keys` entry) when it
/// differs — a rename or a switch to another row/official.
pub(crate) fn retire_previous_slot(
    paths: &Paths,
    store: &mut Store,
    provider: &Provider,
    new_key: Option<String>,
) -> Result<()> {
    let old_key = store.slot_keys.get(&provider.app).cloned();
    if old_key == new_key {
        return Ok(());
    }
    if let Some(old) = old_key
        .as_ref()
        .filter(|old| new_key.as_deref() != Some(old.as_str()))
    {
        let adapter = adapter::get(provider.app)?;
        if let Err(e) = adapter.clear_slot(paths, old) {
            log::warn!("slot.clear_failed app={} key={old}: {e:#}", provider.app);
        }
    }
    match new_key {
        Some(k) => {
            store.slot_keys.insert(provider.app, k);
        }
        None => {
            store.slot_keys.remove(&provider.app);
        }
    }
    Ok(())
}

fn apply_then_remember(
    paths: &Paths,
    adapter: &dyn adapter::AgentAdapter,
    provider: &Provider,
    stderr_warn: bool,
) -> Result<adapter::ApplyReport> {
    let outcome = adapter.apply(paths, provider)?;
    let reload = match &outcome {
        ApplyOutcome::SkippedUninitialized => {
            log::warn!(
                "switch.skip_uninitialized app={} id={}",
                provider.app,
                provider.id
            );
            if stderr_warn {
                eprintln!(
                    "warning: {} is not initialized ({}); skipped live write",
                    adapter.display_name(),
                    adapter.resolved_dir(paths).display()
                );
            }
            crate::reload::ReloadOutcome::NotApplicable
        }
        ApplyOutcome::Applied { files } => {
            let list = files
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            log::info!("switch.applied files={list}");
            // Only a live write can leave a running process stale: an app
            // without a long-lived one reports NotApplicable.
            let reload = adapter.reload_service(paths);
            match &reload {
                crate::reload::ReloadOutcome::Restarted { pid } => log::info!(
                    "switch.reload_restarted app={} id={} pid={pid:?}",
                    provider.app,
                    provider.id
                ),
                crate::reload::ReloadOutcome::Failed(err) => log::warn!(
                    "switch.reload_failed app={} id={} err={err}",
                    provider.app,
                    provider.id
                ),
                _ => {}
            }
            reload
        }
    };
    Ok(adapter::ApplyReport { outcome, reload })
}

fn persist(store: &Store, paths: &Paths, after_live: bool, recovery: &str) -> Result<()> {
    match store.save(paths) {
        Ok(()) => Ok(()),
        Err(e) if after_live => Err(e.context(format!(
            "live config updated but failed to save store; {recovery}"
        ))),
        Err(e) => Err(e),
    }
}

/// Seed an empty store from each agent's existing live configuration, so
/// hand-configured users don't re-enter providers. Returns the apps that
/// contributed rows. No-op when every app reports nothing (the store stays
/// unwritten on disk).
pub fn rescue_from_live(paths: &Paths, store: &mut Store) -> Result<Vec<AppId>> {
    let mut touched: Vec<AppId> = Vec::new();
    for adapter in adapter::registry() {
        let app = adapter.id();
        let rows = adapter.rescue(paths);
        if rows.is_empty() {
            continue;
        }
        touched.push(app);
        for row in rows {
            let mut provider = row.provider;
            provider.app = app;
            // Unique id derived from the display name; "provider" as a
            // last-resort base when the name has no slug-able characters.
            let mut n = 1u32;
            let id = loop {
                let base = {
                    let b = sanitize_id(&provider.name);
                    if b.is_empty() {
                        format!("provider-{n}")
                    } else if n == 1 {
                        b
                    } else {
                        format!("{b}-{n}")
                    }
                };
                if !store.providers.contains_key(&base) {
                    break base;
                }
                n += 1;
            };
            provider.id = id.clone();
            store.providers.insert(id.clone(), provider);
            if row.active {
                let key = store.providers[&id].slot_key();
                store.slot_keys.insert(app, key);
                store.current.insert(app, id);
            }
        }
    }
    Ok(touched)
}

fn confirm_delete(id: &str, name: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!("non-interactive delete requires --yes");
    }
    eprint!("Delete provider '{id}' ({name})? [y/N] ");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let ans = line.trim();
    if ans.eq_ignore_ascii_case("y") || ans.eq_ignore_ascii_case("yes") {
        Ok(())
    } else {
        anyhow::bail!("delete cancelled");
    }
}

/// Same-app display names must stay unique: the CLI and TUI address
/// providers by name, so a duplicate would be ambiguous.
pub(crate) fn ensure_unique_name(
    store: &Store,
    name: &str,
    app: AppId,
    except_id: Option<&str>,
) -> Result<()> {
    let clash = store
        .providers
        .values()
        .any(|p| p.app == app && p.name == name && except_id.is_none_or(|id| p.id != id));
    if clash {
        anyhow::bail!("a {app} provider named '{name}' already exists");
    }
    Ok(())
}

/// Whether a generated/imported id matches the canonical slug form.
pub(crate) fn is_valid_id(s: &str) -> bool {
    let mut parts = s.split('-');
    let valid_part = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    };
    match parts.next() {
        Some(first) if valid_part(first) => parts.all(valid_part),
        _ => false,
    }
}

pub(crate) fn sanitize_id(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if (c.is_ascii_whitespace() || c == '-') && !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn generate_id(name: &str, store: &Store) -> Result<String> {
    let base = sanitize_id(name);
    if base.is_empty() {
        // Neutral fallback so any name can be added, even one made purely
        // of symbols/emoji that slugify to nothing.
        let mut n = 1u32;
        loop {
            let candidate = if n == 1 {
                "provider".into()
            } else {
                format!("provider-{n}")
            };
            if !store.providers.contains_key(&candidate) {
                return Ok(candidate);
            }
            n += 1;
        }
    }
    let mut n = 2;
    let mut candidate = base.clone();
    loop {
        if !store.providers.contains_key(&candidate) {
            return Ok(candidate);
        }
        candidate = format!("{base}-{n}");
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> (tempfile::TempDir, Paths, Store) {
        let td = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_test(td.path());
        (td, paths, Store::empty())
    }

    fn add_packy(paths: &Paths, store: &mut Store, model: Option<&str>) -> String {
        let display = add_provider(
            paths,
            store,
            AddOpts {
                app: AppId::Claude,
                name: "PackyCode".into(),
                base_url: "https://api.example.com".into(),
                api_key: "sk-test-key-abcd".into(),
                model: model.map(str::to_string),
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap();
        resolve(store, &display, None).unwrap().id.clone()
    }

    fn live_settings(paths: &Paths) -> std::path::PathBuf {
        paths.claude_dir.join("settings.json")
    }

    #[test]
    fn official_row_cannot_be_deleted_or_edited() {
        let (_td, paths, mut store) = setup();
        store.ensure_official_providers();

        // Delete is refused.
        let err = delete_provider(&paths, &mut store, "claude-official", None, true).unwrap_err();
        assert!(err.to_string().contains("cannot be deleted"), "{err}");
        assert!(store.providers.contains_key("claude-official"));

        // Edit is refused; the row stays intact for switching.
        let err = edit_provider(
            &paths,
            &mut store,
            EditOpts {
                query: "claude-official".into(),
                app: Some(AppId::Claude),
                name: Some("Renamed".into()),
                base_url: None,
                api_key: None,
                model: None,
                clear_model: false,
                extra: vec![],
                catalog: None,
                slots: None,
                snippet: None,
                apply_snippet: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("cannot be edited"), "{err}");
        assert_eq!(store.providers["claude-official"].name, "Claude Official");

        // Switching to it works (uninitialized dir skips live write).
        let (display, _report) = use_provider(&paths, &mut store, "claude-official", None).unwrap();
        assert_eq!(display, "Claude Official");
    }

    #[test]
    fn seed_is_idempotent() {
        let (_td, _paths, mut store) = setup();
        store.ensure_official_providers();
        store.ensure_official_providers();
        assert_eq!(
            store.providers.values().filter(|p| p.official).count(),
            2 // claude-official + codex-official, exactly once each
        );
    }

    #[test]
    fn add_codex_is_registered() {
        let (_td, paths, mut store) = setup();
        let added = add_provider(
            &paths,
            &mut store,
            AddOpts {
                app: AppId::Codex,
                name: "Codex".into(),
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: None,
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap();
        assert_eq!(added, "Codex"); // display name returned
        assert_eq!(store.providers["codex"].app, AppId::Codex);
    }

    /// Same-app display names must be unique: the CLI addresses providers
    /// by name, so a duplicate would be ambiguous.
    #[test]
    fn duplicate_name_rejected() {
        let (_td, paths, mut store) = setup();
        add_packy(&paths, &mut store, None);
        let err = add_provider(
            &paths,
            &mut store,
            AddOpts {
                app: AppId::Claude,
                name: "PackyCode".into(),
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: None,
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");

        // Same display name under a different app is fine.
        let display = add_provider(
            &paths,
            &mut store,
            AddOpts {
                app: AppId::Codex,
                name: "PackyCode".into(),
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: Some("gpt-5".into()),
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap();
        assert_eq!(display, "PackyCode");
    }

    /// A name that slugifies to nothing still gets a neutral generated id.
    #[test]
    fn unslugifiable_name_gets_neutral_id() {
        let (_td, paths, mut store) = setup();
        let display = add_provider(
            &paths,
            &mut store,
            AddOpts {
                app: AppId::Claude,
                name: "中文名".into(),
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: None,
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap();
        assert_eq!(display, "中文名");
    }

    #[test]
    fn generated_id_suffix_on_collision() {
        let (_td, paths, mut store) = setup();
        let a = add_packy(&paths, &mut store, None);
        assert_eq!(a, "packycode");
        // Different display name whose slug collides with the first id.
        let b = add_provider(
            &paths,
            &mut store,
            AddOpts {
                app: AppId::Claude,
                name: "Packycode".into(),
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: None,
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap();
        assert_eq!(b, "Packycode"); // display name returned
        assert_eq!(
            resolve(&store, "Packycode", None).unwrap().id,
            "packycode-2"
        );
    }

    #[test]
    fn extra_unknown_rejected() {
        let (_td, paths, mut store) = setup();
        let err = add_provider(
            &paths,
            &mut store,
            AddOpts {
                app: AppId::Claude,
                name: "X".into(),
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: None,
                extra: vec!["wire_api=responses".into()],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown extra field"));
    }

    #[test]
    fn normalize_empty_model() {
        let mut p = Provider {
            id: "a".into(),
            name: "A".into(),
            app: AppId::Claude,
            base_url: "https://example.com".into(),
            api_key: "k".into(),
            model: Some("".into()),
            extras: Default::default(),
            ..Provider::blank(AppId::Claude)
        };
        normalize_provider(&mut p);
        assert_eq!(p.model, None);
    }

    #[test]
    fn resolve_exact_id_and_name_substring() {
        let (_td, paths, mut store) = setup();
        add_packy(&paths, &mut store, None);
        add_provider(
            &paths,
            &mut store,
            AddOpts {
                app: AppId::Claude,
                name: "OtherPacky".into(),
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: None,
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap();
        assert_eq!(resolve(&store, "packycode", None).unwrap().id, "packycode");
        let err = resolve(&store, "packy", None).unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
        assert_eq!(resolve(&store, "OtherP", None).unwrap().id, "otherpacky");
        assert!(resolve(&store, "nope", None)
            .unwrap_err()
            .to_string()
            .contains("not found"));
    }

    #[test]
    fn resolve_cross_app_name_is_ambiguous_without_app() {
        let (_td, paths, mut store) = setup();
        add_packy(&paths, &mut store, None);
        add_codex(&paths, &mut store, vec![]);
        let err = resolve(&store, "packy", None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ambiguous"), "{msg}");
        assert!(msg.contains("PackyCode (claude)"), "{msg}");
        assert!(msg.contains("Packy Codex (codex)"), "{msg}");
        assert_eq!(
            resolve(&store, "packy", Some(AppId::Claude)).unwrap().id,
            "packycode"
        );
        assert_eq!(
            resolve(&store, "packy", Some(AppId::Codex)).unwrap().id,
            "packy-codex"
        );
        assert_eq!(
            resolve(&store, "packy-codex", None).unwrap().id,
            "packy-codex"
        );
        let err = resolve(&store, "packy", Some(AppId::Pi)).unwrap_err();
        assert!(err.to_string().contains("not found"), "{}", err);
    }

    #[test]
    fn use_uninitialized_sets_current_without_creating_dir() {
        let (_td, paths, mut store) = setup();
        let id = add_packy(&paths, &mut store, None);
        use_provider(&paths, &mut store, &id, None).unwrap();
        assert_eq!(
            store.current.get(&AppId::Claude).map(String::as_str),
            Some("packycode")
        );
        assert!(!paths.claude_dir.exists());
        assert!(!live_settings(&paths).exists());
        let loaded = Store::load(&paths).unwrap();
        assert_eq!(
            loaded.current.get(&AppId::Claude).map(String::as_str),
            Some("packycode")
        );
    }

    #[test]
    fn use_initialized_writes_live_then_current() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.claude_dir).unwrap();
        let id = add_packy(&paths, &mut store, Some("sonnet"));
        use_provider(&paths, &mut store, &id, None).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(live_settings(&paths)).unwrap()).unwrap();
        assert_eq!(doc["env"]["ANTHROPIC_MODEL"], "sonnet");
        assert_eq!(store.current[&AppId::Claude], id);
    }

    #[test]
    fn corrupt_json_does_not_update_current() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.claude_dir).unwrap();
        let live = live_settings(&paths);
        let bytes = fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/golden/claude/corrupt.json"),
        )
        .unwrap();
        fs::write(&live, &bytes).unwrap();
        let id = add_packy(&paths, &mut store, None);
        let err = use_provider(&paths, &mut store, &id, None).unwrap_err();
        assert!(err.to_string().contains("settings.json"));
        assert!(store.current.is_empty());
        assert_eq!(fs::read(&live).unwrap(), bytes);
        let loaded = Store::load(&paths).unwrap();
        assert!(loaded.current.is_empty());
    }

    #[test]
    fn edit_current_applies_clear_model() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.claude_dir).unwrap();
        let id = add_packy(&paths, &mut store, Some("sonnet"));
        use_provider(&paths, &mut store, &id, None).unwrap();
        edit_provider(
            &paths,
            &mut store,
            EditOpts {
                query: id.clone(),
                app: None,
                name: None,
                base_url: None,
                api_key: None,
                model: None,
                clear_model: true,
                extra: vec![],
                catalog: None,
                slots: None,
                apply_snippet: None,
                snippet: None,
            },
        )
        .unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(live_settings(&paths)).unwrap()).unwrap();
        assert!(doc["env"].get("ANTHROPIC_MODEL").is_none());
        assert_eq!(store.providers[&id].model, None);
    }

    #[test]
    fn edit_model_empty_string_is_none() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.claude_dir).unwrap();
        let id = add_packy(&paths, &mut store, Some("sonnet"));
        use_provider(&paths, &mut store, &id, None).unwrap();
        edit_provider(
            &paths,
            &mut store,
            EditOpts {
                query: id.clone(),
                app: None,
                name: None,
                base_url: None,
                api_key: None,
                model: Some("".into()),
                clear_model: false,
                extra: vec![],
                catalog: None,
                slots: None,
                apply_snippet: None,
                snippet: None,
            },
        )
        .unwrap();
        assert_eq!(store.providers[&id].model, None);
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(live_settings(&paths)).unwrap()).unwrap();
        assert!(doc["env"].get("ANTHROPIC_MODEL").is_none());
    }

    #[test]
    fn edit_non_current_does_not_touch_live() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.claude_dir).unwrap();
        let id = add_packy(&paths, &mut store, Some("sonnet"));
        edit_provider(
            &paths,
            &mut store,
            EditOpts {
                query: id.clone(),
                app: None,
                name: Some("Renamed".into()),
                base_url: None,
                api_key: None,
                model: None,
                clear_model: false,
                extra: vec![],
                catalog: None,
                slots: None,
                apply_snippet: None,
                snippet: None,
            },
        )
        .unwrap();
        assert_eq!(store.providers[&id].name, "Renamed");
        assert!(!live_settings(&paths).exists());
    }

    #[test]
    fn delete_current_drops_key_does_not_scrub_live() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.claude_dir).unwrap();
        let id = add_packy(&paths, &mut store, None);
        use_provider(&paths, &mut store, &id, None).unwrap();
        let before = fs::read(live_settings(&paths)).unwrap();
        delete_provider(&paths, &mut store, &id, None, true).unwrap();
        assert!(!store.providers.contains_key(&id));
        assert!(!store.current.contains_key(&AppId::Claude));
        assert_eq!(fs::read(live_settings(&paths)).unwrap(), before);
    }

    #[test]
    fn delete_non_current_keeps_other_current() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.claude_dir).unwrap();
        let a = add_packy(&paths, &mut store, None);
        let b = add_provider(
            &paths,
            &mut store,
            AddOpts {
                app: AppId::Claude,
                name: "Other".into(),
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: None,
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap();
        use_provider(&paths, &mut store, &a, None).unwrap();
        delete_provider(&paths, &mut store, &b, None, true).unwrap();
        assert_eq!(
            store.current.get(&AppId::Claude).map(String::as_str),
            Some("packycode")
        );
        assert!(!store.providers.contains_key("other"));
    }

    #[test]
    fn delete_without_yes_on_non_tty_fails() {
        let (_td, paths, mut store) = setup();
        let id = add_packy(&paths, &mut store, None);
        let err = delete_provider(&paths, &mut store, &id, None, false).unwrap_err();
        assert!(err.to_string().contains("--yes"));
        assert!(store.providers.contains_key(&id));
    }

    #[test]
    fn delete_unimplemented_codex_succeeds() {
        let (_td, paths, mut store) = setup();
        store.providers.insert(
            "cx".into(),
            Provider {
                id: "cx".into(),
                name: "Codex Proxy".into(),
                app: AppId::Codex,
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: None,
                extras: Default::default(),
                ..Provider::blank(AppId::Codex)
            },
        );
        store.current.insert(AppId::Codex, "cx".into());
        store.save(&paths).unwrap();
        delete_provider(&paths, &mut store, "cx", None, true).unwrap();
        assert!(!store.providers.contains_key("cx"));
        assert!(!store.current.contains_key(&AppId::Codex));
        let loaded = Store::load(&paths).unwrap();
        assert!(!loaded.providers.contains_key("cx"));
        assert!(!loaded.current.contains_key(&AppId::Codex));
    }

    #[test]
    fn edit_pi_renames() {
        let (_td, paths, mut store) = setup();
        store.providers.insert(
            "pi-proxy".into(),
            Provider {
                id: "pi-proxy".into(),
                name: "Pi Proxy".into(),
                app: AppId::Pi,
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: Some("claude-sonnet-4-5".into()),
                extras: Default::default(),
                ..Provider::blank(AppId::Pi)
            },
        );
        let id = edit_provider(
            &paths,
            &mut store,
            EditOpts {
                query: "pi-proxy".into(),
                app: None,
                name: Some("Renamed".into()),
                base_url: None,
                api_key: None,
                model: None,
                clear_model: false,
                extra: vec![],
                catalog: None,
                slots: None,
                apply_snippet: None,
                snippet: None,
            },
        )
        .unwrap();
        assert_eq!(id, "Renamed"); // new display name returned
        assert_eq!(store.providers["pi-proxy"].name, "Renamed");
    }

    #[test]
    fn use_pi_from_store_sets_current() {
        let (_td, paths, mut store) = setup();
        store.providers.insert(
            "g".into(),
            Provider {
                id: "g".into(),
                name: "G".into(),
                app: AppId::Pi,
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: Some("claude-sonnet-4-5".into()),
                extras: Default::default(),
                ..Provider::blank(AppId::Pi)
            },
        );
        use_provider(&paths, &mut store, "g", None).unwrap();
        assert_eq!(store.current.get(&AppId::Pi).map(String::as_str), Some("g"));
        assert!(!paths.pi_dir.exists());
    }

    /// A rename of the current provider must move the live slot table:
    /// the new display-name table is written and the old one removed.
    #[test]
    fn edit_current_rename_retires_old_live_slot() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.pi_dir).unwrap();
        fs::write(
            paths.pi_dir.join("models.json"),
            br#"{"providers":{"Agate":{"name":"Agate","baseUrl":"https://old.example.com"}}}"#,
        )
        .unwrap();
        store.providers.insert(
            "agate".into(),
            Provider {
                id: "agate".into(),
                name: "Agate".into(),
                app: AppId::Pi,
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: Some("claude-sonnet-4-5".into()),
                extras: Default::default(),
                ..Provider::blank(AppId::Pi)
            },
        );
        store.slot_keys.insert(AppId::Pi, "Agate".into());
        store.current.insert(AppId::Pi, "agate".into());

        edit_provider(
            &paths,
            &mut store,
            EditOpts {
                query: "Agate".into(),
                app: None,
                name: Some("Agate_".into()),
                base_url: None,
                api_key: None,
                model: None,
                clear_model: false,
                extra: vec![],
                catalog: None,
                slots: None,
                apply_snippet: None,
                snippet: None,
            },
        )
        .unwrap();

        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(paths.pi_dir.join("models.json")).unwrap())
                .unwrap();
        assert!(doc["providers"].get("Agate").is_none()); // old slot gone
        assert!(doc["providers"].get("Agate_").is_some()); // new slot present
        assert_eq!(
            store.slot_keys.get(&AppId::Pi).map(String::as_str),
            Some("Agate_")
        );
    }

    fn add_opencode(paths: &Paths, store: &mut Store, model: Option<&str>) -> Result<String> {
        add_provider(
            paths,
            store,
            AddOpts {
                app: AppId::OpenCode,
                name: "Open Packy".into(),
                base_url: "https://api.example.com".into(),
                api_key: "sk-test-key-abcd".into(),
                model: model.map(str::to_string),
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
    }

    #[test]
    fn add_opencode_requires_model() {
        let (_td, paths, mut store) = setup();
        let err = add_opencode(&paths, &mut store, None).unwrap_err();
        assert!(err.to_string().contains("model"), "{err}");
        assert!(store.providers.is_empty());
    }

    #[test]
    fn use_opencode_writes_live_and_sets_current() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.opencode_dir).unwrap();
        let id = add_opencode(&paths, &mut store, Some("gpt-4o")).unwrap();
        use_provider(&paths, &mut store, &id, None).unwrap();
        let live = paths.opencode_dir.join("opencode.json");
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&live).unwrap()).unwrap();
        assert_eq!(doc["model"], "Open Packy/gpt-4o");
        assert_eq!(doc["provider"]["Open Packy"]["name"], "Open Packy");
        assert_eq!(store.current[&AppId::OpenCode], "open-packy");
        crate::fsutil::panic_if_host_config_path(&live);
    }

    #[test]
    fn generated_managed_name_skips_reserved() {
        let (_td, paths, mut store) = setup();
        let display = add_provider(
            &paths,
            &mut store,
            AddOpts {
                app: AppId::Claude,
                name: "Managed".into(),
                base_url: "https://example.com".into(),
                api_key: "k".into(),
                model: None,
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap();
        assert_eq!(display, "Managed");
        assert_eq!(resolve(&store, "Managed", None).unwrap().id, "managed");
    }

    fn add_codex(paths: &Paths, store: &mut Store, extra: Vec<String>) -> String {
        let display = add_provider(
            paths,
            store,
            AddOpts {
                app: AppId::Codex,
                name: "Packy Codex".into(),
                base_url: "https://api.example.com".into(),
                api_key: "sk-codex-key".into(),
                model: Some("gpt-5".into()),
                extra,
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap();
        resolve(store, &display, None).unwrap().id.clone()
    }

    #[test]
    fn use_codex_uninitialized_sets_current_without_creating_dir() {
        let (_td, paths, mut store) = setup();
        let id = add_codex(&paths, &mut store, vec![]);
        use_provider(&paths, &mut store, &id, None).unwrap();
        assert_eq!(
            store.current.get(&AppId::Codex).map(String::as_str),
            Some("packy-codex")
        );
        assert!(!paths.codex_dir.exists());
        assert!(!paths.codex_dir.join("config.toml").exists());
        assert!(!paths.codex_dir.join("auth.json").exists());
    }

    #[test]
    fn use_codex_initialized_writes_live_then_current() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.codex_dir).unwrap();
        let id = add_codex(&paths, &mut store, vec![]);
        use_provider(&paths, &mut store, &id, None).unwrap();
        let text = fs::read_to_string(paths.codex_dir.join("config.toml")).unwrap();
        assert!(text.contains(&format!(
            "model_provider = \"{}\"",
            store.providers[&id].slot_key()
        )));
        assert!(text.contains("wire_api = \"responses\""));
        assert!(!text.contains("wire_api = \"chat\""));
        let auth: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(paths.codex_dir.join("auth.json")).unwrap())
                .unwrap();
        assert_eq!(auth["OPENAI_API_KEY"], "sk-codex-key");
        assert_eq!(store.current[&AppId::Codex], id);
        crate::fsutil::panic_if_host_config_path(&paths.codex_dir.join("config.toml"));
    }

    #[test]
    fn corrupt_codex_toml_does_not_update_current() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.codex_dir).unwrap();
        let live = paths.codex_dir.join("config.toml");
        let bytes = fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/golden/codex/corrupt.toml"),
        )
        .unwrap();
        fs::write(&live, &bytes).unwrap();
        let id = add_codex(&paths, &mut store, vec![]);
        let err = use_provider(&paths, &mut store, &id, None).unwrap_err();
        assert!(err.to_string().contains("config.toml"));
        assert!(store.current.is_empty());
        assert_eq!(fs::read(&live).unwrap(), bytes);
        let loaded = Store::load(&paths).unwrap();
        assert!(loaded.current.is_empty());
    }

    #[test]
    fn isolation_switch_does_not_touch_host() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.claude_dir).unwrap();
        let id = add_packy(&paths, &mut store, None);
        use_provider(&paths, &mut store, &id, None).unwrap();
        crate::fsutil::panic_if_host_config_path(&live_settings(&paths));
        crate::fsutil::panic_if_host_config_path(&paths.store_file());
    }

    fn add_pi(paths: &Paths, store: &mut Store, extra: Vec<String>) -> String {
        add_provider(
            paths,
            store,
            AddOpts {
                app: AppId::Pi,
                name: "PackyCode".into(),
                base_url: "https://proxy.example.com/v1".into(),
                api_key: "sk-test-key-abcd".into(),
                model: Some("claude-sonnet-4-5".into()),
                extra,
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn add_pi_requires_model() {
        let (_td, paths, mut store) = setup();
        let err = add_provider(
            &paths,
            &mut store,
            AddOpts {
                app: AppId::Pi,
                name: "PackyCode".into(),
                base_url: "https://proxy.example.com/v1".into(),
                api_key: "sk-test-key-abcd".into(),
                model: None,
                extra: vec![],
                catalog: vec![],
                slots: Default::default(),
                apply_snippet: false,
                snippet: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("model"), "{err}");
        assert!(store.providers.is_empty());
    }

    #[test]
    fn use_pi_writes_global_files_and_delete_leaves_old_slot() {
        let (_td, paths, mut store) = setup();
        fs::create_dir_all(&paths.pi_dir).unwrap();
        let models = paths.pi_dir.join("models.json");
        fs::write(
            &models,
            br#"{"providers":{"ollama":{"baseUrl":"http://localhost:11434"}}}"#,
        )
        .unwrap();
        let id = add_pi(&paths, &mut store, vec!["protocol=openai-responses".into()]);
        use_provider(&paths, &mut store, &id, None).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&models).unwrap()).unwrap();
        assert_eq!(doc["providers"]["PackyCode"]["apiKey"], "sk-test-key-abcd");
        assert_eq!(doc["providers"]["PackyCode"]["api"], "openai-responses");
        assert_eq!(
            doc["providers"]["ollama"]["baseUrl"],
            "http://localhost:11434"
        );
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(paths.pi_dir.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(settings["defaultProvider"], "PackyCode");
        assert_eq!(settings["defaultModel"], "claude-sonnet-4-5");

        delete_provider(&paths, &mut store, &id, None, true).unwrap();
        assert!(!store.current.contains_key(&AppId::Pi));
        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&models).unwrap()).unwrap();
        assert!(after["providers"].get("PackyCode").is_some());
        assert_eq!(
            after["providers"]["ollama"]["baseUrl"],
            "http://localhost:11434"
        );
    }
}
