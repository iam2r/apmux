//! Explicit `apmux import` from a cc-switch SQLite DB into `store.json`.
//!
//! Also copies WebDAV credentials from cc-switch `settings.json` into
//! `webdav.json` (`baseUrl` only; `remoteRoot` is ignored). Does not apply live
//! files, MKCOL, or pull the remote snapshot.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use toml_edit::DocumentMut;

use crate::adapter::{self, AppId};
use crate::backup;
use crate::cloud;
use crate::mask;
use crate::paths::Paths;
use crate::store::{Provider, Store};
use crate::switch;
use crate::webdav;

const SUPPORTED: [AppId; 3] = [AppId::Claude, AppId::Codex, AppId::OpenCode];

#[derive(Debug, Clone)]
pub struct ImportOpts {
    pub db: PathBuf,
    pub settings: PathBuf,
    pub dry_run: bool,
    pub force: bool,
}

impl ImportOpts {
    pub fn from_home(home: &Path) -> Self {
        let cc = home.join(".cc-switch");
        Self {
            db: cc.join("cc-switch.db"),
            settings: cc.join("settings.json"),
            dry_run: false,
            force: false,
        }
    }
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub skipped_existing: Vec<String>,
    pub skipped: Vec<String>,
    pub current: BTreeMap<AppId, String>,
    pub providers: Vec<Provider>,
    pub webdav: WebDavPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WebDavPlan {
    #[default]
    None,
    Invalid(String),
    SkippedExisting {
        url: String,
        username: String,
    },
    Ready {
        url: String,
        username: String,
        password: String,
    },
}

impl WebDavPlan {
    fn will_write(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

impl ImportReport {
    pub fn format(&self, dest: &Path, dry_run: bool) -> String {
        let mut lines = Vec::new();
        let action = if dry_run { "would write" } else { "writing" };
        lines.push(format!(
            "{action} {} ({} providers: {} added, {} updated, {} kept)",
            dest.display(),
            self.providers.len(),
            self.added.len(),
            self.updated.len(),
            self.skipped_existing.len()
        ));
        let mut counts: BTreeMap<AppId, usize> = BTreeMap::new();
        for p in &self.providers {
            *counts.entry(p.app).or_default() += 1;
        }
        if !counts.is_empty() {
            let summary: Vec<String> = SUPPORTED
                .iter()
                .filter_map(|app| counts.get(app).map(|n| format!("{app}={n}")))
                .collect();
            lines.push(format!("counts: {}", summary.join(", ")));
        }
        lines.push(String::new());
        lines.push(format!(
            "{:<10} {:<28} {:<20} {:<14} model",
            "app", "id", "name", "key"
        ));
        for p in &self.providers {
            let model = p.model.as_deref().unwrap_or("-");
            lines.push(format!(
                "{:<10} {:<28} {:<20} {:<14} {model}",
                p.app,
                p.id,
                truncate(&p.name, 20),
                mask::mask_key(&p.api_key)
            ));
        }
        lines.push(String::new());
        if self.current.is_empty() {
            lines.push("current: (none)".into());
        } else {
            lines.push("current:".into());
            for (app, id) in &self.current {
                lines.push(format!("  {app} -> {id}"));
            }
        }
        if !self.skipped.is_empty() {
            lines.push(String::new());
            lines.push("skipped:".into());
            for note in &self.skipped {
                lines.push(format!("  {note}"));
            }
        }
        if !self.skipped_existing.is_empty() {
            lines.push(String::new());
            lines.push("already in store (pass --force to overwrite):".into());
            for id in &self.skipped_existing {
                lines.push(format!("  {id}"));
            }
        }
        lines.push(String::new());
        match &self.webdav {
            WebDavPlan::None => {}
            WebDavPlan::Invalid(reason) => {
                lines.push(format!("webdav: skipped ({reason})"));
            }
            WebDavPlan::SkippedExisting { url, username } => {
                lines.push(format!(
                    "webdav: already configured at {url} (user {username}; pass --force to overwrite)"
                ));
            }
            WebDavPlan::Ready { url, username, .. } => {
                let action = if dry_run { "would write" } else { "writing" };
                lines.push(format!("{action} webdav {url}  user={username}"));
            }
        }
        lines.push(String::new());
        lines.push("Does not apply live files. After import: apmux list && apmux use <id>".into());
        lines.join("\n")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

struct CcRow {
    id: String,
    app: String,
    name: String,
    cfg: Value,
    category: Option<String>,
    sort_index: Option<i64>,
    is_current: bool,
    meta: Value,
}

struct Mapped {
    base_url: String,
    api_key: String,
    model: Option<String>,
    extras: BTreeMap<String, String>,
}

pub fn run(paths: &Paths, store: &mut Store, opts: &ImportOpts) -> Result<ImportReport> {
    if !opts.db.is_file() {
        anyhow::bail!("cc-switch db not found: {}", opts.db.display());
    }
    let settings = load_settings(&opts.settings)?;
    let rows = read_rows(&opts.db)?;
    let mut report = plan(store, &rows, &settings, opts.force)?;
    report.webdav = plan_webdav(&settings, paths.webdav_file().is_file(), opts.force);
    if let WebDavPlan::Invalid(reason) = &report.webdav {
        report.skipped.push(format!("skip webdav: {reason}"));
    }

    if opts.dry_run {
        return Ok(report);
    }

    let write_store = !report.added.is_empty() || !report.updated.is_empty();
    let write_webdav = report.webdav.will_write();
    if !write_store && !write_webdav {
        return Ok(report);
    }

    if write_store && paths.store_file().exists() {
        let stem = backup::create(paths, None)?;
        eprintln!("backed up store to backups/{stem}.json");
    }

    if write_store {
        apply_plan(store, &report);
        store.save(paths)?;
    }
    if let WebDavPlan::Ready {
        url,
        username,
        password,
    } = &report.webdav
    {
        cloud::import_config(paths, url.clone(), username.clone(), password.clone())?;
    }
    Ok(report)
}

fn apply_plan(store: &mut Store, report: &ImportReport) {
    let overwrite: HashSet<&str> = report
        .added
        .iter()
        .chain(report.updated.iter())
        .map(String::as_str)
        .collect();
    for p in &report.providers {
        if overwrite.contains(p.id.as_str()) {
            store.providers.insert(p.id.clone(), p.clone());
        }
    }
    for (app, id) in &report.current {
        store.current.insert(*app, id.clone());
    }
}

fn plan(store: &Store, rows: &[CcRow], settings: &Value, force: bool) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    // Allocate ids from the import batch only so a re-import is stable
    // (`agate` stays `agate` even if it already exists in the store).
    let mut used: HashSet<String> = HashSet::new();
    let mut cc_to_new: BTreeMap<(AppId, String), String> = BTreeMap::new();
    let mut imported: Vec<Provider> = Vec::new();

    let mut ordered: Vec<&CcRow> = rows.iter().collect();
    ordered.sort_by(|a, b| {
        app_rank(&a.app)
            .cmp(&app_rank(&b.app))
            .then(a.sort_index.is_none().cmp(&b.sort_index.is_none()))
            .then(a.sort_index.unwrap_or(0).cmp(&b.sort_index.unwrap_or(0)))
            .then(a.name.cmp(&b.name))
    });

    for row in ordered {
        let Some(app) = parse_app(&row.app) else {
            report.skipped.push(format!(
                "skip {}/{} ({}): unsupported app",
                row.app, row.name, row.id
            ));
            continue;
        };
        if !SUPPORTED.contains(&app) {
            report.skipped.push(format!(
                "skip {}/{} ({}): unsupported app",
                row.app, row.name, row.id
            ));
            continue;
        }
        let Some(mapped) = extract(app, &row.cfg, &row.meta) else {
            let official = row
                .category
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case("official"));
            let reason = if official {
                "empty official"
            } else {
                "missing base_url/api_key (or model)"
            };
            report.skipped.push(format!(
                "skip {}/{} ({}): {reason}",
                row.app, row.name, row.id
            ));
            continue;
        };
        let new_id = allocate_id(&row.id, &row.name, app, &used);
        used.insert(new_id.clone());
        let mut provider = Provider {
            id: new_id.clone(),
            name: row.name.clone(),
            app,
            base_url: mapped.base_url,
            api_key: mapped.api_key,
            model: mapped.model,
            extras: mapped.extras,
            ..Provider::blank(app)
        };
        switch::normalize_provider(&mut provider);
        if let Err(e) = adapter::get(app)?.validate(&provider) {
            report
                .skipped
                .push(format!("skip {}/{} ({}): {e}", row.app, row.name, row.id));
            used.remove(&new_id);
            continue;
        }
        cc_to_new.insert((app, row.id.clone()), new_id.clone());
        imported.push(provider);
    }

    let settings_current = settings_current_map(settings);
    let mut current: BTreeMap<AppId, String> = BTreeMap::new();
    for app in SUPPORTED {
        if let Some(cc_id) = settings_current.get(&app) {
            if let Some(new_id) = cc_to_new.get(&(app, cc_id.clone())) {
                current.insert(app, new_id.clone());
                continue;
            }
        }
        if let Some(row) = rows
            .iter()
            .find(|r| r.app == app.to_string() && r.is_current)
        {
            if let Some(new_id) = cc_to_new.get(&(app, row.id.clone())) {
                current.insert(app, new_id.clone());
            }
        }
    }

    for provider in imported {
        let id = provider.id.clone();
        let exists = store.providers.contains_key(&id);
        if exists && !force {
            report.skipped_existing.push(id.clone());
        } else if exists {
            report.updated.push(id.clone());
        } else {
            report.added.push(id.clone());
        }
        report.providers.push(provider);
    }

    for (app, id) in current {
        let exists = store.current.get(&app) == Some(&id);
        if force || !store.current.contains_key(&app) || exists {
            report.current.insert(app, id);
        } else if let Some(existing) = store.current.get(&app) {
            report.current.insert(app, existing.clone());
        }
    }

    Ok(report)
}

fn app_rank(app: &str) -> u8 {
    match app {
        "claude" => 0,
        "codex" => 1,
        "opencode" => 2,
        _ => 9,
    }
}

fn parse_app(s: &str) -> Option<AppId> {
    match s {
        "claude" => Some(AppId::Claude),
        "codex" => Some(AppId::Codex),
        "opencode" => Some(AppId::OpenCode),
        "pi" => Some(AppId::Pi),
        _ => None,
    }
}

fn settings_current_map(settings: &Value) -> BTreeMap<AppId, String> {
    let mut out = BTreeMap::new();
    for (app, key) in [
        (AppId::Claude, "currentProviderClaude"),
        (AppId::Codex, "currentProviderCodex"),
        (AppId::OpenCode, "currentProviderOpenCode"),
    ] {
        if let Some(id) = nonempty(settings.get(key)) {
            out.insert(app, id);
        }
    }
    out
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    let is_hex = |c: u8| c.is_ascii_hexdigit();
    let dash = |i: usize| b[i] == b'-';
    dash(8)
        && dash(13)
        && dash(18)
        && dash(23)
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => true,
            _ => is_hex(*c),
        })
}

fn allocate_id(cc_id: &str, name: &str, app: AppId, used: &HashSet<String>) -> String {
    let mut candidates = Vec::new();
    if switch::is_valid_id(cc_id) && !is_uuid(cc_id) {
        candidates.push(cc_id.to_string());
    }
    let slug = switch::sanitize_id(name);
    if !slug.is_empty() {
        candidates.push(slug.clone());
        candidates.push(format!("{slug}-{app}"));
    }
    let mut seen = HashSet::new();
    for cand in candidates {
        if !seen.insert(cand.clone()) {
            continue;
        }
        if !used.contains(&cand) {
            return cand;
        }
    }
    let base = if slug.is_empty() {
        "provider".to_string()
    } else {
        slug
    };
    let mut n = 2u32;
    loop {
        let cand = format!("{base}-{n}");
        if !used.contains(&cand) {
            return cand;
        }
        n += 1;
    }
}

fn nonempty(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn extract(app: AppId, cfg: &Value, meta: &Value) -> Option<Mapped> {
    match app {
        AppId::Claude => extract_claude(cfg, meta),
        AppId::Codex => extract_codex(cfg),
        AppId::OpenCode => extract_opencode(cfg),
        AppId::Pi => None,
    }
}

fn extract_claude(cfg: &Value, meta: &Value) -> Option<Mapped> {
    let env = cfg.get("env").unwrap_or(&Value::Null);
    let base_url = nonempty(env.get("ANTHROPIC_BASE_URL"))?;
    let auth_token = nonempty(env.get("ANTHROPIC_AUTH_TOKEN"));
    let api_key_env = nonempty(env.get("ANTHROPIC_API_KEY"));
    let meta_field = nonempty(meta.get("api_key_field"));
    let (field, api_key) = match meta_field.as_deref() {
        Some("ANTHROPIC_API_KEY") => ("api_key", api_key_env.or(auth_token)),
        Some("ANTHROPIC_AUTH_TOKEN") => ("auth_token", auth_token.or(api_key_env)),
        _ if api_key_env.is_some() && auth_token.is_none() => ("api_key", api_key_env),
        _ => ("auth_token", auth_token.or(api_key_env)),
    };
    let api_key = api_key?;
    let mut extras = BTreeMap::new();
    if field != "auth_token" {
        extras.insert("api_key_field".into(), field.into());
    }
    Some(Mapped {
        base_url,
        api_key,
        model: nonempty(env.get("ANTHROPIC_MODEL")),
        extras,
    })
}

fn extract_codex(cfg: &Value) -> Option<Mapped> {
    let auth = cfg.get("auth").unwrap_or(&Value::Null);
    let (config_api_key, base_url, model) = match cfg.get("config") {
        Some(Value::String(raw)) => {
            let doc: DocumentMut = raw.parse().ok()?;
            let preferred = toml_str(&doc, &["model_provider"]).unwrap_or_else(|| "custom".into());
            let slot = ["model_providers", preferred.as_str(), "base_url"];
            let token_slot = [
                "model_providers",
                preferred.as_str(),
                "experimental_bearer_token",
            ];
            let base = toml_str(&doc, &slot)
                .or_else(|| first_codex_base_url(&doc))
                .or_else(|| toml_str(&doc, &["base_url"]));
            let model = toml_str(&doc, &["model"]);
            let api_key = toml_str(&doc, &token_slot)
                .or_else(|| toml_str(&doc, &["experimental_bearer_token"]));
            (api_key, base, model)
        }
        Some(obj) if obj.is_object() => {
            let preferred = nonempty(obj.get("model_provider")).unwrap_or_else(|| "custom".into());
            let base = nonempty(obj.pointer(&format!("/model_providers/{preferred}/base_url")))
                .or_else(|| first_json_codex_base(obj))
                .or_else(|| nonempty(obj.get("base_url")));
            let model = nonempty(obj.get("model"));
            let api_key = nonempty(obj.pointer(&format!(
                "/model_providers/{preferred}/experimental_bearer_token"
            )))
            .or_else(|| nonempty(obj.get("experimental_bearer_token")));
            (api_key, base, model)
        }
        _ => (None, None, None),
    };
    let mut api_key = config_api_key.or_else(|| nonempty(auth.get("OPENAI_API_KEY")));
    if api_key.is_none() {
        api_key = nonempty(cfg.pointer("/env/OPENAI_API_KEY"))
            .or_else(|| nonempty(cfg.get("apiKey")))
            .or_else(|| nonempty(cfg.get("api_key")));
    }
    let base_url = base_url.or_else(|| nonempty(cfg.get("base_url")))?;
    let api_key = api_key?;
    let extras = BTreeMap::new();
    Some(Mapped {
        base_url,
        api_key,
        model,
        extras,
    })
}

fn toml_str(doc: &DocumentMut, path: &[&str]) -> Option<String> {
    let mut item = doc.as_item();
    for key in path {
        item = item.get(key)?;
    }
    item.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn first_codex_base_url(doc: &DocumentMut) -> Option<String> {
    let table = doc.get("model_providers")?.as_table()?;
    for (_, item) in table.iter() {
        if let Some(s) = item.get("base_url").and_then(|v| v.as_str()) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn first_json_codex_base(obj: &Value) -> Option<String> {
    let providers = obj.get("model_providers")?.as_object()?;
    for (_, slot) in providers {
        if let Some(s) = nonempty(slot.get("base_url")) {
            return Some(s);
        }
    }
    None
}

fn extract_opencode(cfg: &Value) -> Option<Mapped> {
    let options = cfg.get("options").unwrap_or(&Value::Null);
    let base_url = nonempty(options.get("baseURL")).or_else(|| nonempty(options.get("baseUrl")))?;
    let api_key = nonempty(options.get("apiKey")).or_else(|| nonempty(options.get("api_key")))?;
    let model = cfg
        .get("models")
        .and_then(Value::as_object)
        .and_then(|m| m.keys().find(|k| !k.trim().is_empty()).cloned())
        .or_else(|| nonempty(cfg.get("model")))?;
    let mut extras = BTreeMap::new();
    if let Some(npm) = nonempty(cfg.get("npm")) {
        let protocol = match npm.as_str() {
            "@ai-sdk/openai" => Some("openai-responses"),
            "@ai-sdk/anthropic" => Some("anthropic"),
            _ => None,
        };
        if let Some(protocol) = protocol {
            extras.insert("protocol".into(), protocol.into());
        }
    }
    Some(Mapped {
        base_url,
        api_key,
        model: Some(model),
        extras,
    })
}

fn plan_webdav(settings: &Value, dest_exists: bool, force: bool) -> WebDavPlan {
    match extract_webdav(settings) {
        WebDavPlan::Ready {
            url,
            username,
            password: _,
        } if dest_exists && !force => WebDavPlan::SkippedExisting { url, username },
        other => other,
    }
}

fn extract_webdav(settings: &Value) -> WebDavPlan {
    let Some(raw) = settings
        .get("webdavSync")
        .or_else(|| settings.get("webdav_sync"))
    else {
        return WebDavPlan::None;
    };
    if raw.is_null() {
        return WebDavPlan::None;
    }
    let Some(obj) = raw.as_object() else {
        return WebDavPlan::Invalid("webdavSync is not an object".into());
    };
    let get =
        |camel: &str, snake: &str| nonempty(obj.get(camel)).or_else(|| nonempty(obj.get(snake)));
    let base = get("baseUrl", "base_url");
    let username = get("username", "username");
    let password = get("password", "password");
    if base.is_none() && username.is_none() && password.is_none() {
        return WebDavPlan::None;
    }
    let Some(base) = base else {
        return WebDavPlan::Invalid("missing baseUrl".into());
    };
    let Some(username) = username else {
        return WebDavPlan::Invalid("missing username".into());
    };
    let Some(password) = password else {
        return WebDavPlan::Invalid("missing password".into());
    };
    // cc-switch `remoteRoot` (cc-switch-sync) is that app's namespace. apmux uses
    // its own built-in `apmux-sync` at sync time and stores only the WebDAV root.
    match webdav::validate_remote_url(&base) {
        Ok(url) => WebDavPlan::Ready {
            url,
            username,
            password,
        },
        Err(e) => WebDavPlan::Invalid(format!("{e:#}")),
    }
}

fn load_settings(path: &Path) -> Result<Value> {
    if !path.is_file() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let data = fs::read_to_string(path).map_err(|e| crate::error::Error::io(path, e))?;
    let value: Value =
        serde_json::from_str(&data).map_err(|e| crate::error::Error::json(path, e))?;
    if value.is_object() {
        Ok(value)
    } else {
        anyhow::bail!("{}: root must be a JSON object", path.display());
    }
}

fn read_rows(db: &Path) -> Result<Vec<CcRow>> {
    let (_guard, conn) = open_snapshot(db)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, app_type, name, settings_config, category, sort_index, is_current, meta
             FROM providers",
        )
        .with_context(|| format!("prepare providers query {}", db.display()))?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, app, name, cfg, category, sort_index, is_current, meta) =
            row.context("read providers row")?;
        out.push(CcRow {
            id,
            app,
            name,
            cfg: parse_object(&cfg),
            category,
            sort_index,
            is_current: is_current != 0,
            meta: parse_object(meta.as_deref().unwrap_or("{}")),
        });
    }
    Ok(out)
}

fn parse_object(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or(Value::Object(serde_json::Map::new()))
}

struct SnapshotGuard {
    dir: PathBuf,
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn open_snapshot(src: &Path) -> Result<(SnapshotGuard, Connection)> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("apmux-import-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let guard = SnapshotGuard { dir: dir.clone() };
    let dest = dir.join("cc-switch.db");
    fs::copy(src, &dest).with_context(|| format!("copy {}", src.display()))?;
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", src.display()));
        if side.is_file() {
            let _ = fs::copy(&side, dir.join(format!("cc-switch.db{suffix}")));
        }
    }
    let conn = Connection::open_with_flags(
        &dest,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open snapshot {}", dest.display()))?;
    Ok((guard, conn))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil;
    use crate::paths::Paths;
    use serde_json::json;

    fn fixture_rows() -> Vec<CcRow> {
        vec![
            CcRow {
                id: "3d0ca930-13bf-42c9-a294-b601eebbf3a2".into(),
                app: "claude".into(),
                name: "Agate".into(),
                cfg: json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://api.example.com",
                        "ANTHROPIC_AUTH_TOKEN": "sk-test-key-abcd",
                        "ANTHROPIC_MODEL": "demo-model"
                    }
                }),
                category: None,
                sort_index: Some(0),
                is_current: true,
                meta: json!({}),
            },
            CcRow {
                id: "claude-other".into(),
                app: "claude".into(),
                name: "DeepSeek".into(),
                cfg: json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://api.example.com/key",
                        "ANTHROPIC_API_KEY": "sk-api-key-wxyz"
                    }
                }),
                category: Some("cn_official".into()),
                sort_index: Some(1),
                is_current: false,
                meta: json!({}),
            },
            CcRow {
                id: "codex-official".into(),
                app: "codex".into(),
                name: "OpenAI Official".into(),
                cfg: json!({"auth": {}, "config": ""}),
                category: Some("official".into()),
                sort_index: Some(0),
                is_current: false,
                meta: json!({}),
            },
            CcRow {
                id: "074fe4f6-9e99-4337-9cc1-c33438182aed".into(),
                app: "codex".into(),
                name: "Agate".into(),
                cfg: json!({
                    "auth": {"OPENAI_API_KEY": "sk-codex-key-efgh"},
                    "config": "model_provider = \"custom\"\nmodel = \"gpt-demo\"\n\n[model_providers.custom]\nname = \"Agate\"\nbase_url = \"https://codex.example.com/v1\"\nwire_api = \"chat\"\nrequires_openai_auth = true\n"
                }),
                category: None,
                sort_index: Some(1),
                is_current: true,
                meta: json!({}),
            },
            CcRow {
                id: "agate".into(),
                app: "opencode".into(),
                name: "Agate".into(),
                cfg: json!({
                    "npm": "@ai-sdk/openai-compatible",
                    "options": {"baseURL": "https://oc.example.com/v1", "apiKey": "sk-open-key-ijkl"},
                    "models": {"flash": {"name": "flash"}}
                }),
                category: None,
                sort_index: Some(0),
                is_current: false,
                meta: json!({}),
            },
            CcRow {
                id: "gemini-official".into(),
                app: "gemini".into(),
                name: "Google Official".into(),
                cfg: json!({"env": {}}),
                category: Some("official".into()),
                sort_index: Some(0),
                is_current: false,
                meta: json!({}),
            },
        ]
    }

    #[test]
    fn maps_ids_and_skips_unsupported() {
        let store = Store::empty();
        let settings = json!({
            "currentProviderClaude": "3d0ca930-13bf-42c9-a294-b601eebbf3a2"
        });
        let report = plan(&store, &fixture_rows(), &settings, false).unwrap();
        let ids: Vec<&str> = report.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            ["agate", "claude-other", "agate-codex", "agate-opencode"]
        );
        let agate = report.providers.iter().find(|p| p.id == "agate").unwrap();
        assert_eq!(agate.app, AppId::Claude);
        assert_eq!(agate.model.as_deref(), Some("demo-model"));
        assert!(agate.extras.is_empty());

        let deepseek = report
            .providers
            .iter()
            .find(|p| p.name == "DeepSeek")
            .unwrap();
        assert_eq!(
            deepseek.extras.get("api_key_field").map(String::as_str),
            Some("api_key")
        );
        assert_eq!(deepseek.api_key, "sk-api-key-wxyz");

        let codex = report
            .providers
            .iter()
            .find(|p| p.app == AppId::Codex)
            .unwrap();
        assert_eq!(codex.id, "agate-codex");
        assert_eq!(codex.base_url, "https://codex.example.com/v1");
        assert!(codex.extras.is_empty());
        assert_eq!(codex.model.as_deref(), Some("gpt-demo"));

        let oc = report
            .providers
            .iter()
            .find(|p| p.app == AppId::OpenCode)
            .unwrap();
        assert_eq!(oc.id, "agate-opencode");
        assert_eq!(oc.model.as_deref(), Some("flash"));
        assert!(oc.extras.is_empty());

        assert_eq!(
            report.current.get(&AppId::Claude).map(String::as_str),
            Some("agate")
        );
        assert_eq!(
            report.current.get(&AppId::Codex).map(String::as_str),
            Some("agate-codex")
        );
        assert!(!report.current.contains_key(&AppId::OpenCode));

        let skip = report.skipped.join("\n");
        assert!(skip.contains("gemini"), "{skip}");
        assert!(skip.contains("OpenAI Official"), "{skip}");
        assert_eq!(report.providers.len(), 4);

        let text = report.format(Path::new("/tmp/store.json"), true);
        assert!(!text.contains("sk-test-key-abcd"), "{text}");
        assert!(text.contains("sk-t…abcd"), "{text}");
        assert!(!text.contains("webdav:"), "{text}");
    }

    #[test]
    fn merge_skips_existing_ids_force_overwrites() {
        let mut store = Store::empty();
        store.providers.insert(
            "agate".into(),
            Provider {
                id: "agate".into(),
                name: "Old".into(),
                app: AppId::Claude,
                base_url: "https://old.example".into(),
                api_key: "sk-old-key-aaaa".into(),
                model: None,
                extras: BTreeMap::new(),
                ..Provider::blank(AppId::Claude)
            },
        );
        store.current.insert(AppId::Claude, "agate".into());
        store.current.insert(AppId::Pi, "agate-pi".into());
        store.providers.insert(
            "agate-pi".into(),
            Provider {
                id: "agate-pi".into(),
                name: "Pi Agate".into(),
                app: AppId::Pi,
                base_url: "https://pi.example".into(),
                api_key: "sk-pi-key-zzzz".into(),
                model: Some("m".into()),
                extras: BTreeMap::new(),
                ..Provider::blank(AppId::Pi)
            },
        );

        let settings = json!({});
        let merge = plan(&store, &fixture_rows(), &settings, false).unwrap();
        assert!(merge.skipped_existing.contains(&"agate".to_string()));
        assert!(merge.added.iter().any(|id| id == "agate-codex"));
        apply_plan(&mut store, &merge);
        assert_eq!(store.providers.get("agate").unwrap().name, "Old");
        assert!(store.providers.contains_key("agate-pi"));
        assert_eq!(
            store.current.get(&AppId::Pi).map(String::as_str),
            Some("agate-pi")
        );

        let force = plan(&store, &fixture_rows(), &settings, true).unwrap();
        assert!(force.updated.contains(&"agate".to_string()));
        apply_plan(&mut store, &force);
        assert_eq!(store.providers.get("agate").unwrap().name, "Agate");
        assert_eq!(
            store.providers.get("agate").unwrap().api_key,
            "sk-test-key-abcd"
        );
        assert!(store.providers.contains_key("agate-pi"));
        assert_eq!(
            store.current.get(&AppId::Pi).map(String::as_str),
            Some("agate-pi")
        );
    }

    fn write_fixture_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE providers (
                id TEXT NOT NULL, app_type TEXT NOT NULL, name TEXT NOT NULL,
                settings_config TEXT NOT NULL, category TEXT, sort_index INTEGER,
                is_current BOOLEAN NOT NULL DEFAULT 0, meta TEXT NOT NULL DEFAULT '{}'
            );",
        )
        .unwrap();
        for row in fixture_rows() {
            conn.execute(
                "INSERT INTO providers VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![
                    row.id,
                    row.app,
                    row.name,
                    row.cfg.to_string(),
                    row.category,
                    row.sort_index,
                    row.is_current as i64,
                    row.meta.to_string(),
                ],
            )
            .unwrap();
        }
    }

    #[test]
    fn sqlite_roundtrip_does_not_print_or_drop_other_apps() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        fsutil::ensure_dir_0700(&paths.config_dir).unwrap();
        let mut store = Store::empty();
        store.providers.insert(
            "agate-pi".into(),
            Provider {
                id: "agate-pi".into(),
                name: "Pi Agate".into(),
                app: AppId::Pi,
                base_url: "https://pi.example".into(),
                api_key: "sk-pi-key-zzzz".into(),
                model: Some("m".into()),
                extras: BTreeMap::new(),
                ..Provider::blank(AppId::Pi)
            },
        );
        store.current.insert(AppId::Pi, "agate-pi".into());
        store.save(&paths).unwrap();

        let db = td.path().join("cc-switch.db");
        write_fixture_db(&db);
        let settings = td.path().join("settings.json");
        fs::write(
            &settings,
            json!({
                "currentProviderClaude": "3d0ca930-13bf-42c9-a294-b601eebbf3a2",
                "webdavSync": {
                    "enabled": true,
                    "baseUrl": "https://webdav.example.com/",
                    "remoteRoot": "cc-switch-sync",
                    "profile": "default",
                    "username": "ccswitch",
                    "password": "dav-secret-zzzz"
                }
            })
            .to_string(),
        )
        .unwrap();

        let mut store = Store::load(&paths).unwrap();
        let opts = ImportOpts {
            db,
            settings,
            dry_run: false,
            force: false,
        };
        let report = run(&paths, &mut store, &opts).unwrap();
        let text = report.format(&paths.store_file(), false);
        assert!(!text.contains("sk-test-key-abcd"));
        assert!(!text.contains("dav-secret-zzzz"), "{text}");
        assert!(text.contains("writing webdav"), "{text}");
        assert!(text.contains("https://webdav.example.com/"), "{text}");
        assert!(
            !text.contains("cc-switch-sync"),
            "must not splice cc-switch remoteRoot: {text}"
        );
        let dav: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(paths.webdav_file()).unwrap()).unwrap();
        assert_eq!(dav["url"].as_str(), Some("https://webdav.example.com/"));
        assert_eq!(dav["username"].as_str(), Some("ccswitch"));
        assert_eq!(dav["password"].as_str(), Some("dav-secret-zzzz"));
        assert_eq!(dav["last_pulled_sha256"].as_str(), Some(""));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(paths.webdav_file())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        let store = Store::load(&paths).unwrap();
        assert!(store.providers.contains_key("agate"));
        assert!(store.providers.contains_key("agate-pi"));
        assert_eq!(
            store.current.get(&AppId::Pi).map(String::as_str),
            Some("agate-pi")
        );
        assert_eq!(
            store.current.get(&AppId::Claude).map(String::as_str),
            Some("agate")
        );
    }

    #[test]
    fn dry_run_does_not_write() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let db = td.path().join("cc-switch.db");
        write_fixture_db(&db);
        let mut store = Store::empty();
        let opts = ImportOpts {
            db,
            settings: td.path().join("missing.json"),
            dry_run: true,
            force: false,
        };
        let report = run(&paths, &mut store, &opts).unwrap();
        assert!(!report.added.is_empty());
        assert!(!paths.store_file().exists());
        assert!(!paths.webdav_file().exists());
        assert!(store.providers.is_empty());
    }

    fn webdav_settings() -> Value {
        json!({
            "webdavSync": {
                "enabled": true,
                "baseUrl": "https://webdav.iamrazo.eu.org/",
                "remoteRoot": "cc-switch-sync",
                "username": "ccswitch",
                "password": "app-password-secret"
            }
        })
    }

    #[test]
    fn webdav_ready_keeps_base_url_ignores_remote_root() {
        let plan = extract_webdav(&webdav_settings());
        match plan {
            WebDavPlan::Ready {
                url,
                username,
                password,
            } => {
                assert_eq!(url, "https://webdav.iamrazo.eu.org/");
                assert!(!url.contains("cc-switch-sync"));
                assert_eq!(username, "ccswitch");
                assert_eq!(password, "app-password-secret");
            }
            other => panic!("{other:?}"),
        }
        let existing = plan_webdav(&webdav_settings(), true, false);
        assert!(matches!(existing, WebDavPlan::SkippedExisting { .. }));
        let forced = plan_webdav(&webdav_settings(), true, true);
        assert!(forced.will_write());
    }

    #[test]
    fn webdav_import_writes_and_force_overwrites() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        fsutil::ensure_dir_0700(&paths.config_dir).unwrap();
        let db = td.path().join("cc-switch.db");
        write_fixture_db(&db);
        let settings = td.path().join("settings.json");
        fs::write(&settings, webdav_settings().to_string()).unwrap();

        let mut store = Store::empty();
        let opts = ImportOpts {
            db: db.clone(),
            settings: settings.clone(),
            dry_run: true,
            force: false,
        };
        let report = run(&paths, &mut store, &opts).unwrap();
        let text = report.format(&paths.store_file(), true);
        assert!(text.contains("would write webdav"), "{text}");
        assert!(!text.contains("app-password-secret"), "{text}");
        assert!(!paths.webdav_file().exists());

        let opts = ImportOpts {
            db: db.clone(),
            settings: settings.clone(),
            dry_run: false,
            force: false,
        };
        run(&paths, &mut store, &opts).unwrap();
        let first = fs::read_to_string(paths.webdav_file()).unwrap();
        assert!(first.contains("app-password-secret"));

        fs::write(
            &settings,
            json!({
                "webdavSync": {
                    "baseUrl": "https://webdav.iamrazo.eu.org/",
                    "remoteRoot": "cc-switch-sync",
                    "username": "ccswitch",
                    "password": "new-password-yyyy"
                }
            })
            .to_string(),
        )
        .unwrap();
        let mut store = Store::load(&paths).unwrap();
        let skip = run(
            &paths,
            &mut store,
            &ImportOpts {
                db: db.clone(),
                settings: settings.clone(),
                dry_run: false,
                force: false,
            },
        )
        .unwrap();
        assert!(matches!(skip.webdav, WebDavPlan::SkippedExisting { .. }));
        assert_eq!(
            serde_json::from_str::<Value>(&fs::read_to_string(paths.webdav_file()).unwrap())
                .unwrap()["password"]
                .as_str(),
            Some("app-password-secret")
        );

        let mut store = Store::load(&paths).unwrap();
        run(
            &paths,
            &mut store,
            &ImportOpts {
                db,
                settings,
                dry_run: false,
                force: true,
            },
        )
        .unwrap();
        let dav: Value =
            serde_json::from_str(&fs::read_to_string(paths.webdav_file()).unwrap()).unwrap();
        assert_eq!(dav["password"].as_str(), Some("new-password-yyyy"));
    }

    #[test]
    fn webdav_missing_password_is_skipped() {
        let plan = extract_webdav(&json!({
            "webdavSync": {
                "baseUrl": "https://webdav.example.com/dav",
                "username": "u"
            }
        }));
        assert!(matches!(plan, WebDavPlan::Invalid(ref s) if s.contains("password")));
    }
}
