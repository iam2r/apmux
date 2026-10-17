use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use toml_edit::DocumentMut;

use super::merge::{json_remove, json_set, toml_remove, toml_set};
use super::{
    require_non_empty, AgentAdapter, AppId, ApplyOutcome, FieldKind, FieldSpec, FieldStorage,
};
use crate::error::Error;
use crate::fsutil;
use crate::paths::Paths;
use crate::store::Provider;

const FIELDS: &[FieldSpec] = &[
    FieldSpec {
        key: "name",
        label: "field.name",
        kind: FieldKind::Text,
        required: true,
        default: None,
        storage: FieldStorage::Name,
    },
    FieldSpec {
        key: "base_url",
        label: "field.base_url",
        kind: FieldKind::Url,
        required: true,
        default: None,
        storage: FieldStorage::BaseUrl,
    },
    FieldSpec {
        key: "api_key",
        label: "field.api_key",
        kind: FieldKind::Secret,
        required: true,
        default: None,
        storage: FieldStorage::ApiKey,
    },
    FieldSpec {
        key: "model",
        label: "field.model",
        kind: FieldKind::Model,
        required: false,
        default: None,
        storage: FieldStorage::Model,
    },
];

pub struct CodexAdapter;

impl CodexAdapter {
    fn config_file(&self, paths: &Paths) -> PathBuf {
        self.resolved_dir(paths).join("config.toml")
    }

    fn auth_file(&self, paths: &Paths) -> PathBuf {
        self.resolved_dir(paths).join("auth.json")
    }

    fn catalog_file(&self, paths: &Paths) -> PathBuf {
        self.resolved_dir(paths).join(CATALOG_FILENAME)
    }
}

const CATALOG_FILENAME: &str = crate::name::CODEX_CATALOG_FILE;

impl AgentAdapter for CodexAdapter {
    fn id(&self) -> AppId {
        AppId::Codex
    }

