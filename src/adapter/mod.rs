mod claude;
mod codex;
mod merge;
pub(crate) mod models;
mod opencode;
mod pi;
pub(crate) mod protocol;
pub(crate) mod quick;

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{anyhow, Result};

use crate::paths::Paths;
use crate::store::Provider;

pub use crate::store::AppId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Secret,
    Url,
    Model,
    Select(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldStorage {
    Name,
    BaseUrl,
    ApiKey,
    Model,
    Extra(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    pub default: Option<&'static str>,
    pub storage: FieldStorage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied { files: Vec<PathBuf> },
    SkippedUninitialized,
}

/// Result of applying a provider live: what happened to the config files,
/// plus whether a running process had to be refreshed for the write to take
/// effect ([`crate::reload`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    pub outcome: ApplyOutcome,
    pub reload: crate::reload::ReloadOutcome,
}

impl ApplyReport {
    /// True when the live config was written (nothing to do otherwise).
    pub fn applied(&self) -> bool {
        matches!(self.outcome, ApplyOutcome::Applied { .. })
    }
}

/// A provider discovered in a live agent config by [`AgentAdapter::rescue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescuedRow {
    pub provider: crate::store::Provider,
    /// The app's live config currently points at this row.
    pub active: bool,
}

/// A reverse-read of the app's live config: who is actually active right
/// now. [`AgentAdapter::inspect`] produces one without touching the store;
/// `apmux status` then reconciles it against the store to expose drift.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveFinger {
    /// The slot key the live config points at (the display name apmux
    /// writes); empty for apps whose config carries no slot identity
    /// (Claude injects plain env keys).
    pub slot_key: String,
    /// Base URL observed in the live config (empty when native/unknown).
    pub base_url: String,
    /// Model observed in the live config (empty when unset).
    pub model: String,
    /// True when no third-party material is injected (native login).
    pub native: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnippetSyntax {
    Json,
    Toml,
}

/// Render a snippet JSON object as the adapter's editor syntax.
pub fn render_snippet(syntax: SnippetSyntax, v: &serde_json::Value) -> String {
    match syntax {
        SnippetSyntax::Json => serde_json::to_string_pretty(v).unwrap_or_else(|_| "{\n}".into()),
        SnippetSyntax::Toml => {
            let mut doc = toml_edit::DocumentMut::new();
            fill_toml_table(doc.as_table_mut(), v);
            doc.to_string()
        }
    }
}

/// Render JSON into standard config.toml sections: scalars stay in their
/// parent section, nested objects become `[dotted]` headers. Null keys are
/// dropped (TOML has no null).
fn fill_toml_table(table: &mut toml_edit::Table, v: &serde_json::Value) {
    let Some(obj) = v.as_object() else {
        return;
    };
    for (k, val) in obj {
        if val.is_object() || val.is_null() {
            continue;
        }
        if let Some(x) = toml_scalar(val) {
            table.insert(k, toml_edit::Item::Value(x));
        }
    }
    for (k, val) in obj {
        if val.is_object() {
            let mut sub = toml_edit::Table::new();
            fill_toml_table(&mut sub, val);
            table.insert(k, toml_edit::Item::Table(sub));
        }
    }
}

/// JSON scalar/array -> inline toml_edit value (objects become inline tables).
fn toml_scalar(v: &serde_json::Value) -> Option<toml_edit::Value> {
    Some(match v {
        serde_json::Value::Bool(b) => (*b).into(),
        serde_json::Value::String(s) => s.as_str().into(),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else {
                n.as_f64().unwrap_or_default().into()
            }
        }
        serde_json::Value::Array(items) => {
            let mut arr = toml_edit::Array::new();
            for item in items {
                if let Some(x) = toml_scalar(item) {
                    arr.push(x);
                }
            }
            arr.into()
        }
        serde_json::Value::Object(o) => {
            let mut t = toml_edit::InlineTable::new();
            for (k, sv) in o {
                if let Some(x) = toml_scalar(sv) {
                    t.insert(k, x);
                }
            }
            t.into()
        }
        serde_json::Value::Null => return None,
    })
}

