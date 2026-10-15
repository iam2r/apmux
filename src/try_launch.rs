//! `apmux try <PROVIDER> [-- <args…>]`: launch a CLI against a provider
//! without touching the live config. Each app gets a throwaway config
//! directory (or single file) selected via its official override env var —
//! the same isolation trick cc-switch-cli uses for Codex (`CODEX_HOME`) —
//! and the real binary runs attached to this terminal. When it exits, the
//! temporary directory is removed. Live configs are never read or written.

use crate::adapter::protocol;
use crate::store::{AppId, Provider, Store};
use crate::switch;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::process::Command;

/// Build the throwaway config payload for one provider.
/// Returns (relative file name, contents) pairs to place in the temp dir,
/// plus the env var name that redirects the CLI there (or to the file).
fn payload(provider: &Provider) -> Result<(Vec<(String, String)>, &'static str)> {
    if provider.official {
        anyhow::bail!(
            "'{}' is the official native-login row — there is nothing to try against",
            provider.name
        );
    }
    match provider.app {
        AppId::Claude => Ok((
            vec![(
                "settings.json".into(),
                claude_settings(provider)?.to_string(),
            )],
            "CLAUDE_CONFIG_DIR",
        )),
        AppId::Codex => Ok((
            vec![
                ("config.toml".into(), codex_config(provider)?),
                (
                    "auth.json".into(),
                    json!({"OPENAI_API_KEY": provider.api_key}).to_string(),
                ),
            ],
            "CODEX_HOME",
        )),
        AppId::OpenCode => Ok((
            vec![(
                "opencode.json".into(),
                opencode_config(provider)?.to_string(),
            )],
            "OPENCODE_CONFIG",
        )),
        AppId::Pi => Ok((
            vec![
                ("settings.json".into(), pi_settings(provider)?.to_string()),
                ("models.json".into(), pi_models(provider)?.to_string()),
            ],
            "PI_CODING_AGENT_DIR",
        )),
    }
}

fn claude_settings(provider: &Provider) -> Result<Value> {
    Ok(json!({
        "env": {
            "ANTHROPIC_BASE_URL": provider.base_url,
            "ANTHROPIC_AUTH_TOKEN": provider.api_key,
        }
    }))
}

fn codex_config(provider: &Provider) -> Result<String> {
    let mut doc = toml_edit::DocumentMut::new();
    let key = provider.slot_key();
    doc["model_provider"] = toml_edit::value(key.as_str());
    let model = provider.model.as_deref().unwrap_or_default();
    doc["model"] = toml_edit::value(model);
    let tbl = &mut doc["model_providers"][key.as_str()];
    tbl["name"] = toml_edit::value(provider.name.as_str());
    tbl["base_url"] = toml_edit::value(provider.base_url.as_str());
    tbl["wire_api"] = toml_edit::value("responses");
    tbl["experimental_bearer_token"] = toml_edit::value(provider.api_key.as_str());
    // Keep the legacy auth.json payload too for older Codex versions.
    tbl["requires_openai_auth"] = toml_edit::value(true);
    Ok(doc.to_string())
}

fn opencode_config(provider: &Provider) -> Result<Value> {
    let model_id = provider
        .model
        .as_deref()
        .filter(|s| !s.is_empty())
        .context("provider has no default model to try")?;
    let protocol = protocol::from_extras(&provider.extras).unwrap_or(protocol::DEFAULT);
    let npm = protocol::opencode_npm(protocol);
    let key = provider.slot_key();
    let mut models = serde_json::Map::new();
    models.insert(model_id.to_string(), json!({}));
    Ok(json!({
        "provider": {
            key.as_str(): {
                "npm": npm,
                "name": provider.name,
                "options": {"baseURL": provider.base_url, "apiKey": provider.api_key},
                "models": models,
            }
        },
        "model": format!("{key}/{model_id}"),
    }))
}

