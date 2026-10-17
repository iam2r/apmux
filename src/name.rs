//! Single source of truth for the product name and everything derived from it.
//!
//! # Rename policy
//!
//! To rename the tool, change **only** the two package macros at the top:
//! `pkg!()` (lowercase, used in file/dir names, namespace, assets) and
//! `envpref!()` (uppercase, used in environment variable prefixes). Everything
//! else below is derived via `concat!` at compile time.
//!
//! Data-layer values (store schema, manifest format, slot keys, catalog file)
//! are deliberately **package-name-free**: they ride along across renames and
//! are reads-compatible with every prior format.

// --- single editable source --------------------------------------------------

macro_rules! pkg {
    () => {
        "apmux"
    };
}

macro_rules! envpref {
    () => {
        "APMUX"
    };
}

// --- derived constants (binary name, files, namespaces, env keys) ------------

/// Binary and display name (`apmux use <id>`, help header).
pub const NAME: &str = pkg!();

/// Config directory under `$HOME` (`~/.apmux`), overridable via
/// [`ENV_CONFIG_DIR`].
pub const DOT_DIR: &str = concat!(".", pkg!());

/// Log file inside the config dir.
pub const LOG_FILE: &str = concat!(pkg!(), ".log");

/// WebDAV collection namespace under the user-provided root. This is the
/// only cloud-side name coupled to the package name.
pub const SYNC_NAMESPACE: &str = concat!(pkg!(), "-sync");

// --- environment variables (derived with the uppercase prefix) ---------------

/// Overrides the config directory location.
pub const ENV_CONFIG_DIR: &str = concat!(envpref!(), "_CONFIG_DIR");

/// UI language override.
pub const ENV_LANG: &str = concat!(envpref!(), "_LANG");

/// Set to `1` to print full API keys (dangerous).
pub const ENV_SHOW_SECRETS: &str = concat!(envpref!(), "_SHOW_SECRETS");

/// Read a runtime env var (empty strings count as unset).
pub fn read_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

// --- data-layer values (package-name-free, stable across renames) ------------

/// Format tag stored inside the cloud `manifest.json`. Package-name-free;
/// reads accept the current format only.
pub const MANIFEST_FORMAT: &str = "webdav-sync";

/// Codex model catalog file written next to `config.toml`. No product-name
/// prefix: the file already lives inside the app's own config dir.
pub const CODEX_CATALOG_FILE: &str = "model-catalog.json";

/// Directory (under the Codex home) where Codex keeps its local app-server
/// daemon state. apmux reads the pid file to tell "a daemon has run here"
/// from "this machine never used one".
pub const CODEX_DAEMON_DIR: &str = "app-server-daemon";

/// Daemon identity file inside [`CODEX_DAEMON_DIR`].
pub const CODEX_DAEMON_PID_FILE: &str = "daemon.pid";

/// Fallback slot key when a provider has no usable display name.
pub const DEFAULT_SLOT_KEY: &str = "managed";
