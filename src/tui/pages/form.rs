use anyhow::{bail, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::Span;

use crate::adapter::{self, FieldKind, FieldSpec, FieldStorage};
use crate::i18n::{t, tf};
use crate::store::{AppId, Provider};
use crate::switch::{AddOpts, EditOpts};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormKind {
    Add { app: AppId },
    Edit { app: AppId },
    SyncSetup,
    GistSetup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormCmd {
    Continue,
    Cancel,
    Submit,
    FetchModels,
    OpenSnippet,
    OpenModels,
}

#[derive(Debug, Clone)]
pub struct InputField {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    pub storage: Option<FieldStorage>,
    pub value: String,
    pub cursor: usize,
    pub secret_keep: bool,
    pub saved_secret: Option<String>,
    pub readonly: bool,
}

impl InputField {
    fn finish(mut self) -> Self {
        self.cursor = self.value.chars().count();
        self
    }

    /// A secret field whose value is already stored means "keep it" while it
    /// is untouched (the `(keep current)` placeholder) *and* after the user
    /// clears it: the TUI has no way to author an empty key, so an emptied
    /// field keeps the stored value on confirm instead of wiping it.
    fn keeps_stored_secret(&self) -> bool {
        matches!(self.kind, FieldKind::Secret)
            && (self.secret_keep || (self.saved_secret.is_some() && self.value.trim().is_empty()))
    }
}

/// Drop the `(keep current)` placeholder so the field is an ordinary empty
/// input again. Every edit gesture on a kept secret starts from blank; the
/// stored value is never spliced into whatever gets typed next.
fn release_kept_secret(f: &mut InputField) {
    if matches!(f.kind, FieldKind::Secret) && f.secret_keep {
        f.secret_keep = false;
        f.value.clear();
        f.cursor = 0;
    }
}

#[derive(Debug, Clone)]
pub struct Form {
    pub kind: FormKind,
    pub fields: Vec<InputField>,
    pub focus: usize,
    pub error: Option<String>,
    pub edit_id: Option<String>,
    pub catalog: Vec<crate::store::ModelEntry>,
    pub slots: std::collections::BTreeMap<String, String>,
    pub snippet: Option<serde_json::Value>,
    pub apply_snippet: bool,
    pub quick_extras: std::collections::BTreeMap<String, String>,
    /// Setup forms only: a backend config already exists on disk (the token
    /// / password field then shows "keep current" when left empty).
    pub has_config: bool,
}

pub fn for_add(app: AppId) -> Result<Form> {
    let adapter = adapter::get(app)?;
    let mut fields: Vec<_> = adapter
        .fields()
        .iter()
        .map(|s| field_from_spec(s, None))
        .collect();
    fields.push(models_open_field(app, &[], &Default::default(), None));
    fields.push(snippet_open_field(app, None, &Default::default()));
    fields.push(apply_snippet_field(false));
    let mut form = Form {
        kind: FormKind::Add { app },
        fields,
        focus: 0,
        error: None,
        edit_id: None,
        catalog: Vec::new(),
        slots: Default::default(),
        snippet: None,
        apply_snippet: false,
        quick_extras: Default::default(),
        has_config: false,
    };
    form.refresh_meta_summaries();
    Ok(form)
}

pub fn for_edit(provider: &Provider) -> Result<Form> {
    let adapter = adapter::get(provider.app)?;
    let mut fields: Vec<_> = adapter
        .fields()
        .iter()
        .map(|s| field_from_spec(s, Some(provider)))
        .collect();
    fields.push(models_open_field(
        provider.app,
        &provider.catalog,
        &provider.slots,
        provider.model.as_deref(),
    ));
    fields.push(snippet_open_field(
        provider.app,
        Some(provider),
        &provider.extras,
    ));
    fields.push(apply_snippet_field(provider.apply_snippet));
    let mut form = Form {
        kind: FormKind::Edit { app: provider.app },
        fields,
        focus: 0,
        error: None,
        edit_id: Some(provider.id.clone()),
        catalog: provider.catalog.clone(),
        slots: provider.slots.clone(),
        snippet: provider.snippet.clone(),
        apply_snippet: provider.apply_snippet,
        has_config: false,
        quick_extras: provider
            .extras
            .iter()
            .filter(|(k, _)| {
                adapter
                    .quick_items()
                    .iter()
                    .any(|q| q.extra_key == Some(k.as_str()))
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    };
    form.refresh_meta_summaries();
    Ok(form)
}

pub fn for_sync_setup(existing: Option<(String, String, String)>) -> Form {
    let (url, user, pass) = existing.unwrap_or_default();
    let keep = !pass.is_empty();
    Form {
        kind: FormKind::SyncSetup,
        fields: vec![
            InputField {
                key: "url",
                label: "URL",
                kind: FieldKind::Url,
                required: true,
                storage: None,
                value: url,
                cursor: 0,
                secret_keep: false,
                saved_secret: None,
                readonly: false,
            }
            .finish(),
            InputField {
                key: "namespace",
                label: "ui.namespace",
                kind: FieldKind::Text,
                required: false,
                storage: None,
                value: crate::webdav::NAMESPACE.to_string(),
                cursor: 0,
                secret_keep: false,
                saved_secret: None,
                readonly: true,
            }
            .finish(),
            InputField {
                key: "username",
                label: "ui.username",
                required: true,
                kind: FieldKind::Text,
                storage: None,
                value: user,
                cursor: 0,
                secret_keep: false,
                saved_secret: None,
                readonly: false,
            }
            .finish(),
            InputField {
                key: "password",
                label: "ui.password",
                kind: FieldKind::Secret,
                required: true,
                storage: None,
                value: String::new(),
                cursor: 0,
                secret_keep: keep,
                saved_secret: if keep { Some(pass) } else { None },
                readonly: false,
            }
            .finish(),
        ],
        focus: 0,
        error: None,
        edit_id: None,
        catalog: Vec::new(),
        slots: Default::default(),
        snippet: None,
        apply_snippet: false,
        quick_extras: Default::default(),
        has_config: keep,
    }
}

/// Gist setup: one token field plus an optional pinned gist id. When a
/// token is already stored, leaving the token empty keeps it.
pub fn for_gist_setup(token_stored: bool, has_config: bool) -> Form {
    Form {
        kind: FormKind::GistSetup,
        fields: vec![
            InputField {
                key: "token",
                label: "ui.token",
                kind: FieldKind::Secret,
                required: !token_stored,
                storage: None,
                value: String::new(),
                cursor: 0,
                secret_keep: token_stored,
                saved_secret: None,
                readonly: false,
            }
            .finish(),
            InputField {
                key: "gist",
                label: "ui.gist_id",
                kind: FieldKind::Text,
                required: false,
                storage: None,
                value: String::new(),
                cursor: 0,
                secret_keep: false,
                saved_secret: None,
                readonly: false,
            }
            .finish(),
        ],
        focus: 0,
        error: None,
        edit_id: None,
        catalog: Vec::new(),
        slots: Default::default(),
        snippet: None,
        apply_snippet: false,
        quick_extras: Default::default(),
        has_config,
    }
}

fn models_open_field(
    app: AppId,
    catalog: &[crate::store::ModelEntry],
    slots: &std::collections::BTreeMap<String, String>,
    model: Option<&str>,
) -> InputField {
    let (label, value) = models_summary(app, catalog, slots, model);
    InputField {
        key: "models",
        label,
        kind: FieldKind::Text,
        required: false,
        storage: None,
        value,
        cursor: 0,
        secret_keep: false,
        saved_secret: None,
        readonly: false,
    }
}

fn models_summary(
    app: AppId,
    catalog: &[crate::store::ModelEntry],
    slots: &std::collections::BTreeMap<String, String>,
    model: Option<&str>,
) -> (&'static str, String) {
    match crate::adapter::get(app).map(|a| a.model_ui()).unwrap_or(
        crate::adapter::models::ModelUi::Catalog {
            fields: crate::adapter::models::OPENCODE_FIELDS,
        },
    ) {
        crate::adapter::models::ModelUi::Catalog { .. } => {
            let value = if catalog.is_empty() {
                t("field.models_empty").to_string()
            } else {
                catalog.len().to_string()
            };
            ("field.catalog", value)
        }
        crate::adapter::models::ModelUi::Slots { slots: specs } => {
            let filled = specs
                .iter()
                .filter(|s| slots.get(s.key).is_some_and(|v| !v.is_empty()))
                .count()
                + usize::from(model.is_some_and(|m| !m.is_empty()));
            ("field.slots", format!("{filled}/{}", specs.len() + 1))
        }
    }
}

fn snippet_open_field(
    app: AppId,
    _provider: Option<&Provider>,
    extras: &std::collections::BTreeMap<String, String>,
) -> InputField {
    let items = adapter::get(app).map(|a| a.quick_items()).unwrap_or(&[]);
    let on = items.iter().filter(|i| i.extra_on(extras)).count();
    let value = if items.is_empty() {
        t("quick.edit_json").to_string()
    } else {
        format!("{on}/{}", items.len())
    };
    InputField {
        key: "snippet",
        label: "field.snippet",
        kind: FieldKind::Text,
        required: false,
        storage: None,
        value,
        cursor: 0,
        secret_keep: false,
        saved_secret: None,
        readonly: false,
    }
}

fn apply_snippet_field(on: bool) -> InputField {
    InputField {
        key: "apply_snippet",
        label: "field.apply_snippet",
        kind: FieldKind::Select(&["no", "yes"]),
        required: false,
        storage: None,
        value: if on { "yes" } else { "no" }.to_string(),
        cursor: 0,
        secret_keep: false,
        saved_secret: None,
        readonly: false,
    }
    .finish()
}

fn field_from_spec(spec: &FieldSpec, existing: Option<&Provider>) -> InputField {
    let (value, secret_keep, saved_secret) = match (existing, spec.storage, spec.kind) {
        (Some(p), FieldStorage::Name, _) => (p.name.clone(), false, None),
        (Some(p), FieldStorage::BaseUrl, _) => (p.base_url.clone(), false, None),
        (Some(p), FieldStorage::ApiKey, _) => (String::new(), true, Some(p.api_key.clone())),
        (Some(p), FieldStorage::Model, _) => (p.model.clone().unwrap_or_default(), false, None),
        (Some(p), FieldStorage::Extra(k), _) => {
            let value = if k == "protocol" {
                crate::adapter::protocol::from_extras(&p.extras)
                    .unwrap_or(crate::adapter::protocol::DEFAULT)
                    .to_string()
            } else {
                p.extras
                    .get(k)
                    .cloned()
                    .or_else(|| spec.default.map(str::to_string))
                    .unwrap_or_default()
            };
            (value, false, None)
        }
        (None, _, FieldKind::Secret) => (String::new(), false, None),
        (None, _, _) => (spec.default.unwrap_or("").to_string(), false, None),
    };
    InputField {
        key: spec.key,
        label: spec.label,
        kind: spec.kind,
        required: spec.required,
        storage: Some(spec.storage),
        value,
        cursor: 0,
        secret_keep,
        saved_secret,
        readonly: false,
    }
    .finish()
}

impl Form {
    pub fn title(&self) -> &'static str {
        match self.kind {
            FormKind::Add { .. } => t("ui.add_provider"),
            FormKind::Edit { .. } => t("ui.edit_provider"),
            FormKind::SyncSetup => t("ui.sync_setup"),
            FormKind::GistSetup => t("ui.gist_setup"),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> FormCmd {
        match key.code {
            KeyCode::Esc => FormCmd::Cancel,
            // Enter always submits the form; opening a sub-editor
            // (Catalog/Slots/Snippet) or fetching models is Space's job.
            KeyCode::Enter => FormCmd::Submit,
            KeyCode::Tab | KeyCode::Down => {
                self.shift(1);
                FormCmd::Continue
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.shift(-1);
                FormCmd::Continue
            }
            KeyCode::Left if self.can_edit_text() => {
                crate::tui::edit::left(&mut self.fields[self.focus].cursor);
                FormCmd::Continue
            }
            KeyCode::Right if self.can_edit_text() => {
                let f = &mut self.fields[self.focus];
                crate::tui::edit::right(&f.value, &mut f.cursor);
                FormCmd::Continue
            }
            KeyCode::Home if self.can_edit_text() => {
                crate::tui::edit::home(&mut self.fields[self.focus].cursor);
                FormCmd::Continue
            }
            KeyCode::End if self.can_edit_text() => {
                let f = &mut self.fields[self.focus];
                crate::tui::edit::end(&f.value, &mut f.cursor);
                FormCmd::Continue
            }
            KeyCode::Delete if self.can_edit_text() => {
                self.delete_char();
                FormCmd::Continue
            }
            KeyCode::Char(' ')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.fields.get(self.focus).map(|f| f.key) == Some("snippet") =>
            {
                FormCmd::OpenSnippet
            }
            KeyCode::Char(' ')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.fields.get(self.focus).map(|f| f.key) == Some("models") =>
            {
                FormCmd::OpenModels
            }
            KeyCode::Char(' ')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(
                        self.fields.get(self.focus).map(|f| f.kind),
                        Some(FieldKind::Model)
                    ) =>
            {
                FormCmd::FetchModels
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) && c != '?' => {
                self.type_char(c);
                FormCmd::Continue
            }
            KeyCode::Backspace => {
                self.backspace();
                FormCmd::Continue
            }
            _ => FormCmd::Continue,
        }
    }

    /// Insert a bracketed-paste payload into the focused editable field.
    pub fn paste(&mut self, text: &str) {
        if self.can_edit_text() {
            let f = &mut self.fields[self.focus];
            release_kept_secret(f);
            crate::tui::edit::paste(&mut f.value, &mut f.cursor, text);
        }
    }

    fn shift(&mut self, delta: isize) {
        let n = self.fields.len() as isize;
        if n == 0 {
            return;
        }
        for _ in 0..self.fields.len() {
            self.focus = (self.focus as isize + delta).rem_euclid(n) as usize;
            if !self.fields[self.focus].readonly {
                self.snap_cursor();
                return;
            }
        }
    }

    fn snap_cursor(&mut self) {
        if let Some(f) = self.fields.get_mut(self.focus) {
            f.cursor = f.value.chars().count();
        }
    }

    fn can_edit_text(&self) -> bool {
        let Some(f) = self.fields.get(self.focus) else {
            return false;
        };
        if f.readonly || matches!(f.key, "snippet" | "models") {
            return false;
        }
        if matches!(f.kind, FieldKind::Select(_)) {
            return false;
        }
        true
    }

    fn type_char(&mut self, c: char) {
        self.error = None;
        if self.fields.is_empty() {
            return;
        }
        if self.fields[self.focus].readonly {
            return;
        }
        if matches!(self.fields[self.focus].key, "snippet" | "models") {
            return;
        }
        if matches!(self.fields[self.focus].kind, FieldKind::Select(_)) {
            if c == ' ' {
                self.cycle_select();
            }
            return;
        }
        let f = &mut self.fields[self.focus];
        release_kept_secret(f);
        crate::tui::edit::insert(&mut f.value, &mut f.cursor, c);
    }

    fn cycle_select(&mut self) {
        let i = self.focus;
        let FieldKind::Select(opts) = self.fields[i].kind else {
            return;
        };
        if opts.is_empty() {
            return;
        }
        let cur = opts
            .iter()
            .position(|o| *o == self.fields[i].value)
            .unwrap_or(0);
        let next = (cur + 1) % opts.len();
        self.fields[i].value = opts[next].to_string();
    }

    fn backspace(&mut self) {
        self.error = None;
        if self.fields.is_empty() {
            return;
        }
        if self.fields[self.focus].readonly {
            return;
        }
        if matches!(self.fields[self.focus].key, "snippet" | "models") {
            return;
        }
        let f = &mut self.fields[self.focus];
        if matches!(f.kind, FieldKind::Select(_)) {
            return;
        }
        // Backspace on a kept secret drops the placeholder and leaves an
        // empty field that can be typed into; it does not flip straight back
        // to "keep current", which would make clearing a dead end. An empty
        // field still keeps the stored value on confirm (`secret_value`).
        release_kept_secret(f);
        crate::tui::edit::backspace(&mut f.value, &mut f.cursor);
    }

    fn delete_char(&mut self) {
        self.error = None;
        if !self.can_edit_text() {
            return;
        }
        let f = &mut self.fields[self.focus];
        release_kept_secret(f);
        crate::tui::edit::delete(&mut f.value, &mut f.cursor);
    }

    pub fn add_opts(&self) -> Result<AddOpts> {
        let FormKind::Add { app } = self.kind else {
            bail!("not an add form");
        };
        self.check_required()?;
        Ok(AddOpts {
            app,
            name: self.stored(FieldStorage::Name),
            base_url: self.stored(FieldStorage::BaseUrl),
            api_key: self.secret_value(FieldStorage::ApiKey)?,
            model: self.model(),
            extra: self.extras(),
            catalog: self.catalog.clone(),
            slots: self.slots.clone(),
            snippet: self.snippet.clone(),
            apply_snippet: self.snippet_flag(),
        })
    }

    pub fn edit_opts(&self) -> Result<EditOpts> {
        let FormKind::Edit { app } = self.kind else {
            bail!("not an edit form");
        };
        self.check_required()?;
        let id = self
            .edit_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing id"))?;
        let api_key = self.fields.iter().find_map(|f| {
            if f.storage != Some(FieldStorage::ApiKey) {
                return None;
            }
            if f.keeps_stored_secret() {
                // `None` = keep the stored key: untouched, or cleared by
                // hand. apmux never writes an empty API key by accident.
                None
            } else {
                Some(f.value.trim().to_string())
            }
        });
        Ok(EditOpts {
            query: id,
            app: Some(app),
            name: Some(self.stored(FieldStorage::Name)),
            base_url: Some(self.stored(FieldStorage::BaseUrl)),
            api_key,
            // empty string → None in edit_provider; never treat empty Model as keep
            model: Some(self.stored(FieldStorage::Model)),
            clear_model: false,
            extra: self.extras(),
            catalog: Some(self.catalog.clone()),
            slots: Some(self.slots.clone()),
            snippet: Some(
                self.snippet
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({})),
            ),
            apply_snippet: Some(self.snippet_flag()),
        })
    }

    pub fn sync_setup(&self) -> Result<(String, String, String)> {
        let url = field_by_key(&self.fields, "url").trim().to_string();
        let username = field_by_key(&self.fields, "username").trim().to_string();
        let pass_f = self
            .fields
            .iter()
            .find(|f| f.key == "password")
            .ok_or_else(|| anyhow::anyhow!("missing password"))?;
        let password = if pass_f.keeps_stored_secret() {
            pass_f.saved_secret.clone().unwrap_or_default()
        } else {
            pass_f.value.clone()
        };
        if url.is_empty() {
            bail!("{}", t("form.url_empty"));
        }
        if username.is_empty() {
            bail!("{}", t("form.user_empty"));
        }
        if password.is_empty() {
            bail!("{}", t("form.pass_empty"));
        }
        Ok((url, username, password))
    }

    /// Gist setup submit: `(token, pinned gist id)`. An empty token means
    /// "keep the stored one" — the caller passes `None` and setup_with
    /// falls back to the saved credential.
    pub fn gist_setup(&self) -> Result<(Option<String>, Option<String>)> {
        let token_f = self
            .fields
            .iter()
            .find(|f| f.key == "token")
            .ok_or_else(|| anyhow::anyhow!("missing token"))?;
        let token = if token_f.keeps_stored_secret() {
            // Kept secret: none typed; setup_with reuses the stored token.
            None
        } else {
            let v = token_f.value.trim().to_string();
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        };
        let gist = field_by_key(&self.fields, "gist").trim().to_string();
        let gist = if gist.is_empty() { None } else { Some(gist) };
        if token.is_none() && !self.has_config {
            bail!("{}", t("form.token_empty"));
        }
        Ok((token, gist))
    }

    fn check_required(&self) -> Result<()> {
        for f in &self.fields {
            if f.storage.is_none() {
                continue;
            }
            if f.keeps_stored_secret() {
                continue;
            }
            let v = f.value.trim();
            if f.required && v.is_empty() {
                bail!("{}", tf("form.required", &[t(f.label)]));
            }
            if matches!(f.kind, FieldKind::Url) && f.storage == Some(FieldStorage::BaseUrl) {
                adapter::require_http_url(v)?;
            }
            if let FieldKind::Select(opts) = f.kind {
                if !v.is_empty() && !opts.contains(&v) {
                    bail!("{}", tf("form.invalid", &[t(f.label)]));
                }
            }
        }
        Ok(())
    }

    fn stored(&self, storage: FieldStorage) -> String {
        self.fields
            .iter()
            .find(|f| f.storage == Some(storage))
            .map(|f| f.value.trim().to_string())
            .unwrap_or_default()
    }

    fn secret_value(&self, storage: FieldStorage) -> Result<String> {
        let f = self
            .fields
            .iter()
            .find(|f| f.storage == Some(storage))
            .ok_or_else(|| anyhow::anyhow!("missing field"))?;
        if f.keeps_stored_secret() {
            f.saved_secret
                .clone()
                .ok_or_else(|| anyhow::anyhow!("{}", tf("form.required", &[t(f.label)])))
        } else {
            Ok(f.value.trim().to_string())
        }
    }

    fn snippet_flag(&self) -> bool {
        self.fields
            .iter()
            .find(|f| f.key == "apply_snippet")
            .map(|f| f.value == "yes")
            .unwrap_or(self.apply_snippet)
    }

    fn model(&self) -> Option<String> {
        let v = self.stored(FieldStorage::Model);
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    }

    pub fn refresh_meta_summaries(&mut self) {
        self.refresh_models_summary();
        self.refresh_quick_summary();
    }

    fn refresh_models_summary(&mut self) {
        let app = match self.kind {
            FormKind::Add { app } | FormKind::Edit { app } => app,
            FormKind::SyncSetup | FormKind::GistSetup => return,
        };
        let model = self.model();
        let (label, value) = models_summary(app, &self.catalog, &self.slots, model.as_deref());
        if let Some(f) = self.fields.iter_mut().find(|f| f.key == "models") {
            f.label = label;
            f.value = value;
        }
    }

    pub fn refresh_quick_summary(&mut self) {
        let app = match self.kind {
            FormKind::Add { app } | FormKind::Edit { app } => app,
            FormKind::SyncSetup | FormKind::GistSetup => return,
        };
        let items = adapter::get(app).map(|a| a.quick_items()).unwrap_or(&[]);
        let Some(f) = self.fields.iter_mut().find(|f| f.key == "snippet") else {
            return;
        };
        if items.is_empty() {
            f.value = t("quick.edit_json").to_string();
            return;
        }
        let snippet = self.snippet.as_ref();
        let extras = self.quick_extras.clone();
        let on = items
            .iter()
            .filter(|i| {
                if i.extra_key.is_some() {
                    i.extra_on(&extras)
                } else {
                    snippet.is_some_and(|s| i.snippet_on(s))
                }
            })
            .count();
        f.value = format!("{on}/{}", items.len());
    }

    pub fn fetch_creds(&self) -> Option<(String, String, Option<String>)> {
        let app = match self.kind {
            FormKind::Add { app } | FormKind::Edit { app } => app,
            FormKind::SyncSetup | FormKind::GistSetup => return None,
        };
        let url = self
            .fields
            .iter()
            .find(|f| f.storage == Some(FieldStorage::BaseUrl))
            .map(|f| f.value.trim().to_string())
            .unwrap_or_default();
        let key = self
            .fields
            .iter()
            .find(|f| f.storage == Some(FieldStorage::ApiKey))
            .map(|f| {
                if f.keeps_stored_secret() {
                    f.saved_secret.clone().unwrap_or_default()
                } else {
                    f.value.clone()
                }
            })
            .unwrap_or_default();
        let protocol = match app {
            AppId::Claude => Some("anthropic".into()),
            _ => self
                .fields
                .iter()
                .find(|f| f.key == "protocol")
                .map(|f| f.value.clone())
                .filter(|s| !s.is_empty()),
        };
        Some((url, key, protocol))
    }

    fn extras(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .fields
            .iter()
            .filter_map(|f| {
                match f.storage {
                    Some(FieldStorage::Extra(_)) => {}
                    _ => return None,
                }
                let v = f.value.trim();
                if v.is_empty() {
                    None
                } else {
                    Some(format!("{}={v}", f.key))
                }
            })
            .collect();
        let app = match self.kind {
            FormKind::Add { app } | FormKind::Edit { app } => Some(app),
            FormKind::SyncSetup | FormKind::GistSetup => None,
        };
        if let Some(app) = app {
            if let Ok(adapter) = adapter::get(app) {
                for item in adapter.quick_items() {
                    if let Some(k) = item.extra_key {
                        let on = item.extra_on(&self.quick_extras);
                        let encoded = if on { "true" } else { "false" };
                        out.retain(|e| !e.starts_with(&format!("{k}=")));
                        out.push(format!("{k}={encoded}"));
                    }
                }
            }
        }
        out
    }
}

fn field_by_key(fields: &[InputField], key: &str) -> String {
    fields
        .iter()
        .find(|f| f.key == key)
        .map(|f| f.value.clone())
        .unwrap_or_default()
}

fn masked_value(f: &InputField) -> String {
    if matches!(f.kind, FieldKind::Secret) {
        if f.secret_keep {
            t("ui.keep_previous").to_string()
        } else {
            "*".repeat(f.value.chars().count())
        }
    } else {
        f.value.clone()
    }
}

/// Focused field rendering: free-text fields carry an underline caret that
/// marks a position without occupying a character; cycle fields (yes/no),
/// readonly rows, and kept secrets are not cursor-addressable and render as
/// plain text.
pub fn value_spans(f: &InputField, focused: bool, style: Style) -> Vec<Span<'static>> {
    let raw = masked_value(f);
    let caret = focused
        && !f.readonly
        && !matches!(f.kind, FieldKind::Select(_))
        && !(matches!(f.kind, FieldKind::Secret) && f.secret_keep);
    if caret {
        crate::tui::edit::caret_spans(&raw, f.cursor, style)
    } else {
        vec![Span::styled(raw, style)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_required(form: &mut Form, key: &str) {
        if let Some(f) = form.fields.iter_mut().find(|f| f.key == key) {
            match f.key {
                "name" => f.value = "Packy".into(),
                "base_url" => f.value = "https://api.example.com".into(),
                "api_key" => {
                    f.secret_keep = false;
                    f.value = "sk-test-key-abcd".into();
                }
                "model" => f.value = "gpt-4o".into(),
                _ => {}
            }
        }
    }

    #[test]
    fn add_fields_come_from_adapter() {
        for adapter in adapter::registry() {
            let form = for_add(adapter.id()).unwrap();
            let keys: Vec<_> = form.fields.iter().map(|f| f.key).collect();
            let expect: Vec<_> = adapter.fields().iter().map(|f| f.key).collect();
            assert_eq!(
                &keys[..expect.len()],
                expect.as_slice(),
                "{}",
                adapter.display_name()
            );
            assert!(keys.contains(&"models"));
            assert!(keys.contains(&"snippet"));
            assert_eq!(keys.last().copied(), Some("apply_snippet"));
        }
    }

    #[test]
    fn space_on_model_fetches() {
        let mut form = for_add(AppId::OpenCode).unwrap();
        form.focus = form
            .fields
            .iter()
            .position(|f| f.key == "model")
            .expect("model");
        let cmd = form.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(matches!(cmd, FormCmd::FetchModels));
    }

    #[test]
    fn enter_submits_and_space_opens_models_editor() {
        let mut form = for_add(AppId::Claude).unwrap();
        form.focus = form
            .fields
            .iter()
            .position(|f| f.key == "models")
            .expect("models");
        // Enter always submits the form, even on the read-only summary field.
        let cmd = form.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(cmd, FormCmd::Submit));
        let cmd = form.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(matches!(cmd, FormCmd::OpenModels));
    }

    #[test]
    fn opencode_and_pi_protocol_is_space_select() {
        let mut oc = for_add(AppId::OpenCode).unwrap();
        let idx = oc
            .fields
            .iter()
            .position(|f| f.key == "protocol")
            .expect("protocol field");
        assert!(matches!(oc.fields[idx].kind, FieldKind::Select(_)));
        oc.focus = idx;
        oc.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(oc.fields[idx].value, "openai-completions");
        oc.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(oc.fields[idx].value, "openai-responses");
        oc.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(oc.fields[idx].value, "anthropic");
        oc.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(oc.fields[idx].value, "openai-completions");

        let mut pi = for_add(AppId::Pi).unwrap();
        let idx = pi
            .fields
            .iter()
            .position(|f| f.key == "protocol")
            .expect("protocol field");
        pi.focus = idx;
        pi.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        pi.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        pi.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(pi.fields[idx].value, "google");
        let form = for_add(AppId::Codex).unwrap();
        assert!(!form
            .fields
            .iter()
            .any(|f| f.key == "wire_api" || f.key == "protocol"));
    }

    #[test]
    fn optional_model_empty_is_none() {
        let mut form = for_add(AppId::Claude).unwrap();
        fill_required(&mut form, "name");
        fill_required(&mut form, "base_url");
        fill_required(&mut form, "api_key");
        let model = form.fields.iter().find(|f| f.key == "model").unwrap();
        assert!(!model.required);
        assert!(model.value.is_empty());
        let opts = form.add_opts().unwrap();
        assert_eq!(opts.model, None);
    }

    #[test]
    fn required_model_empty_cannot_submit() {
        let mut form = for_add(AppId::Pi).unwrap();
        fill_required(&mut form, "name");
        fill_required(&mut form, "base_url");
        fill_required(&mut form, "api_key");
        let model = form.fields.iter().find(|f| f.key == "model").unwrap();
        assert!(model.required);
        assert!(model.value.is_empty());
        let err = match form.add_opts() {
            Err(e) => e.to_string(),
            Ok(_) => panic!("required model must block submit"),
        };
        assert!(err.contains("must not be empty"), "{err}");
        fill_required(&mut form, "model");
        assert!(form.add_opts().is_ok());
    }

    #[test]
    fn opencode_required_model_empty_cannot_submit() {
        let mut form = for_add(AppId::OpenCode).unwrap();
        fill_required(&mut form, "name");
        fill_required(&mut form, "base_url");
        fill_required(&mut form, "api_key");
        let err = match form.add_opts() {
            Err(e) => e.to_string(),
            Ok(_) => panic!("required model must block submit"),
        };
        assert!(err.contains("must not be empty"), "{err}");
    }

    #[test]
    fn edit_secret_keep_omits_api_key() {
        let p = Provider {
            id: "packy".into(),
            name: "Packy".into(),
            app: AppId::Claude,
            base_url: "https://api.example.com".into(),
            api_key: "sk-old-secret-key".into(),
            model: Some("sonnet".into()),
            extras: Default::default(),
            ..Provider::blank(AppId::Claude)
        };
        let form = for_edit(&p).unwrap();
        let key = form.fields.iter().find(|f| f.key == "api_key").unwrap();
        assert!(key.secret_keep);
        assert!(key.value.is_empty());
        let opts = form.edit_opts().unwrap();
        assert!(opts.api_key.is_none());
        assert_eq!(opts.model.as_deref(), Some("sonnet"));
    }

    /// An edit form whose API key is stored but untouched: the field shows
    /// the `(keep current)` placeholder with the caret parked at 0.
    fn edit_form_with_stored_key() -> Form {
        let p = Provider {
            id: "packy".into(),
            name: "Packy".into(),
            app: AppId::Claude,
            base_url: "https://api.example.com".into(),
            api_key: "sk-old-secret-key".into(),
            model: Some("sonnet".into()),
            extras: Default::default(),
            ..Provider::blank(AppId::Claude)
        };
        let mut form = for_edit(&p).unwrap();
        form.focus = form
            .fields
            .iter()
            .position(|f| f.key == "api_key")
            .expect("api_key");
        form
    }

    fn press(form: &mut Form, code: KeyCode) {
        form.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn backspace_releases_a_kept_secret_instead_of_snapping_back() {
        let mut form = edit_form_with_stored_key();
        assert!(form.fields[form.focus].secret_keep);
        press(&mut form, KeyCode::Backspace);
        // Emptied, not re-placeholdered: the field stays somewhere the user
        // can type into…
        assert!(!form.fields[form.focus].secret_keep);
        assert!(form.fields[form.focus].value.is_empty());
        // …and confirming it empty keeps the stored key.
        assert!(form.edit_opts().unwrap().api_key.is_none());
    }

    #[test]
    fn typing_after_clearing_replaces_the_stored_key() {
        let mut form = edit_form_with_stored_key();
        press(&mut form, KeyCode::Backspace);
        for c in "sk-new-key".chars() {
            press(&mut form, KeyCode::Char(c));
        }
        assert_eq!(
            form.edit_opts().unwrap().api_key.as_deref(),
            Some("sk-new-key")
        );
    }

    #[test]
    fn paste_replaces_a_kept_secret() {
        let mut form = edit_form_with_stored_key();
        form.paste("sk-pasted-key");
        assert!(!form.fields[form.focus].secret_keep);
        assert_eq!(
            form.edit_opts().unwrap().api_key.as_deref(),
            Some("sk-pasted-key")
        );
    }

    #[test]
    fn cleared_sync_password_keeps_the_stored_one() {
        let mut form = for_sync_setup(Some((
            "https://dav.example.com/".into(),
            "user".into(),
            "stored-pass".into(),
        )));
        form.focus = form
            .fields
            .iter()
            .position(|f| f.key == "password")
            .unwrap();
        press(&mut form, KeyCode::Backspace);
        assert!(!form.fields[form.focus].secret_keep);
        assert_eq!(form.sync_setup().unwrap().2, "stored-pass");
    }

    #[test]
    fn edit_cleared_model_is_empty_string_not_keep() {
        let p = Provider {
            id: "packy".into(),
            name: "Packy".into(),
            app: AppId::Claude,
            base_url: "https://api.example.com".into(),
            api_key: "sk-old-secret-key".into(),
            model: Some("sonnet".into()),
            extras: Default::default(),
            ..Provider::blank(AppId::Claude)
        };
        let mut form = for_edit(&p).unwrap();
        let model = form.fields.iter_mut().find(|f| f.key == "model").unwrap();
        model.value.clear();
        let opts = form.edit_opts().unwrap();
        assert_eq!(opts.model.as_deref(), Some(""));
        assert!(!opts.clear_model);
    }

    #[test]
    fn namespace_is_readonly_and_skipped() {
        let mut form = for_sync_setup(None);
        form.fields
            .iter_mut()
            .find(|f| f.key == "url")
            .unwrap()
            .value = "https://webdav.example.com/".into();
        let ns = form.fields.iter().find(|f| f.key == "namespace").unwrap();
        assert_eq!(ns.value, crate::webdav::NAMESPACE);
        assert!(ns.readonly);
        form.fields
            .iter_mut()
            .find(|f| f.key == "username")
            .unwrap()
            .value = "user".into();
        let pass = form
            .fields
            .iter_mut()
            .find(|f| f.key == "password")
            .unwrap();
        pass.secret_keep = false;
        pass.value = "pass".into();
        let (url, user, pass) = form.sync_setup().unwrap();
        assert_eq!(url, "https://webdav.example.com/");
        assert_eq!(user, "user");
        assert_eq!(pass, "pass");
        let ns_idx = form
            .fields
            .iter()
            .position(|f| f.key == "namespace")
            .unwrap();
        form.focus = ns_idx;
        form.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(form.fields[ns_idx].value, crate::webdav::NAMESPACE);
        form.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_ne!(form.focus, ns_idx);
    }

    #[test]
    fn display_masks_secret_and_keep_placeholder() {
        let mut f = InputField {
            key: "api_key",
            label: "API Key",
            kind: FieldKind::Secret,
            required: true,
            storage: Some(FieldStorage::ApiKey),
            value: "sk-secret".into(),
            cursor: 0,
            secret_keep: false,
            saved_secret: None,
            readonly: false,
        };
        assert_eq!(masked_value(&f), "*********");
        assert!(!masked_value(&f).contains("sk-"));
        f.secret_keep = true;
        f.value.clear();
        assert!(masked_value(&f).contains("keep current"));

        // Cycle fields (yes/no) and kept secrets never render a caret.
        let mut sel = InputField {
            key: "apply_snippet",
            label: "Apply snippet",
            kind: FieldKind::Select(&["no", "yes"]),
            required: false,
            storage: None,
            value: "yes".into(),
            cursor: 0,
            secret_keep: false,
            saved_secret: None,
            readonly: false,
        };
        let spans = value_spans(&sel, true, Style::default());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "yes");
        // Free-text fields render an underline caret that adds no character.
        sel.kind = FieldKind::Text;
        sel.cursor = 0;
        let spans = value_spans(&sel, false, Style::default());
        assert_eq!(spans.len(), 1);
        let spans = value_spans(&sel, true, Style::default());
        let shown: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(shown, 3); // "yes" — caret underlines 'y', adds nothing
        sel.cursor = sel.value.chars().count();
        let end_spans = value_spans(&sel, true, Style::default());
        let shown: usize = end_spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(shown, 4); // + one underlined insertion-point space
    }
}