/// Parse the editor body back into the JSON SSOT.
pub fn parse_snippet(syntax: SnippetSyntax, text: &str) -> Result<serde_json::Value, String> {
    match syntax {
        SnippetSyntax::Json => serde_json::from_str(text).map_err(|e| e.to_string()),
        SnippetSyntax::Toml => toml_edit::de::from_str::<serde_json::Value>(text)
            .map_err(|e| format!("invalid TOML: {e}")),
    }
}

pub trait AgentAdapter: Send + Sync {
    fn id(&self) -> AppId;
    fn display_name(&self) -> &'static str;
    fn fields(&self) -> &'static [FieldSpec];
    fn resolved_dir(&self, paths: &Paths) -> PathBuf;
    fn is_initialized(&self, paths: &Paths) -> bool {
        self.resolved_dir(paths).is_dir()
    }
    fn live_paths(&self, paths: &Paths) -> Vec<PathBuf>;
    fn validate(&self, provider: &Provider) -> Result<()>;
    fn apply(&self, paths: &Paths, provider: &Provider) -> Result<ApplyOutcome>;
    /// Refresh a running instance of this app so it picks up what [`apply`]
    /// just wrote. Default: the app reads its config at launch, so there is
    /// nothing to refresh. Codex overrides this for its app-server daemon,
    /// which loads config once per daemon and is shared by every session.
    ///
    /// [`apply`]: AgentAdapter::apply
    fn reload_service(&self, paths: &Paths) -> crate::reload::ReloadOutcome {
        let _ = paths;
        crate::reload::ReloadOutcome::NotApplicable
    }
    /// Remove the provider slot previously injected under `key` from the live
    /// config (called on switch after a successful re-apply under a new key).
    /// Default: no-op (apps without an injected slot).
    fn clear_slot(&self, paths: &Paths, key: &str) -> Result<()> {
        let _ = (paths, key);
        Ok(())
    }
    /// Discover third-party providers in this app's existing live config so
    /// an empty store can be seeded from hand-edited files. Rows carry the
    /// core identity (name/base_url/api_key/model/catalog); `active` marks
    /// the entry the app currently points at.
    fn rescue(&self, paths: &Paths) -> Vec<RescuedRow> {
        let _ = paths;
        Vec::new()
    }
    /// Read the app's live config to see who is currently active, without
    /// consulting the store. `Ok(None)` means "not installed / nothing to
    /// report" (no live dir). Used by `apmux status`.
    fn inspect(&self, paths: &Paths) -> Result<Option<LiveFinger>> {
        let _ = (self, paths);
        Ok(None)
    }
    fn model_ui(&self) -> models::ModelUi {
        models::ModelUi::Catalog {
            fields: models::OPENCODE_FIELDS,
        }
    }
    fn apply_snippet(&self, live: &mut serde_json::Value, snippet: &serde_json::Value) {
        merge::json_merge(live, snippet);
    }
    fn quick_items(&self) -> &'static [quick::QuickItem] {
        &[]
    }
    /// Surface syntax for editing the common snippet; the store stays JSON.
    fn snippet_syntax(&self) -> SnippetSyntax {
        SnippetSyntax::Json
    }
}

static CLAUDE: claude::ClaudeAdapter = claude::ClaudeAdapter;
static CODEX: codex::CodexAdapter = codex::CodexAdapter;
static OPENCODE: opencode::OpenCodeAdapter = opencode::OpenCodeAdapter;
static PI: pi::PiAdapter = pi::PiAdapter;

pub fn registry() -> &'static [&'static dyn AgentAdapter] {
    static REGISTRY: [&'static dyn AgentAdapter; 4] = [&CLAUDE, &CODEX, &OPENCODE, &PI];
    &REGISTRY
}

pub fn get(app: AppId) -> Result<&'static dyn AgentAdapter> {
    registry()
        .iter()
        .copied()
        .find(|a| a.id() == app)
        .ok_or_else(|| anyhow!("adapter not implemented: {app}"))
}