    fn display_name(&self) -> &'static str {
        "Codex"
    }

    fn fields(&self) -> &'static [FieldSpec] {
        FIELDS
    }

    fn resolved_dir(&self, paths: &Paths) -> PathBuf {
        paths.codex_dir.clone()
    }

    fn live_paths(&self, paths: &Paths) -> Vec<PathBuf> {
        vec![
            self.config_file(paths),
            self.auth_file(paths),
            self.catalog_file(paths),
        ]
    }

    fn model_ui(&self) -> super::models::ModelUi {
        super::models::ModelUi::Catalog {
            fields: super::models::CODEX_FIELDS,
        }
    }

    fn quick_items(&self) -> &'static [super::quick::QuickItem] {
        super::quick::CODEX
    }

    fn snippet_syntax(&self) -> super::SnippetSyntax {
        super::SnippetSyntax::Toml
    }

    fn validate(&self, provider: &Provider) -> Result<()> {
        require_non_empty("name", &provider.name)?;
        if provider.official {
            return Ok(());
        }
        require_non_empty("base_url", &provider.base_url)?;
        require_non_empty("api_key", &provider.api_key)?;
        Ok(())
    }

    fn inspect(&self, paths: &Paths) -> Result<Option<super::LiveFinger>> {
        use super::LiveFinger;
        if !self.is_initialized(paths) {
            return Ok(None);
        }
        let doc = read_toml(&self.config_file(paths))?;
        // No model_provider override: Codex's native ChatGPT login is active.
        let Some(slot) = doc
            .get("model_provider")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            return Ok(Some(LiveFinger {
                native: true,
                ..Default::default()
            }));
        };
        let base_url = doc
            .get("model_providers")
            .and_then(|v| v.get(&slot))
            .and_then(|t| t.get("base_url"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let model = doc
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(Some(LiveFinger {
            slot_key: slot,
            base_url,
            model,
            native: false,
        }))
    }

    fn rescue(&self, paths: &Paths) -> Vec<super::RescuedRow> {
        use super::RescuedRow;
        const BUILTIN: &[&str] = &[
            "openai",
            "amazon-bedrock",
            "amazon-bedrock-runtime",
            "ollama",
            "lmstudio",
            "ollama-chat",
        ];
        let config_path = self.config_file(paths);
        let Ok(doc) = read_toml(&config_path) else {
            return Vec::new();
        };
        // Only tables under [model_providers.*] are user-defined providers;
        // built-in ids never appear there (they live in code).
        let Some(providers) = doc.get("model_providers").and_then(|v| v.as_table()) else {
            return Vec::new();
        };
        let active_id = doc
            .get("model_provider")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let legacy_api_key = read_json_object(&self.auth_file(paths))
            .ok()
            .and_then(|auth| {
                auth.get("OPENAI_API_KEY")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let model = doc
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let catalog = doc
            .get("model_catalog_json")
            .and_then(|v| v.as_str())
            .and_then(|f| read_catalog_entries(&self.resolved_dir(paths).join(f)));

        let mut rows = Vec::new();
        for (key, table) in providers.iter() {
            if BUILTIN.contains(&key) {
                continue;
            }
            let base_url = table
                .get("base_url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if base_url.is_empty() {
                continue;
            }
            let requires_login = table
                .get("requires_openai_auth")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let active = key == active_id;
            let toml_api_key = table
                .get("experimental_bearer_token")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let api_key = toml_api_key
                .or_else(|| {
                    if active {
                        doc.get("experimental_bearer_token")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    if active || requires_login {
                        Some(legacy_api_key.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            rows.push(RescuedRow {
                provider: crate::store::Provider {
                    id: String::new(), // assigned by the store assembler
                    name: key.to_string(),
                    base_url,
                    api_key,
                    model: if active { model.clone() } else { None },
                    catalog: if active {
                        catalog.clone().unwrap_or_default()
                    } else {
                        Vec::new()
                    },
                    ..crate::store::Provider::blank(crate::store::AppId::Codex)
                },
                active,
            });
        }
        rows
    }

    fn clear_slot(&self, paths: &Paths, key: &str) -> Result<()> {
        let config_path = self.config_file(paths);
        if !config_path.is_file() {
            return Ok(());
        }
        let mut doc = read_toml(&config_path)?;
        let existed = doc
            .get("model_providers")
            .and_then(|t| t.get(key))
            .is_some();
        if !existed {
            return Ok(());
        }
        toml_remove(&mut doc, &["model_providers", key])
            .with_context(|| config_path.display().to_string())?;
        write_live_toml(&config_path, &doc)
    }

    fn apply(&self, paths: &Paths, provider: &Provider) -> Result<ApplyOutcome> {
        if !self.is_initialized(paths) {
            return Ok(ApplyOutcome::SkippedUninitialized);
        }
        self.validate(provider)?;

        let config_path = self.config_file(paths);
        let auth_path = self.auth_file(paths);

        let mut toml_doc = read_toml(&config_path)?;
        if let Some(snippet) = super::snippet_to_apply(provider) {
            super::merge::toml_merge_json(&mut toml_doc, snippet);
        }
        patch_codex_toml(&mut toml_doc, provider)
            .with_context(|| config_path.display().to_string())?;
        let catalog_path = self.catalog_file(paths);
        let wrote_catalog = write_codex_catalog(&catalog_path, &mut toml_doc, provider)?;

        let mut auth = read_json_object(&auth_path)?;
        if provider.official {
            // Hand auth.json back to Codex's native login: remove the
            // third-party API key so ChatGPT OAuth material (tokens, kept
            // intact) takes priority again. Never clobber the login cache.
            json_remove(&mut auth, &["OPENAI_API_KEY"])
                .with_context(|| auth_path.display().to_string())?;
        } else {
            json_set(
                &mut auth,
                &["OPENAI_API_KEY"],
                Value::String(provider.api_key.clone()),
            )
            .with_context(|| auth_path.display().to_string())?;
        }

        // Parse both first; write auth then config so a failed second write can restore auth.
        let old_auth = read_existing_bytes(&auth_path)?;
        write_live_json(&auth_path, &auth)?;
        if let Err(e) = write_live_toml(&config_path, &toml_doc) {
            // Don't swallow restore errors: live may be split (new auth, old config).
            if let Err(restore_err) = restore_file(&auth_path, old_auth.as_deref()) {
                return Err(e.context(restore_err).context(format!(
                    "failed to restore auth.json after config.toml write failed ({})",
                    auth_path.display()
                )));
            }
            return Err(e);
        }

        let mut files = vec![config_path, auth_path];
        if wrote_catalog {
            files.push(catalog_path);
        }
        Ok(ApplyOutcome::Applied { files })
    }

    /// Codex's TUI talks to a long-lived `app-server` daemon that loads
    /// `config.toml` and the model catalog once, at daemon start. A session
    /// opened after a switch re-attaches to that same daemon, so `/model`
    /// keeps offering the rows from daemon start until it restarts — this is
    /// what makes the new catalog visible to already-open sessions.
    fn reload_service(&self, paths: &Paths) -> crate::reload::ReloadOutcome {
        match which::which(AppId::Codex.to_string()) {
            Ok(bin) => crate::reload::codex(paths, &bin),
            Err(_) => crate::reload::ReloadOutcome::NotApplicable,
        }
    }
}

fn patch_codex_toml(doc: &mut DocumentMut, provider: &Provider) -> Result<()> {
    let key = provider.slot_key();
    // The official row hands Codex back to its native ChatGPT login:
    // drop our provider table and the model_provider override entirely.
    // An official row never carries a catalog, so the catalog file and key
    // are removed too (write_codex_catalog handles that below). The previous
    // slot table is removed by clear_slot after a successful switch.
    if provider.official {
        toml_remove(doc, &["model_provider"])?;
        toml_remove(doc, &["model"])?;
        toml_remove(doc, &["experimental_bearer_token"])?;
        return Ok(());
    }

    toml_set(doc, &["model_provider"], toml_edit::value(key.as_str()))?;
    match &provider.model {
        Some(model) => toml_set(doc, &["model"], toml_edit::value(model.as_str()))?,
        None => toml_remove(doc, &["model"])?,
    }

    let provider_name = if matches!(
        provider.extras.get("remote_compaction").map(String::as_str),
        Some("true") | Some("yes") | Some("1")
    ) {
        "OpenAI"
    } else {
        provider.name.as_str()
    };
    toml_set(
        doc,
        &["model_providers", key.as_str(), "name"],
        toml_edit::value(provider_name),
    )?;
    toml_set(
        doc,
        &["model_providers", key.as_str(), "base_url"],
        toml_edit::value(provider.base_url.as_str()),
    )?;
    toml_set(
        doc,
        &["model_providers", key.as_str(), "wire_api"],
        toml_edit::value("responses"),
    )?;
    toml_set(
        doc,
        &["model_providers", key.as_str(), "requires_openai_auth"],
        toml_edit::value(true),
    )?;
    toml_set(
        doc,
        &["model_providers", key.as_str(), "experimental_bearer_token"],
        toml_edit::value(provider.api_key.as_str()),
    )?;
    Ok(())
}

fn write_codex_catalog(path: &Path, doc: &mut DocumentMut, provider: &Provider) -> Result<bool> {
    let entries: Vec<_> = provider
        .catalog
        .iter()
        .filter(|e| !e.id.trim().is_empty())
        .cloned()
        .collect();
    if entries.is_empty() {
        toml_remove(doc, &["model_catalog_json"])?;
        if path.exists() {
            fs::remove_file(path).map_err(|e| Error::io(path, e))?;
        }
        return Ok(false);
    }
    toml_set(
        doc,
        &["model_catalog_json"],
        toml_edit::value(CATALOG_FILENAME),
    )?;
    let models: Vec<Value> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| codex_catalog_entry(e, i))
        .collect();
    let body = serde_json::json!({ "models": models });
    write_live_json(path, &body)?;
    Ok(true)
}

/// Codex requires richer catalog rows than apmux stores (e.g.
/// `supported_reasoning_levels`, `base_instructions`): every row is a clone of
/// this native `/responses` template with the stored fields overlaid. Mirrors
/// cc-switch's approach. Never strip `base_instructions` — Codex refuses
/// catalog files without it.
const CATALOG_TEMPLATE: &str = include_str!("codex_native_responses.json");

/// Read stored catalog rows from a `model_catalog_json` file (the slim
/// user-facing subset; template-only fields are re-added on apply).
fn read_catalog_entries(path: &Path) -> Option<Vec<crate::store::ModelEntry>> {
    #[derive(serde::Deserialize)]
    struct Row {
        slug: String,
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        context_window: Option<u64>,
        #[serde(default)]
        max_tokens: Option<u64>,
    }
    let data = fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&data).ok()?;
    let models = value.get("models")?.as_array()?;
    Some(
        models
            .iter()
            .filter_map(|m| serde_json::from_value::<Row>(m.clone()).ok())
            .map(|r| crate::store::ModelEntry {
                id: r.slug,
                label: r.display_name,
                context_window: r.context_window,
                max_tokens: r.max_tokens,
                target_model_id: None,
            })
            .collect(),
    )
}

fn codex_catalog_entry(row: &crate::store::ModelEntry, idx: usize) -> Value {
    let label = row
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&row.id);
    let ctx = row.context_window.unwrap_or(128_000);
    let mut entry: Value =
        serde_json::from_str(CATALOG_TEMPLATE).expect("embedded codex template is valid JSON");
    let obj = entry.as_object_mut().expect("template root is an object");
    for (k, v) in [
        ("slug", json!(row.id)),
        ("display_name", json!(label)),
        ("description", json!(label)),
        ("context_window", json!(ctx)),
        ("max_context_window", json!(ctx)),
        ("priority", json!(1000 + idx)),
        ("additional_speed_tiers", json!([])),
        ("service_tiers", json!([])),
        ("availability_nux", Value::Null),
        ("upgrade", Value::Null),
    ] {
        obj.insert(k.to_string(), v);
    }
    // Defensive: gateways reject Codex's freeform custom tools; the embedded
    // template is already clean but guard against future drift.
    for k in ["apply_patch_tool_type", "web_search_tool_type", "tools"] {
        obj.remove(k);
    }
    entry
}

fn read_toml(path: &Path) -> Result<DocumentMut> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }
    let data = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    data.parse::<DocumentMut>()
        .map_err(|e| Error::toml(path, e).into())
}

fn read_json_object(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let data = fs::read(path).map_err(|e| Error::io(path, e))?;
    let value: Value = serde_json::from_slice(&data).map_err(|e| Error::json(path, e))?;
    if !value.is_object() {
        anyhow::bail!("{}: root must be a JSON object", path.display());
    }
    Ok(value)
}

fn write_live_toml(path: &Path, doc: &DocumentMut) -> Result<()> {
    let mut body = doc.to_string();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    fsutil::atomic_write_preserving_mode(path, body.as_bytes())
}

fn write_live_json(path: &Path, value: &Value) -> Result<()> {
    let mut body = serde_json::to_string_pretty(value).context("serialize live JSON")?;
    if !body.ends_with('\n') {
        body.push('\n');
    }
    fsutil::atomic_write_preserving_mode(path, body.as_bytes())
}

fn read_existing_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(path, e).into()),
    }
}