fn pi_settings(provider: &Provider) -> Result<Value> {
    let model = provider
        .model
        .as_deref()
        .filter(|s| !s.is_empty())
        .context("provider has no default model to try")?;
    let key = provider.slot_key();
    Ok(json!({"defaultProvider": key.as_str(), "defaultModel": model}))
}

fn pi_models(provider: &Provider) -> Result<Value> {
    let protocol = protocol::from_extras(&provider.extras).unwrap_or(protocol::DEFAULT);
    let api = protocol::pi_api(protocol);
    let key = provider.slot_key();
    let model_id = provider
        .model
        .as_deref()
        .filter(|s| !s.is_empty())
        .context("provider has no default model to try")?;
    Ok(json!({
        "providers": {
            key.as_str(): {
                "name": provider.name,
                "baseUrl": provider.base_url,
                "api": api,
                "apiKey": provider.api_key,
                "models": [{"id": model_id}],
            }
        }
    }))
}

fn resolve_bin(bin_override: Option<&str>, app: AppId) -> Result<std::path::PathBuf> {
    let name = bin_override
        .map(str::to_string)
        .unwrap_or_else(|| app.to_string());
    let path = which::which(&name)
        .with_context(|| format!("'{name}' not found on PATH — install it first or pass --bin"))?;
    Ok(path)
}

/// Stage a provider's throwaway config into a temp dir.
/// Returns the dir plus the env var that points the CLI at it.
fn stage(provider: &Provider) -> Result<(tempfile::TempDir, &'static str)> {
    let (files, env_var) = payload(provider)?;
    let tmp = tempfile::Builder::new()
        .prefix(concat!("apmux-try-", env!("CARGO_PKG_NAME"), "-"))
        .tempdir()
        .context("create temp dir for trial launch")?;
    for (name, contents) in &files {
        std::fs::write(tmp.path().join(name), contents)
            .with_context(|| format!("write {}", tmp.path().join(name).display()))?;
    }
    Ok((tmp, env_var))
}

/// Run `bin` with the staged dir wired through `env_var`.
fn launch(
    bin: &std::path::Path,
    args: &[String],
    tmp: &tempfile::TempDir,
    env_var: &str,
) -> Result<std::process::ExitStatus> {
    Command::new(bin)
        .args(args)
        .env(env_var, tmp.path())
        .status()
        .with_context(|| format!("launch {}", bin.display()))
}

/// A staged trial launch queued by the TUI: everything is prepared before
/// the terminal is suspended so failures surface in the status bar.
pub struct TryJob {
    pub provider_name: String,
    bin: std::path::PathBuf,
    args: Vec<String>,
    tmp: tempfile::TempDir,
    env_var: &'static str,
}

impl TryJob {
    /// Validate + stage a trial launch for one provider id.
    pub fn for_provider(store: &Store, id: &str) -> Result<TryJob> {
        let provider = switch::resolve(store, id, None)?.clone();
        // resolve the binary up front: failing here keeps the TUI alive
        let bin = resolve_bin(None, provider.app)?;
        let (tmp, env_var) = stage(&provider)?;
        Ok(TryJob {
            provider_name: provider.name.clone(),
            bin,
            args: Vec::new(),
            tmp,
            env_var,
        })
    }

    pub fn run_detached(&self) -> Result<std::process::ExitStatus> {
        Command::new(&self.bin)
            .args(&self.args)
            .env(self.env_var, self.tmp.path())
            .env_remove("NO_COLOR")
            .status()
            .with_context(|| format!("launch {}", self.bin.display()))
    }
}