pub(crate) fn parse_extras(
    adapter: &dyn AgentAdapter,
    extra: &[String],
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for item in extra {
        let Some((key, value)) = item.split_once('=') else {
            anyhow::bail!("invalid --extra '{item}', expected key=value");
        };
        if adapter
            .quick_items()
            .iter()
            .any(|q| q.extra_key == Some(key))
        {
            out.insert(key.to_string(), value.to_string());
            continue;
        }
        let spec = adapter
            .fields()
            .iter()
            .find(|f| f.key == key && matches!(f.storage, FieldStorage::Extra(_)))
            .ok_or_else(|| anyhow!("unknown extra field: {key}"))?;
        if let FieldKind::Select(allowed) = spec.kind {
            if !allowed.contains(&value) {
                anyhow::bail!("invalid value for {key}: {value}");
            }
        }
        let storage_key = match spec.storage {
            FieldStorage::Extra(k) => k,
            _ => anyhow::bail!("unknown extra field: {key}"),
        };
        out.insert(storage_key.to_string(), value.to_string());
    }
    Ok(out)
}

pub(crate) fn require_non_empty(field: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        anyhow::bail!("{field} must not be empty");
    }
    Ok(())
}

pub(crate) fn snippet_to_apply(provider: &Provider) -> Option<&serde_json::Value> {
    if !provider.apply_snippet {
        return None;
    }
    provider
        .snippet
        .as_ref()
        .filter(|v| !crate::store::is_empty_snippet(v))
}

pub(crate) fn require_http_url(url: &str) -> Result<()> {
    if !is_http_https_url(url) {
        anyhow::bail!("base_url must be an http or https URL");
    }
    Ok(())
}

fn is_http_https_url(s: &str) -> bool {
    let rest = if s.len() >= 8 && s.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
        &s[8..]
    } else if s.len() >= 7 && s.as_bytes()[..7].eq_ignore_ascii_case(b"http://") {
        &s[7..]
    } else {
        return false;
    };
    if rest.is_empty() || rest.starts_with('/') {
        return false;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !host.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_all_four_adapters() {
        let reg = registry();
        assert_eq!(reg.len(), 4);
        assert_eq!(reg[0].id(), AppId::Claude);
        assert_eq!(reg[1].id(), AppId::Codex);
        assert_eq!(reg[2].id(), AppId::OpenCode);
        assert_eq!(reg[3].id(), AppId::Pi);
        for app in [AppId::Claude, AppId::Codex, AppId::OpenCode, AppId::Pi] {
            assert!(get(app).is_ok());
        }
    }

    #[test]
    fn http_url_accepts_http_https() {
        assert!(is_http_https_url("https://api.example.com"));
        assert!(is_http_https_url("HTTP://localhost:8080/v1"));
        assert!(!is_http_https_url(""));
        assert!(!is_http_https_url("ftp://x"));
        assert!(!is_http_https_url("https://"));
        assert!(!is_http_https_url("https:///nohost"));
        assert!(!is_http_https_url("https://exa mple.com"));
    }

    #[test]
    fn parse_extras_rejects_unknown_and_non_extra() {
        let adapter = get(AppId::Claude).unwrap();
        let err = parse_extras(adapter, &["foo=bar".into()]).unwrap_err();
        assert!(err.to_string().contains("unknown extra field: foo"));
        let err = parse_extras(adapter, &["name=x".into()]).unwrap_err();
        assert!(err.to_string().contains("unknown extra field: name"));
        let err = parse_extras(adapter, &["api_key_field=token".into()]).unwrap_err();
        assert!(err.to_string().contains("invalid value"));
        let ok = parse_extras(adapter, &["api_key_field=api_key".into()]).unwrap();
        assert_eq!(ok.get("api_key_field").map(String::as_str), Some("api_key"));
        let err = parse_extras(adapter, &["nope".into()]).unwrap_err();
        assert!(err.to_string().contains("expected key=value"));
    }
}