fn restore_file(path: &Path, old: Option<&[u8]>) -> Result<()> {
    match old {
        Some(bytes) => fsutil::atomic_write_preserving_mode(path, bytes),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(path, e).into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{get, parse_extras, registry};
    use crate::paths::EnvOverrides;
    use crate::store::AppId;
    use crate::switch;
    use std::collections::BTreeMap;

    fn setup() -> (tempfile::TempDir, Paths) {
        let td = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_test(td.path());
        (td, paths)
    }

    fn provider(model: Option<&str>) -> Provider {
        Provider {
            id: "packy-codex".into(),
            name: "PackyCode".into(),
            app: AppId::Codex,
            base_url: "https://api.example.com".into(),
            api_key: "sk-test-key-abcd".into(),
            model: model.map(str::to_string),
            extras: BTreeMap::new(),
            ..Provider::blank(AppId::Codex)
        }
    }

    fn golden(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden/codex")
            .join(name)
    }

    fn init_codex(paths: &Paths) {
        fs::create_dir_all(&paths.codex_dir).unwrap();
    }

    fn read_toml_file(path: &Path) -> DocumentMut {
        fs::read_to_string(path).unwrap().parse().unwrap()
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn resolved_dir_uses_paths_codex_dir() {
        let (_td, paths) = setup();
        let a = CodexAdapter;
        assert_eq!(a.resolved_dir(&paths), paths.codex_dir);
        assert!(!a.is_initialized(&paths));
    }

    #[test]
    fn uninitialized_apply_does_not_create_dir() {
        let (_td, paths) = setup();
        let a = CodexAdapter;
        let outcome = a.apply(&paths, &provider(None)).unwrap();
        assert_eq!(outcome, ApplyOutcome::SkippedUninitialized);
        assert!(!paths.codex_dir.exists());
        assert!(!a.config_file(&paths).exists());
        assert!(!a.auth_file(&paths).exists());
    }

    #[test]
    fn missing_files_treated_as_empty_new_auth_0600() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let outcome = CodexAdapter
            .apply(&paths, &provider(Some("gpt-5")))
            .unwrap();
        match outcome {
            ApplyOutcome::Applied { files } => {
                assert_eq!(
                    files,
                    vec![
                        paths.codex_dir.join("config.toml"),
                        paths.codex_dir.join("auth.json")
                    ]
                );
            }
            other => panic!("{other:?}"),
        }
        let doc = read_toml_file(&paths.codex_dir.join("config.toml"));
        let want = fs::read_to_string(golden("empty.after.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            doc["model_provider"].as_str(),
            want["model_provider"].as_str()
        );
        assert_eq!(doc["model"].as_str(), want["model"].as_str());
        assert_eq!(
            doc["model_providers"]["PackyCode"]["name"].as_str(),
            want["model_providers"]["PackyCode"]["name"].as_str()
        );
        assert_eq!(
            doc["model_providers"]["PackyCode"]["base_url"].as_str(),
            want["model_providers"]["PackyCode"]["base_url"].as_str()
        );
        assert_eq!(
            doc["model_providers"]["PackyCode"]["wire_api"].as_str(),
            want["model_providers"]["PackyCode"]["wire_api"].as_str()
        );
        assert_eq!(
            doc["model_providers"]["PackyCode"]["requires_openai_auth"].as_bool(),
            want["model_providers"]["PackyCode"]["requires_openai_auth"].as_bool()
        );
        let auth = read_json(&paths.codex_dir.join("auth.json"));
        let want_auth = read_json(&golden("empty_auth.after.json"));
        assert_eq!(auth, want_auth);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(paths.codex_dir.join("auth.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn mcp_servers_and_comments_preserved() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let live = paths.codex_dir.join("config.toml");
        fs::copy(golden("mcp_servers.before.toml"), &live).unwrap();
        CodexAdapter
            .apply(&paths, &provider(Some("gpt-5")))
            .unwrap();
        let text = fs::read_to_string(&live).unwrap();
        assert!(text.contains("# keep this comment"), "{text}");
        assert!(text.contains("# another comment"), "{text}");
        let got = text.parse::<DocumentMut>().unwrap();
        let want = fs::read_to_string(golden("mcp_servers.after.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            got["model_provider"].as_str(),
            want["model_provider"].as_str()
        );
        assert_eq!(got["model"].as_str(), want["model"].as_str());
        assert_eq!(
            got["mcp_servers"]["docs"]["command"].as_str(),
            want["mcp_servers"]["docs"]["command"].as_str()
        );
        assert_eq!(
            got["mcp_servers"]["docs"]["args"]
                .as_array()
                .unwrap()
                .iter()
                .next()
                .and_then(|v| v.as_str()),
            Some("mcp-server")
        );
        assert_eq!(
            got["model_providers"]["openai"]["name"].as_str(),
            want["model_providers"]["openai"]["name"].as_str()
        );
        assert_eq!(
            got["model_providers"]["PackyCode"]["name"].as_str(),
            want["model_providers"]["PackyCode"]["name"].as_str()
        );
        assert_eq!(
            got["model_providers"]["PackyCode"]["base_url"].as_str(),
            want["model_providers"]["PackyCode"]["base_url"].as_str()
        );
        assert_eq!(
            got["model_providers"]["PackyCode"]["wire_api"].as_str(),
            want["model_providers"]["PackyCode"]["wire_api"].as_str()
        );
        assert_eq!(
            got["model_providers"]["PackyCode"]["requires_openai_auth"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn model_none_deletes_live_key() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let live = paths.codex_dir.join("config.toml");
        fs::copy(golden("model_none.before.toml"), &live).unwrap();
        CodexAdapter.apply(&paths, &provider(None)).unwrap();
        let text = fs::read_to_string(&live).unwrap();
        let got = text.parse::<DocumentMut>().unwrap();
        let want = fs::read_to_string(golden("model_none.after.toml"))
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert!(got.get("model").is_none(), "{text}");
        assert!(want.get("model").is_none());
        assert_eq!(got["model_provider"].as_str(), Some("PackyCode"));
        assert_eq!(
            got["model_providers"]["openai"]["name"].as_str(),
            Some("OpenAI")
        );
        assert_eq!(
            got["model_providers"]["PackyCode"]["name"].as_str(),
            Some("PackyCode")
        );
    }

    #[test]
    fn auth_json_merges_key_keeps_unknown() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let auth = paths.codex_dir.join("auth.json");
        fs::copy(golden("preserve_auth.before.json"), &auth).unwrap();
        CodexAdapter.apply(&paths, &provider(None)).unwrap();
        let got = read_json(&auth);
        let want = read_json(&golden("preserve_auth.after.json"));
        assert_eq!(got, want);
    }

    #[test]
    fn catalog_writes_json_file_without_max_tokens() {
        use crate::store::ModelEntry;
        let (_td, paths) = setup();
        init_codex(&paths);
        let mut p = provider(Some("gpt-5"));
        p.catalog = vec![ModelEntry {
            id: "gpt-5".into(),
            label: Some("GPT-5".into()),
            context_window: Some(192_000),
            max_tokens: Some(999),
            target_model_id: None,
        }];
        let outcome = CodexAdapter.apply(&paths, &p).unwrap();
        match outcome {
            ApplyOutcome::Applied { files } => {
                assert!(files.contains(&paths.codex_dir.join(CATALOG_FILENAME)));
            }
            other => panic!("{other:?}"),
        }
        let doc = read_toml_file(&paths.codex_dir.join("config.toml"));
        assert_eq!(doc["model_catalog_json"].as_str(), Some(CATALOG_FILENAME));
        let catalog = read_json(&paths.codex_dir.join(CATALOG_FILENAME));
        assert_eq!(catalog["models"][0]["slug"], "gpt-5");
        assert_eq!(catalog["models"][0]["display_name"], "GPT-5");
        assert_eq!(catalog["models"][0]["context_window"], 192000);
        assert_eq!(catalog["models"][0]["max_context_window"], 192000);
        assert!(catalog["models"][0].get("max_tokens").is_none());
        assert!(catalog["models"][0].get("max_output_tokens").is_none());
        // Fields Codex's catalog parser requires (regression: live Codex
        // refused apmux's minimal rows with "missing field
        // `supported_reasoning_levels`").
        assert_eq!(
            catalog["models"][0]["supported_reasoning_levels"][1]["effort"],
            "high"
        );
        assert!(!catalog["models"][0]["base_instructions"]
            .as_str()
            .unwrap()
            .is_empty());
        assert_eq!(catalog["models"][0]["shell_type"], "shell_command");
    }

    #[test]
    fn catalog_rows_are_independent_clones() {
        use crate::store::ModelEntry;
        let a = codex_catalog_entry(
            &ModelEntry {
                id: "a".into(),
                context_window: Some(1_000),
                ..ModelEntry::default()
            },
            0,
        );
        let b = codex_catalog_entry(
            &ModelEntry {
                id: "b".into(),
                ..ModelEntry::default()
            },
            1,
        );
        assert_eq!(a["slug"], "a");
        assert_eq!(b["slug"], "b");
        assert_eq!(b["context_window"], 128000); // default applied per row
        assert_ne!(a["priority"], b["priority"]);
    }

    #[test]
    fn remote_compaction_writes_openai_name() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let mut p = provider(None);
        p.extras.insert("remote_compaction".into(), "true".into());
        CodexAdapter.apply(&paths, &p).unwrap();
        let doc = read_toml_file(&paths.codex_dir.join("config.toml"));
        assert_eq!(
            doc["model_providers"]["PackyCode"]["name"].as_str(),
            Some("OpenAI")
        );
    }

    #[test]
    fn leftover_wire_api_chat_still_writes_responses() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let mut p = provider(None);
        p.extras.insert("wire_api".into(), "chat".into());
        CodexAdapter.apply(&paths, &p).unwrap();
        let doc = read_toml_file(&paths.codex_dir.join("config.toml"));
        assert_eq!(
            doc["model_providers"]["PackyCode"]["wire_api"].as_str(),
            Some("responses")
        );
    }

    #[test]
    fn unknown_extras_ignored() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let mut p = provider(None);
        p.extras.insert("nope".into(), "xyz".into());
        CodexAdapter.validate(&p).unwrap();
        CodexAdapter.apply(&paths, &p).unwrap();
        let text = fs::read_to_string(paths.codex_dir.join("config.toml")).unwrap();
        assert!(!text.contains("nope"));
        let auth = read_json(&paths.codex_dir.join("auth.json"));
        assert!(auth.get("nope").is_none());
    }

    #[test]
    fn existing_slot_table_keeps_unknown_keys() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let live = paths.codex_dir.join("config.toml");
        fs::write(
            &live,
            r#"
[model_providers.PackyCode]
name = "old"
env_key = "CUSTOM"
"#,
        )
        .unwrap();
        CodexAdapter.apply(&paths, &provider(None)).unwrap();
        let doc = read_toml_file(&live);
        assert_eq!(
            doc["model_providers"]["PackyCode"]["name"].as_str(),
            Some("PackyCode")
        );
        assert_eq!(
            doc["model_providers"]["PackyCode"]["env_key"].as_str(),
            Some("CUSTOM")
        );
    }

    #[test]
    fn rescue_imports_custom_provider_tables() {
        // A hand-configured user's config.toml: one active custom provider
        // plus a second inactive requires_openai_auth table. The legacy
        // auth.json key is still accepted when no TOML token is present.
        let (_td, paths) = setup();
        init_codex(&paths);
        fs::write(
            paths.codex_dir.join("config.toml"),
            r#"
model_provider = "Agate"
model = "mimo-v2.5"
model_catalog_json = "catalog.json"

[model_providers.Agate]
name = "Agate"
base_url = "https://agate.example.com/v1"
requires_openai_auth = true

[model_providers.OpenRouter]
name = "OpenRouter"
base_url = "https://openrouter.example.com/v1"
"#,
        )
        .unwrap();
        fs::write(
            paths.codex_dir.join("auth.json"),
            br#"{"OPENAI_API_KEY":"sk-live"}"#,
        )
        .unwrap();
        fs::write(
            paths.codex_dir.join("catalog.json"),
            r#"{"models":[{"slug":"mimo-v2.5","display_name":"MiMo","context_window":500000}]}"#,
        )
        .unwrap();

        let rows = CodexAdapter.rescue(&paths);
        assert_eq!(rows.len(), 2);

        let agate = rows.iter().find(|r| r.provider.name == "Agate").unwrap();
        assert!(agate.active);
        assert_eq!(agate.provider.base_url, "https://agate.example.com/v1");
        // Legacy requires_openai_auth rows read the shared auth.json key.
        assert_eq!(agate.provider.api_key, "sk-live");
        assert_eq!(agate.provider.model.as_deref(), Some("mimo-v2.5"));
        assert_eq!(agate.provider.catalog.len(), 1);
        assert_eq!(agate.provider.catalog[0].id, "mimo-v2.5");
        assert_eq!(agate.provider.catalog[0].context_window, Some(500000));

        let openrouter = rows
            .iter()
            .find(|r| r.provider.name == "OpenRouter")
            .unwrap();
        assert!(!openrouter.active);
        // No requires_openai_auth and no TOML token: rescue has no key to claim.
        assert_eq!(openrouter.provider.api_key, "");
        assert_eq!(openrouter.provider.model, None);
        assert_eq!(openrouter.provider.catalog.len(), 0);
    }

    #[test]
    fn rescue_then_use_login_row_by_name() {
        // A hand-configured user with two requires_openai_auth rows: rescue
        // must give both the shared auth.json key so switching to either
        // passes validation (regression for "Switch failed: api_key must
        // not be empty" on a rescued row).
        let (_td, paths) = setup();
        init_codex(&paths);
        fs::write(
            paths.codex_dir.join("config.toml"),
            r#"
model_provider = "Agate"

[model_providers.Agate]
name = "Agate"
base_url = "https://agate.example.com/v1"
wire_api = "responses"
requires_openai_auth = true

[model_providers.OpenRouter]
name = "OpenRouter"
base_url = "https://openrouter.example.com/v1"
wire_api = "responses"
requires_openai_auth = true
"#,
        )
        .unwrap();
        fs::write(
            paths.codex_dir.join("auth.json"),
            br#"{"OPENAI_API_KEY":"sk-live"}"#,
        )
        .unwrap();

        let mut store = crate::store::Store::empty();
        switch::rescue_from_live(&paths, &mut store).unwrap();
        let openrouter = switch::resolve(&store, "OpenRouter", None).unwrap();
        assert_eq!(openrouter.api_key, "sk-live");

        switch::use_provider(&paths, &mut store, "OpenRouter", None).unwrap();
        let doc = read_toml_file(&paths.codex_dir.join("config.toml"));
        assert_eq!(
            doc["model_provider"].as_str(),
            Some("OpenRouter") as Option<&str>
        );
    }

    #[test]
    fn rescue_prefers_provider_toml_token_and_supports_top_level_token() {
        let (_td, paths) = setup();
        init_codex(&paths);
        fs::write(
            paths.codex_dir.join("config.toml"),
            r#"
model_provider = "OpenRouter"
experimental_bearer_token = "sk-top-level"

[model_providers.Agate]
name = "Agate"
base_url = "https://agate.example.com/v1"
experimental_bearer_token = "sk-agate"

[model_providers.OpenRouter]
name = "OpenRouter"
base_url = "https://openrouter.example.com/v1"
requires_openai_auth = true
"#,
        )
        .unwrap();
        fs::write(
            paths.codex_dir.join("auth.json"),
            br#"{"OPENAI_API_KEY":"sk-legacy"}"#,
        )
        .unwrap();

        let rows = CodexAdapter.rescue(&paths);
        assert_eq!(
            rows.iter()
                .find(|row| row.provider.name == "Agate")
                .unwrap()
                .provider
                .api_key,
            "sk-agate"
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.provider.name == "OpenRouter")
                .unwrap()
                .provider
                .api_key,
            "sk-top-level"
        );
    }

    #[test]
    fn apply_writes_new_and_legacy_codex_tokens() {
        let (_td, paths) = setup();
        init_codex(&paths);

        CodexAdapter.apply(&paths, &provider(None)).unwrap();

        let doc = read_toml_file(&paths.codex_dir.join("config.toml"));
        assert_eq!(
            doc["model_providers"]["PackyCode"]["experimental_bearer_token"].as_str(),
            Some("sk-test-key-abcd")
        );
        let auth = read_json(&paths.codex_dir.join("auth.json"));
        assert_eq!(auth["OPENAI_API_KEY"], "sk-test-key-abcd");
    }

    #[test]
    fn rescue_skips_builtin_and_missing_files() {
        let (_td, paths) = setup();
        init_codex(&paths);
        fs::write(
            paths.codex_dir.join("config.toml"),
            r#"
model_provider = "openai"

[model_providers.openai]
base_url = "https://api.openai.com/v1"
"#,
        )
        .unwrap();
        assert!(CodexAdapter.rescue(&paths).is_empty());

        let (_td, paths) = setup();
        assert!(CodexAdapter.rescue(&paths).is_empty());
    }

    #[test]
    fn corrupt_toml_writes_nothing() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let config = paths.codex_dir.join("config.toml");
        let auth = paths.codex_dir.join("auth.json");
        let bytes = fs::read(golden("corrupt.toml")).unwrap();
        fs::write(&config, &bytes).unwrap();
        fs::write(&auth, br#"{"OPENAI_API_KEY":"old"}"#).unwrap();
        let auth_before = fs::read(&auth).unwrap();
        let err = CodexAdapter.apply(&paths, &provider(None)).unwrap_err();
        assert!(
            err.to_string().contains("config.toml"),
            "error should name the path: {err}"
        );
        assert_eq!(fs::read(&config).unwrap(), bytes);
        assert_eq!(fs::read(&auth).unwrap(), auth_before);
    }

    #[test]
    fn corrupt_auth_json_writes_nothing() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let config = paths.codex_dir.join("config.toml");
        let auth = paths.codex_dir.join("auth.json");
        fs::write(&config, "model = \"gpt-4\"\n").unwrap();
        let config_before = fs::read(&config).unwrap();
        let bytes = fs::read(golden("corrupt.json")).unwrap();
        fs::write(&auth, &bytes).unwrap();
        let err = CodexAdapter.apply(&paths, &provider(None)).unwrap_err();
        assert!(
            err.to_string().contains("auth.json"),
            "error should name the path: {err}"
        );
        assert_eq!(fs::read(&config).unwrap(), config_before);
        assert_eq!(fs::read(&auth).unwrap(), bytes);
    }

    #[test]
    fn model_providers_not_table_writes_nothing() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let config = paths.codex_dir.join("config.toml");
        let auth = paths.codex_dir.join("auth.json");
        let bytes = b"model_providers = \"nope\"\n";
        fs::write(&config, bytes).unwrap();
        fs::write(&auth, b"{}\n").unwrap();
        let auth_before = fs::read(&auth).unwrap();
        let err = CodexAdapter.apply(&paths, &provider(None)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("expected table at model_providers"), "{msg}");
        assert!(msg.contains("config.toml"), "{msg}");
        assert_eq!(fs::read(&config).unwrap(), bytes);
        assert_eq!(fs::read(&auth).unwrap(), auth_before);
    }

    #[test]
    fn second_write_failure_rolls_back_auth() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let config = paths.codex_dir.join("config.toml");
        let auth = paths.codex_dir.join("auth.json");
        fs::write(&config, "model = \"old\"\n").unwrap();
        fs::write(
            &auth,
            br#"{"OPENAI_API_KEY":"old-key","account_id":"keep"}"#,
        )
        .unwrap();
        let config_before = fs::read(&config).unwrap();
        let auth_before = fs::read(&auth).unwrap();
        fsutil::fail_before_rename_nth(2);
        let err = CodexAdapter.apply(&paths, &provider(None)).unwrap_err();
        assert!(err.to_string().contains("injected failure"), "{err}");
        assert_eq!(fs::read(&config).unwrap(), config_before);
        assert_eq!(fs::read(&auth).unwrap(), auth_before);
    }

    #[test]
    fn second_write_and_restore_failure_reports_split_live() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let config = paths.codex_dir.join("config.toml");
        let auth = paths.codex_dir.join("auth.json");
        fs::write(&config, "model = \"old\"\n").unwrap();
        fs::write(&auth, br#"{"OPENAI_API_KEY":"old-key"}"#).unwrap();
        let config_before = fs::read(&config).unwrap();
        fsutil::fail_before_rename_from_nth(2);
        let result = CodexAdapter.apply(&paths, &provider(None));
        fsutil::fail_before_rename(false);
        let err = result.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to restore auth.json after config.toml write failed"),
            "{msg}"
        );
        assert!(msg.contains("injected failure"), "{msg}");
        assert!(msg.contains(&auth.display().to_string()), "{msg}");
        assert_eq!(fs::read(&config).unwrap(), config_before);
        let got: Value = serde_json::from_slice(&fs::read(&auth).unwrap()).unwrap();
        assert_eq!(got["OPENAI_API_KEY"], "sk-test-key-abcd");
    }

    #[test]
    fn second_write_failure_deletes_new_auth() {
        let (_td, paths) = setup();
        init_codex(&paths);
        let config = paths.codex_dir.join("config.toml");
        fs::write(&config, "model = \"old\"\n").unwrap();
        let config_before = fs::read(&config).unwrap();
        fsutil::fail_before_rename_nth(2);
        let err = CodexAdapter.apply(&paths, &provider(None)).unwrap_err();
        assert!(err.to_string().contains("injected failure"), "{err}");
        assert_eq!(fs::read(&config).unwrap(), config_before);
        assert!(!paths.codex_dir.join("auth.json").exists());
    }

    #[test]
    fn codex_home_not_a_dir_falls_back_to_home_codex() {
        let td = tempfile::tempdir().expect("tempdir");
        let home = td.path().join("home");
        fs::create_dir_all(home.join(".codex")).unwrap();
        let not_dir = td.path().join("codex-file");
        fs::write(&not_dir, b"not-a-dir").unwrap();
        let paths = Paths::from_home_and_env(
            home.clone(),
            EnvOverrides {
                codex_home: Some(not_dir.display().to_string()),
                ..EnvOverrides::default()
            },
        )
        .unwrap();
        let a = CodexAdapter;
        assert_eq!(a.resolved_dir(&paths), home.join(".codex"));
        a.apply(&paths, &provider(None)).unwrap();
        assert!(home.join(".codex").join("config.toml").exists());
        assert_eq!(fs::read(&not_dir).unwrap(), b"not-a-dir");
    }

    #[test]
    fn codex_home_missing_falls_back_and_skips_if_default_missing() {
        let td = tempfile::tempdir().expect("tempdir");
        let home = td.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let missing = td.path().join("codex-missing");
        let paths = Paths::from_home_and_env(
            home.clone(),
            EnvOverrides {
                codex_home: Some(missing.display().to_string()),
                ..EnvOverrides::default()
            },
        )
        .unwrap();
        let a = CodexAdapter;
        assert_eq!(a.resolved_dir(&paths), home.join(".codex"));
        let outcome = a.apply(&paths, &provider(None)).unwrap();
        assert_eq!(outcome, ApplyOutcome::SkippedUninitialized);
        assert!(!home.join(".codex").exists());
        assert!(!missing.exists());
    }

    #[test]
    fn real_codex_home_dir_is_used_not_home_dot_codex() {
        let td = tempfile::tempdir().expect("tempdir");
        let home = td.path().join("home");
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex").join("config.toml"),
            "model = \"home\"\n",
        )
        .unwrap();
        let real = td.path().join("codex-real");
        fs::create_dir_all(&real).unwrap();
        let paths = Paths::from_home_and_env(
            home.clone(),
            EnvOverrides {
                codex_home: Some(real.display().to_string()),
                ..EnvOverrides::default()
            },
        )
        .unwrap();
        CodexAdapter.apply(&paths, &provider(None)).unwrap();
        assert!(real.join("config.toml").exists());
        assert_eq!(
            fs::read_to_string(home.join(".codex").join("config.toml")).unwrap(),
            "model = \"home\"\n"
        );
    }

    #[test]
    fn official_apply_restores_native_codex_auth() {
        use crate::store::ModelEntry;
        let (_td, paths) = setup();
        init_codex(&paths);
        // Third-party state first: provider block, model, key, catalog —
        // applied through the normal switch path so slot bookkeeping runs.
        let mut p = provider(Some("gpt-5"));
        p.catalog = vec![ModelEntry {
            id: "gpt-5".into(),
            ..ModelEntry::default()
        }];
        let mut store = crate::store::Store::empty();
        store.providers.insert(p.id.clone(), p.clone());
        crate::switch::use_provider(&paths, &mut store, &p.id, None).unwrap();

        // ChatGPT OAuth login material sits in auth.json (user ran codex login).
        let auth_path = paths.codex_dir.join("auth.json");
        fs::write(
            &auth_path,
            r#"{"tokens":{"id":"t"},"OPENAI_API_KEY":"sk-third"}"#,
        )
        .unwrap();

        let official = crate::store::official_provider(AppId::Codex).unwrap();
        store.providers.insert(official.id.clone(), official);
        crate::switch::use_provider(&paths, &mut store, "codex-official", None).unwrap();

        let doc = read_toml_file(&paths.codex_dir.join("config.toml"));
        assert!(doc.get("model_provider").is_none());
        assert!(doc.get("model").is_none());
        assert!(doc["model_providers"].get("PackyCode").is_none());
        assert!(!paths.codex_dir.join(CATALOG_FILENAME).exists());

        let auth = read_json(&auth_path);
        assert!(auth.get("OPENAI_API_KEY").is_none()); // third-party key gone
        assert_eq!(auth["tokens"]["id"], "t"); // native login preserved
    }

    #[test]
    fn validate_rejects_empty() {
        let mut p = provider(None);
        p.name.clear();
        assert!(CodexAdapter.validate(&p).is_err());
        p = provider(None);
        p.base_url.clear();
        assert!(CodexAdapter.validate(&p).is_err());
        p = provider(None);
        p.api_key.clear();
        assert!(CodexAdapter.validate(&p).is_err());
        p = provider(None);
        p.extras.insert("wire_api".into(), "chat".into());
        CodexAdapter.validate(&p).unwrap();
    }

    #[test]
    fn parse_extras_has_no_wire_api() {
        let adapter = get(AppId::Codex).unwrap();
        let err = parse_extras(adapter, &["wire_api=chat".into()]).unwrap_err();
        assert!(err.to_string().contains("unknown extra field"));
        assert!(!adapter.fields().iter().any(|f| f.key == "wire_api"));
    }

    #[cfg(unix)]
    #[test]
    fn existing_live_perms_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let (_td, paths) = setup();
        init_codex(&paths);
        let config = paths.codex_dir.join("config.toml");
        let auth = paths.codex_dir.join("auth.json");
        fs::write(&config, b"model = \"old\"\n").unwrap();
        fs::write(&auth, b"{}\n").unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&auth, fs::Permissions::from_mode(0o644)).unwrap();
        CodexAdapter.apply(&paths, &provider(None)).unwrap();
        let config_mode = fs::metadata(&config).unwrap().permissions().mode() & 0o777;
        let auth_mode = fs::metadata(&auth).unwrap().permissions().mode() & 0o777;
        assert_eq!(config_mode, 0o644);
        assert_eq!(auth_mode, 0o644);
    }

    #[test]
    fn isolation_apply_does_not_touch_host() {
        let (_td, paths) = setup();
        init_codex(&paths);
        CodexAdapter.apply(&paths, &provider(None)).unwrap();
        crate::fsutil::panic_if_host_config_path(&paths.codex_dir.join("config.toml"));
        crate::fsutil::panic_if_host_config_path(&paths.codex_dir.join("auth.json"));
        let host = dirs::home_dir().expect("home").join(".codex");
        assert_ne!(paths.codex_dir, host);
    }

    #[test]
    fn registry_codex_fields() {
        let a = get(AppId::Codex).unwrap();
        assert_eq!(a.display_name(), "Codex");
        assert_eq!(a.fields().len(), 4);
        assert!(!a.fields().iter().any(|f| f.key == "wire_api"));
        assert!(registry().iter().any(|x| x.id() == AppId::Codex));
        assert!(registry().iter().any(|x| x.id() == AppId::Claude));
    }
}