/// Resolve + stage + launch. Returns the CLI's exit status.
/// `bin_override` exists so tests can point at a fake binary.
pub fn run(query: &str, args: &[String], bin_override: Option<&str>) -> Result<i32> {
    let (_paths, store) = crate::load_store()?;
    let provider = switch::resolve(&store, query, None)?.clone();
    let bin = resolve_bin(bin_override, provider.app)?;
    let (tmp, env_var) = stage(&provider)?;

    let status = launch(&bin, args, &tmp, env_var)?;

    println!(
        "{}",
        crate::i18n::tf(
            "status.try_done",
            &[
                &provider.name,
                &status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into())
            ]
        )
    );
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(app: AppId) -> Provider {
        Provider {
            id: format!("{app}-pk"),
            name: "pk".into(),
            app,
            base_url: "https://relay.example.com/v1".into(),
            api_key: "sk-test".into(),
            model: Some("gpt-x".into()),
            ..Provider::blank(app)
        }
    }

    #[test]
    fn official_row_has_nothing_to_try() {
        let mut p = provider(AppId::Codex);
        p.official = true;
        assert!(payload(&p).is_err());
    }

    #[test]
    fn claude_payload_redirects_config_dir() {
        let (files, var) = payload(&provider(AppId::Claude)).unwrap();
        assert_eq!(var, "CLAUDE_CONFIG_DIR");
        let v: Value = serde_json::from_str(&files[0].1).unwrap();
        assert_eq!(
            v["env"]["ANTHROPIC_BASE_URL"],
            "https://relay.example.com/v1"
        );
        assert!(v["env"]["ANTHROPIC_AUTH_TOKEN"].as_str().unwrap().len() > 3);
    }

    #[test]
    fn codex_payload_is_minimal_and_valid() {
        let (files, var) = payload(&provider(AppId::Codex)).unwrap();
        assert_eq!(var, "CODEX_HOME");
        let names: Vec<_> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["config.toml", "auth.json"]);
        let cfg: toml_edit::DocumentMut = files[0].1.parse().unwrap();
        assert_eq!(cfg["model_provider"].as_str(), Some("pk"));
        assert_eq!(
            cfg["model_providers"]["pk"]["base_url"].as_str(),
            Some("https://relay.example.com/v1")
        );
        assert_eq!(
            cfg["model_providers"]["pk"]["wire_api"].as_str(),
            Some("responses")
        );
        let auth: Value = serde_json::from_str(&files[1].1).unwrap();
        assert_eq!(auth["OPENAI_API_KEY"], "sk-test");
    }

    #[test]
    fn opencode_payload_sets_model_ref() {
        let (files, var) = payload(&provider(AppId::OpenCode)).unwrap();
        assert_eq!(var, "OPENCODE_CONFIG");
        let v: Value = serde_json::from_str(&files[0].1).unwrap();
        assert_eq!(v["model"], "pk/gpt-x");
        assert_eq!(v["provider"]["pk"]["options"]["apiKey"], "sk-test");
    }

    #[test]
    fn pi_payload_covers_settings_and_models() {
        let (files, var) = payload(&provider(AppId::Pi)).unwrap();
        assert_eq!(var, "PI_CODING_AGENT_DIR");
        let settings: Value = serde_json::from_str(&files[0].1).unwrap();
        assert_eq!(settings["defaultProvider"], "pk");
        let models: Value = serde_json::from_str(&files[1].1).unwrap();
        assert_eq!(models["providers"]["pk"]["apiKey"], "sk-test");
        assert_eq!(models["providers"]["pk"]["models"][0]["id"], "gpt-x");
    }

    #[test]
    #[cfg(unix)] // the fake CLI is a #!/bin/sh script; Windows cannot exec it
    fn end_to_end_launch_uses_isolated_env() {
        let (tmp, var) = stage(&provider(AppId::Codex)).unwrap();

        // fake "codex": verify the config landed inside the redirected home
        // and that we inherit its exit code untouched
        let fake_dir = tempfile::tempdir().unwrap();
        let fake = fake_dir.path().join("fake-codex");
        std::fs::write(
            &fake,
            "#!/bin/sh\ntest -f \"$APMUX_TRY_HOME/config.toml\" && test -f \"$APMUX_TRY_HOME/auth.json\" || echo MISSING_FILES\nexit 7\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let status = launch(fake.as_path(), &[], &tmp, var).unwrap();
        assert_eq!(status.code(), Some(7));

        // TempDir cleans up on drop; nothing is left behind
        let path = tmp.path().to_path_buf();
        drop(tmp);
        assert!(!path.exists());
    }
}
