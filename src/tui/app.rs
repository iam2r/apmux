use std::sync::mpsc::{Receiver, TryRecvError};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::adapter::{self, ApplyOutcome};
use crate::backup;
use crate::cloud;
use crate::gist;
use crate::i18n::{t, tf};
use crate::paths::Paths;
use crate::reload::ReloadOutcome;
use crate::settings::{self, Settings};
use crate::store::{AppId, Provider, Store};
use crate::switch;
use crate::webdav;

use super::keymap::{self, Action, KeyMode};
use super::pages::backups;
use super::pages::form::{self, Form, FormCmd};
use super::pages::models::{
    self, CatalogCmd, CatalogEditor, ModelPicker, PickerCmd, PickerKind, SlotCmd, SlotEditor,
};
use super::pages::quick::{SnippetCmd, SnippetPage};
use super::pages::settings as settings_page;
use super::pages::sync::{self, Job, Outcome};
use crate::try_launch::TryJob;

pub enum Page {
    Providers,
    Data,
    Settings,
}

pub enum Overlay {
    None,
    ConfirmDelete {
        id: String,
        name: String,
    },
    ConfirmRestore {
        name: String,
    },
    ConfirmSync {
        kind: SyncKind,
        url: String,
        last_sync_at: String,
    },
    Form(Form),
    Syncing,
    FetchingModels,
    ModelPicker(ModelPicker),
    CatalogEditor {
        editor: CatalogEditor,
    },
    SlotEditor {
        editor: SlotEditor,
    },
    SnippetEditor(SnippetPage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncKind {
    Push,
    Pull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncTab {
    Webdav,
    Gist,
}

pub struct App {
    pub paths: Paths,
    pub store: Store,
    pub page: Page,
    pub overlay: Overlay,
    pub app_idx: usize,
    pub selected: usize,
    pub help: bool,
    pub status: String,
    pub list_state: ListState,
    pub backups: Vec<backup::BackupEntry>,
    pub backup_sel: usize,
    pub backup_state: ListState,
    pub sync_local: Option<cloud::LocalSync>,
    pub gist_local: Option<gist::GistLocal>,
    /// Which sync backend the Data page's Sync panel shows; the e/p/u keys
    /// act on whichever tab is active.
    pub sync_tab: SyncTab,
    pub settings: Settings,
    pub settings_sel: usize,
    pub settings_state: ListState,
    sync_rx: Option<Receiver<Outcome>>,
    speed_rx: Option<Receiver<Result<crate::speedtest::SpeedResult, String>>>,
    speed_name: Option<String>,
    pending_try: Option<TryJob>,
    setup_form: Option<Form>,
    held_form: Option<Form>,
    fetch_rx: Option<Receiver<Result<Vec<String>, String>>>,
    fetch_kind: Option<PickerKind>,
    held_slots: Option<SlotEditor>,
}

impl App {
    pub fn new(paths: Paths, store: Store) -> Self {
        let settings = Settings::load(&paths).unwrap_or_default();
        let gist_local = gist::local_gist(&paths);
        let mut app = Self {
            paths,
            store,
            page: Page::Providers,
            overlay: Overlay::None,
            app_idx: 0,
            selected: 0,
            help: false,
            status: String::new(),
            list_state: ListState::default(),
            backups: Vec::new(),
            backup_sel: 0,
            backup_state: ListState::default(),
            sync_local: None,
            gist_local,
            sync_tab: SyncTab::Webdav,
            settings,
            settings_sel: 0,
            settings_state: ListState::default(),
            sync_rx: None,
            speed_rx: None,
            speed_name: None,
            pending_try: None,
            setup_form: None,
            held_form: None,
            fetch_rx: None,
            fetch_kind: None,
            held_slots: None,
        };
        app.clamp_app_idx();
        app.focus_current();
        app
    }

    pub fn detected_apps(&self) -> Vec<AppId> {
        settings::detected_apps()
    }

    pub fn visible_apps(&self) -> Vec<AppId> {
        settings::visible_apps(&self.settings, &self.detected_apps())
    }

    pub fn app_count(&self) -> usize {
        self.visible_apps().len().max(1)
    }

    pub fn current_app(&self) -> AppId {
        let vis = self.visible_apps();
        vis.get(self.app_idx)
            .copied()
            .or_else(|| vis.first().copied())
            .unwrap_or(AppId::Claude)
    }

    pub fn tab_titles(&self) -> Vec<String> {
        self.visible_apps()
            .into_iter()
            .map(|id| {
                adapter::get(id)
                    .map(|a| a.display_name())
                    .unwrap_or("?")
                    .to_string()
            })
            .collect()
    }

    fn clamp_app_idx(&mut self) {
        let n = self.visible_apps().len();
        if n == 0 {
            self.app_idx = 0;
        } else if self.app_idx >= n {
            self.app_idx = n - 1;
        }
    }

    fn persist_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.paths) {
            self.status = format!("{e:#}");
        }
    }

    pub fn providers(&self) -> Vec<&Provider> {
        let app = self.current_app();
        self.store
            .providers
            .values()
            .filter(|p| p.app == app)
            .collect()
    }

    pub fn current_id(&self) -> Option<&str> {
        self.store
            .current
            .get(&self.current_app())
            .map(String::as_str)
    }

    pub fn is_syncing(&self) -> bool {
        self.sync_rx.is_some()
            || self.fetch_rx.is_some()
            || matches!(self.overlay, Overlay::Syncing | Overlay::FetchingModels)
    }

    /// Key hints for the current page/overlay. Recomputed every frame; not stored.
    pub fn hint(&self) -> String {
        if self.help {
            return t("status.hint_help").to_string();
        }
        match &self.overlay {
            Overlay::Form(_) => t("ui.form_hint").into(),
            Overlay::ConfirmDelete { .. }
            | Overlay::ConfirmRestore { .. }
            | Overlay::ConfirmSync { .. } => t("ui.confirm_hint").into(),
            Overlay::Syncing | Overlay::FetchingModels => t("status.hint_syncing").into(),
            Overlay::ModelPicker(_) => t("status.hint_picker").into(),
            Overlay::CatalogEditor { editor }
                if editor.slot_picker.is_some() || editor.target_picker.is_some() =>
            {
                t("status.hint_catalog_popover").into()
            }
            Overlay::CatalogEditor { .. } => t("status.hint_catalog").into(),
            Overlay::SlotEditor { .. } => t("status.hint_slots").into(),
            Overlay::SnippetEditor(_) => t("status.hint_snippet").into(),
            Overlay::None => super::keymap::hint_bar(match self.page {
                Page::Providers => KeyMode::List,
                Page::Data => KeyMode::Data,
                Page::Settings => KeyMode::Settings,
            }),
        }
    }

    fn focus_current(&mut self) {
        let cur = self.current_id().map(str::to_string);
        let ids: Vec<String> = self.providers().iter().map(|p| p.id.clone()).collect();
        self.selected = cur
            .as_deref()
            .and_then(|id| ids.iter().position(|p| p == id))
            .unwrap_or(0);
        self.list_state.select(if ids.is_empty() {
            None
        } else {
            Some(self.selected)
        });
    }

    fn focus_id(&mut self, id: &str) {
        let ids: Vec<String> = self.providers().iter().map(|p| p.id.clone()).collect();
        if let Some(i) = ids.iter().position(|p| p == id) {
            self.selected = i;
            self.list_state.select(Some(i));
        } else {
            self.focus_current();
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            return true;
        }
        if self.is_syncing() {
            return matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'));
        }
        if matches!(key.code, KeyCode::Char('?')) {
            self.help = !self.help;
            return false;
        }
        if self.help {
            match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') => return true,
                KeyCode::Esc => self.help = false,
                _ => {}
            }
            return false;
        }
        if matches!(
            self.overlay,
            Overlay::ConfirmDelete { .. }
                | Overlay::ConfirmRestore { .. }
                | Overlay::ConfirmSync { .. }
        ) {
            self.handle_confirm_key(key);
            return false;
        }
        if matches!(self.overlay, Overlay::Form(_)) {
            return self.handle_form_key(key);
        }
        if !matches!(self.overlay, Overlay::None) {
            self.handle_overlay_key(key);
            return false;
        }
        let mode = match self.page {
            Page::Providers => KeyMode::List,
            Page::Data => KeyMode::Data,
            Page::Settings => KeyMode::Settings,
        };
        self.handle_action(keymap::map_key(key, mode))
    }

    fn handle_form_key(&mut self, key: KeyEvent) -> bool {
        if matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return true;
        }
        let cmd = match &mut self.overlay {
            Overlay::Form(form) => form.handle_key(key),
            _ => return false,
        };
        match cmd {
            FormCmd::Continue => {}
            FormCmd::Cancel => {
                self.overlay = Overlay::None;
            }
            FormCmd::Submit => self.submit_form(),
            FormCmd::FetchModels => self.start_form_fetch(),
            FormCmd::OpenSnippet => self.open_snippet_from_form(),
            FormCmd::OpenModels => self.open_models_from_form(),
        }
        false
    }

    /// Route a bracketed-paste payload to whatever text editor has focus.
    pub fn handle_paste(&mut self, text: &str) {
        match &mut self.overlay {
            Overlay::Form(form) => form.paste(text),
            Overlay::ModelPicker(picker) => picker.paste_filter(text),
            Overlay::CatalogEditor { editor } => editor.paste(text),
            Overlay::SlotEditor { editor } => editor.paste(text),
            Overlay::SnippetEditor(page) => page.paste_json(text),
            _ => {}
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.confirm_yes(),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.overlay = Overlay::None;
            }
            _ => {}
        }
    }

    pub fn handle_action(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => return true,
            Action::ToggleHelp => self.help = !self.help,
            Action::CloseOverlay => self.help = false,
            Action::NextApp => self.shift_app(1),
            Action::PrevApp => self.shift_app(-1),
            Action::Up => self.move_sel(-1),
            Action::Down => self.move_sel(1),
            Action::Switch => {
                if let Err(e) = self.switch_selected() {
                    self.status = tf("status.switch_failed", &[&format!("{e:#}")]);
                }
            }
            Action::Add => self.open_add(),
            Action::Edit => self.open_edit(),
            Action::Delete => self.open_delete(),
            Action::Backup => self.create_backup(),
            Action::OpenData => self.open_data(),
            Action::OpenSettings => self.open_settings(),
            Action::ToggleSetting => self.toggle_setting(),
            Action::Back => {
                self.page = Page::Providers;
            }
            Action::Restore => self.open_restore(),
            Action::SyncTab => self.toggle_sync_tab(),
            Action::SyncPush => self.open_sync_confirm(SyncKind::Push),
            Action::SyncPull => self.open_sync_confirm(SyncKind::Pull),
            Action::SyncSetup => self.open_sync_setup(),
            Action::SpeedTest => self.start_speed_test(),
            Action::TryLaunch => self.queue_try_launch(),
            Action::None => {}
        }
        false
    }

    fn shift_app(&mut self, delta: isize) {
        let n = self.app_count() as isize;
        if n == 0 {
            return;
        }
        self.app_idx = ((self.app_idx as isize + delta).rem_euclid(n)) as usize;
        self.focus_current();
    }

    fn move_sel(&mut self, delta: isize) {
        match self.page {
            Page::Providers => {
                let n = self.providers().len() as isize;
                if n == 0 {
                    return;
                }
                self.selected = ((self.selected as isize + delta).rem_euclid(n)) as usize;
                self.list_state.select(Some(self.selected));
            }
            Page::Data => {
                let n = self.backups.len() as isize;
                if n == 0 {
                    return;
                }
                self.backup_sel = ((self.backup_sel as isize + delta).rem_euclid(n)) as usize;
                self.backup_state.select(Some(self.backup_sel));
            }
            Page::Settings => {
                let n = settings_page::row_count() as isize;
                self.settings_sel = ((self.settings_sel as isize + delta).rem_euclid(n)) as usize;
                self.settings_state.select(Some(self.settings_sel));
            }
        }
    }

    fn open_settings(&mut self) {
        self.page = Page::Settings;
        self.settings_sel = self
            .settings_sel
            .min(settings_page::row_count().saturating_sub(1));
        self.settings_state.select(Some(self.settings_sel));
    }

    fn toggle_setting(&mut self) {
        if !matches!(self.page, Page::Settings) {
            return;
        }
        match self.settings_sel {
            settings_page::ROW_LANG => self.settings.cycle_lang(),
            settings_page::ROW_MODE => {
                let detected = self.detected_apps();
                self.settings.cycle_apps_mode(&detected);
                self.clamp_app_idx();
                self.focus_current();
            }
            i if i >= settings_page::ROW_APPS_START => {
                let apps = settings::all_apps();
                if let Some(app) = apps.get(i - settings_page::ROW_APPS_START).copied() {
                    self.settings.toggle_visible(app);
                    self.clamp_app_idx();
                    self.focus_current();
                }
            }
            _ => {}
        }
        self.persist_settings();
    }

    fn open_snippet_from_form(&mut self) {
        let Overlay::Form(form) = std::mem::replace(&mut self.overlay, Overlay::None) else {
            return;
        };
        let app = match form.kind {
            form::FormKind::Add { app } | form::FormKind::Edit { app } => app,
            form::FormKind::SyncSetup | form::FormKind::GistSetup => {
                self.overlay = Overlay::Form(form);
                return;
            }
        };
        let extras = form.quick_extras.clone();
        let snippet = form.snippet.clone();
        self.held_form = Some(form);
        self.help = false;
        self.overlay = Overlay::SnippetEditor(SnippetPage::open(app, snippet.as_ref(), extras));
    }

    fn open_models_from_form(&mut self) {
        let Overlay::Form(form) = std::mem::replace(&mut self.overlay, Overlay::None) else {
            return;
        };
        let app = match form.kind {
            form::FormKind::Add { app } | form::FormKind::Edit { app } => app,
            form::FormKind::SyncSetup | form::FormKind::GistSetup => {
                self.overlay = Overlay::Form(form);
                return;
            }
        };
        self.help = false;
        match models::model_ui_for(app) {
            crate::adapter::models::ModelUi::Catalog { fields } => {
                let default = form
                    .fields
                    .iter()
                    .find(|f| f.storage == Some(crate::adapter::FieldStorage::Model))
                    .map(|f| f.value.clone())
                    .filter(|s| !s.is_empty());
                let mut tmp = crate::store::Provider::blank(app);
                tmp.model = default.clone();
                tmp.catalog = form.catalog.clone();
                tmp.slots = form.slots.clone();
                let rows = crate::adapter::models::catalog_models(&tmp);
                // For Claude (and any other app with a slot-bearing catalog),
                // `from_provider` carries the slot-to-id bindings through the
                // editor. For OpenCode/Pi/Codex, `slot_owner` stays empty.
                let editor = if matches!(app, AppId::Claude) {
                    CatalogEditor::from_provider(fields, &tmp)
                } else {
                    CatalogEditor::new(fields, rows, default.as_deref())
                };
                self.overlay = Overlay::CatalogEditor { editor };
            }
            crate::adapter::models::ModelUi::Slots { .. } => {
                let default = form
                    .fields
                    .iter()
                    .find(|f| f.storage == Some(crate::adapter::FieldStorage::Model))
                    .map(|f| f.value.clone())
                    .unwrap_or_default();
                self.overlay = Overlay::SlotEditor {
                    editor: SlotEditor::from_values(default, form.slots.clone()),
                };
            }
        }
        self.held_form = Some(form);
    }

    fn handle_snippet_key(&mut self, key: KeyEvent) {
        let cmd = match &mut self.overlay {
            Overlay::SnippetEditor(page) => page.handle_key(key),
            _ => return,
        };
        match cmd {
            SnippetCmd::Continue => {}
            SnippetCmd::Cancel => self.close_snippet_editor(),
            SnippetCmd::Save => self.save_snippet(),
            SnippetCmd::Toggle => self.toggle_snippet_item(),
        }
    }

    fn toggle_snippet_item(&mut self) {
        let Overlay::SnippetEditor(page) = &mut self.overlay else {
            return;
        };
        let Some(item) = page.focused_item() else {
            return;
        };
        page.error = None;
        if let Some(key) = item.extra_key {
            if item.extra_on(&page.extras) {
                page.extras.remove(key);
            } else {
                page.extras.insert(key.to_string(), "true".into());
            }
            return;
        }
        let mut snippet = match page.parsed_snippet() {
            Ok(v) => v,
            Err(e) => {
                page.error = Some(e);
                return;
            }
        };
        if item.snippet_on(&snippet) {
            item.remove_snippet(&mut snippet);
        } else {
            item.apply_snippet(&mut snippet);
        }
        page.set_snippet(&snippet);
    }

    fn close_snippet_editor(&mut self) {
        let Overlay::SnippetEditor(_) = std::mem::replace(&mut self.overlay, Overlay::None) else {
            return;
        };
        self.restore_held_form();
    }

    pub fn switch_selected(&mut self) -> Result<()> {
        let id = match self.providers().get(self.selected) {
            Some(p) => p.id.clone(),
            None => {
                self.status = t("status.no_switch").into();
                return Ok(());
            }
        };
        let app = self.current_app();
        let (switched, report) =
            switch::use_provider_quiet(&self.paths, &mut self.store, &id, Some(app))?;
        self.status = match report.outcome {
            ApplyOutcome::SkippedUninitialized => {
                let name = adapter::get(app).map(|a| a.display_name()).unwrap_or("?");
                tf("status.switched_skip", &[&switched, name])
            }
            ApplyOutcome::Applied { .. } => {
                let note =
                    reload_note(&report.reload).unwrap_or_else(|| t("status.restart_short").into());
                format!("{} \u{b7} {note}", tf("status.switched", &[&switched]))
            }
        };
        self.focus_current();
        Ok(())
    }

    fn open_add(&mut self) {
        match form::for_add(self.current_app()) {
            Ok(form) => {
                self.help = false;
                self.overlay = Overlay::Form(form);
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    fn open_edit(&mut self) {
        let p = {
            let providers = self.providers();
            let Some(p) = providers.get(self.selected) else {
                self.status = t("status.no_edit").into();
                return;
            };
            if p.official {
                let id = p.id.clone();
                self.status = tf("status.official_protected", &[&id]);
                return;
            }
            (*p).clone()
        };
        match form::for_edit(&p) {
            Ok(form) => {
                self.help = false;
                self.overlay = Overlay::Form(form);
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    fn open_delete(&mut self) {
        let (id, name, official) = {
            let providers = self.providers();
            let Some(p) = providers.get(self.selected) else {
                self.status = t("status.no_delete").into();
                return;
            };
            (p.id.clone(), p.name.clone(), p.official)
        };
        if official {
            let id = id.clone();
            self.status = tf("status.official_protected", &[&id]);
            return;
        }
        self.help = false;
        self.overlay = Overlay::ConfirmDelete { id, name };
    }

    fn create_backup(&mut self) {
        match backup::create(&self.paths, None) {
            Ok(stem) => {
                self.status = tf("status.backed_up", &[&stem]);
                if matches!(self.page, Page::Data) {
                    self.refresh_backups();
                }
            }
            Err(e) => self.status = tf("status.backup_failed", &[&format!("{e:#}")]),
        }
    }

    fn open_data(&mut self) {
        self.page = Page::Data;
        self.refresh_backups();
        self.sync_local = cloud::local_sync(&self.paths);
        self.gist_local = gist::local_gist(&self.paths);
    }

    fn refresh_backups(&mut self) {
        self.backups = backups::load(&self.paths);
        if self.backups.is_empty() {
            self.backup_sel = 0;
            self.backup_state.select(None);
        } else {
            self.backup_sel = self.backup_sel.min(self.backups.len() - 1);
            self.backup_state.select(Some(self.backup_sel));
        }
    }

    fn open_restore(&mut self) {
        let Some(entry) = self.backups.get(self.backup_sel) else {
            self.status = t("status.no_restore").into();
            return;
        };
        self.help = false;
        self.overlay = Overlay::ConfirmRestore {
            name: entry.name.clone(),
        };
    }

    fn open_sync_setup(&mut self) {
        self.help = false;
        match self.sync_tab {
            SyncTab::Webdav => {
                self.overlay = Overlay::Form(form::for_sync_setup(cloud::credentials(&self.paths)));
            }
            SyncTab::Gist => {
                let token_stored = self
                    .gist_local
                    .as_ref()
                    .is_some_and(|g| !g.gist_id.is_empty());
                self.overlay = Overlay::Form(form::for_gist_setup(
                    token_stored,
                    self.paths.gist_file().is_file(),
                ));
            }
        }
    }

    fn toggle_sync_tab(&mut self) {
        self.sync_tab = match self.sync_tab {
            SyncTab::Webdav => SyncTab::Gist,
            SyncTab::Gist => SyncTab::Webdav,
        };
    }

    fn open_sync_confirm(&mut self, kind: SyncKind) {
        if self.sync_rx.is_some() || matches!(self.overlay, Overlay::Syncing) {
            return;
        }
        match self.sync_tab {
            SyncTab::Webdav => {
                let Some(local) = cloud::local_sync(&self.paths) else {
                    self.status = t("status.sync_unconfigured").into();
                    return;
                };
                self.help = false;
                self.overlay = Overlay::ConfirmSync {
                    kind,
                    url: webdav::redact_url(&local.url),
                    last_sync_at: local.last_sync_at,
                };
            }
            SyncTab::Gist => {
                let Some(local) = gist::local_gist(&self.paths) else {
                    self.status = t("status.gist_unconfigured").into();
                    return;
                };
                if local.gist_id.is_empty() {
                    self.status = t("status.gist_unconfigured").into();
                    return;
                }
                self.help = false;
                self.overlay = Overlay::ConfirmSync {
                    kind,
                    url: format!("https://gist.github.com/{}", local.gist_id),
                    last_sync_at: local.last_sync_at,
                };
            }
        }
    }

    fn confirm_yes(&mut self) {
        match &self.overlay {
            Overlay::ConfirmDelete { id, .. } => {
                let id = id.clone();
                self.overlay = Overlay::None;
                let app = self.current_app();
                match switch::delete_provider(&self.paths, &mut self.store, &id, Some(app), true) {
                    Ok(deleted) => {
                        self.status = tf("status.deleted", &[&deleted]);
                        self.focus_current();
                    }
                    Err(e) => self.status = tf("status.delete_failed", &[&format!("{e:#}")]),
                }
            }
            Overlay::ConfirmRestore { name } => {
                let name = name.clone();
                self.overlay = Overlay::None;
                match backup::restore_quiet(&self.paths, &name) {
                    Ok(skipped) => self.finish_disk_sync(Self::flash_with_skips(
                        tf("status.restored", &[&name]),
                        &skipped,
                    )),
                    Err(e) => {
                        self.finish_disk_sync(tf("status.restore_failed", &[&format!("{e:#}")]))
                    }
                }
            }
            Overlay::ConfirmSync { kind, .. } => {
                let job = match (self.sync_tab, kind) {
                    (SyncTab::Webdav, SyncKind::Push) => Job::Push,
                    (SyncTab::Webdav, SyncKind::Pull) => Job::Pull,
                    (SyncTab::Gist, SyncKind::Push) => Job::GistPush,
                    (SyncTab::Gist, SyncKind::Pull) => Job::GistPull,
                };
                self.overlay = Overlay::None;
                self.start_job(job);
            }
            _ => self.overlay = Overlay::None,
        }
    }

    fn submit_form(&mut self) {
        let kind = match &self.overlay {
            Overlay::Form(form) => form.kind,
            _ => return,
        };
        match kind {
            form::FormKind::Add { .. } => {
                let opts = match overlay_form(&self.overlay).and_then(Form::add_opts) {
                    Ok(o) => o,
                    Err(e) => {
                        set_form_error(&mut self.overlay, e);
                        return;
                    }
                };
                match switch::add_provider(&self.paths, &mut self.store, opts) {
                    Ok(id) => {
                        self.overlay = Overlay::None;
                        self.status = tf("status.added", &[&id]);
                        self.focus_id(&id);
                    }
                    Err(e) => set_form_error(&mut self.overlay, e),
                }
            }
            form::FormKind::Edit { .. } => {
                let opts = match overlay_form(&self.overlay).and_then(Form::edit_opts) {
                    Ok(o) => o,
                    Err(e) => {
                        set_form_error(&mut self.overlay, e);
                        return;
                    }
                };
                match switch::edit_provider_quiet(&self.paths, &mut self.store, opts) {
                    Ok(result) => {
                        self.overlay = Overlay::None;
                        let updated = tf("status.updated", &[&result.name]);
                        self.status = match result.report.and_then(|r| reload_note(&r.reload)) {
                            Some(note) => format!("{updated} \u{b7} {note}"),
                            None => updated,
                        };
                        // Focus by id: a rename leaves the id alone, so the
                        // cursor lands on the edited row rather than falling
                        // back to the current provider.
                        self.focus_id(&result.id);
                    }
                    Err(e) => set_form_error(&mut self.overlay, e),
                }
            }
            form::FormKind::SyncSetup => {
                let values = match overlay_form(&self.overlay).and_then(Form::sync_setup) {
                    Ok(v) => v,
                    Err(e) => {
                        set_form_error(&mut self.overlay, e);
                        return;
                    }
                };
                let form = match std::mem::replace(&mut self.overlay, Overlay::None) {
                    Overlay::Form(f) => f,
                    other => {
                        self.overlay = other;
                        return;
                    }
                };
                self.setup_form = Some(form);
                let (url, username, password) = values;
                self.start_job(Job::Setup {
                    url,
                    username,
                    password,
                });
            }
            form::FormKind::GistSetup => {
                let values = match overlay_form(&self.overlay).and_then(Form::gist_setup) {
                    Ok(v) => v,
                    Err(e) => {
                        set_form_error(&mut self.overlay, e);
                        return;
                    }
                };
                let form = match std::mem::replace(&mut self.overlay, Overlay::None) {
                    Overlay::Form(f) => f,
                    other => {
                        self.overlay = other;
                        return;
                    }
                };
                self.setup_form = Some(form);
                let (token, gist_id) = values;
                self.start_job(Job::GistSetup {
                    token,
                    gist: gist_id,
                });
            }
        }
    }

    fn start_form_fetch(&mut self) {
        let Overlay::Form(form) = &self.overlay else {
            return;
        };
        let app = match form.kind {
            form::FormKind::Add { app } | form::FormKind::Edit { app } => app,
            form::FormKind::SyncSetup | form::FormKind::GistSetup => return,
        };
        let Some((url, key, protocol)) = form.fetch_creds() else {
            return;
        };
        let kind = match models::model_ui_for(app) {
            crate::adapter::models::ModelUi::Catalog { .. } => PickerKind::Catalog,
            crate::adapter::models::ModelUi::Slots { .. } => PickerKind::Slot { key: "default" },
        };
        let Overlay::Form(form) = std::mem::replace(&mut self.overlay, Overlay::FetchingModels)
        else {
            return;
        };
        self.held_form = Some(form);
        self.fetch_kind = Some(kind);
        self.fetch_rx = Some(models::spawn_fetch(url, key, protocol));
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) {
        if matches!(self.overlay, Overlay::ModelPicker(_)) {
            self.handle_picker_key(key);
            return;
        }
        if matches!(self.overlay, Overlay::CatalogEditor { .. }) {
            self.handle_catalog_key(key);
            return;
        }
        if matches!(self.overlay, Overlay::SlotEditor { .. }) {
            self.handle_slot_key(key);
            return;
        }
        if matches!(self.overlay, Overlay::SnippetEditor(_)) {
            self.handle_snippet_key(key);
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        let cmd = match &mut self.overlay {
            Overlay::ModelPicker(p) => p.handle_key(key),
            _ => return,
        };
        match cmd {
            PickerCmd::Continue => {}
            PickerCmd::Cancel => self.restore_after_picker(),
            PickerCmd::ConfirmCatalog(rows) => {
                let fields = match self.held_form.as_ref().and_then(|f| match f.kind {
                    form::FormKind::Add { app } | form::FormKind::Edit { app } => {
                        Some(models::model_ui_for(app))
                    }
                    _ => None,
                }) {
                    Some(crate::adapter::models::ModelUi::Catalog { fields }) => fields,
                    _ => crate::adapter::models::OPENCODE_FIELDS,
                };
                let default = rows.first().map(|r| r.id.clone());
                self.overlay = Overlay::CatalogEditor {
                    editor: CatalogEditor::new(fields, rows, default.as_deref()),
                };
            }
            PickerCmd::ConfirmSlot { key, id } => {
                if let Some(mut editor) = self.held_slots.take() {
                    if key == "default" {
                        editor.default_model = id;
                    } else {
                        editor.values.insert(key.to_string(), id);
                    }
                    self.overlay = Overlay::SlotEditor { editor };
                } else {
                    if let Some(form) = self.held_form.as_mut() {
                        if key == "default" {
                            if let Some(f) = form
                                .fields
                                .iter_mut()
                                .find(|f| f.storage == Some(crate::adapter::FieldStorage::Model))
                            {
                                f.value = id;
                            }
                        } else {
                            form.slots.insert(key.to_string(), id);
                        }
                    }
                    self.restore_held_form();
                }
            }
        }
    }

    fn handle_catalog_key(&mut self, key: KeyEvent) {
        let cmd = match &mut self.overlay {
            Overlay::CatalogEditor { editor, .. } => editor.handle_key(key),
            _ => return,
        };
        match cmd {
            CatalogCmd::Continue => {
                // The editor sets `pending_dropped_slots` and
                // `deleted_default_to` *only* from `delete_row`; popover
                // and picker paths leave them untouched. Read+reset
                // them here so non-delete operations never trigger
                // a "row deleted" status.
                let (dropped, default_moved) = match &mut self.overlay {
                    Overlay::CatalogEditor { editor, .. } => (
                        std::mem::take(&mut editor.pending_dropped_slots),
                        editor.deleted_default_to.take(),
                    ),
                    _ => return,
                };
                if dropped > 0 {
                    self.status = if dropped == 1 {
                        t("status.catalog_row_dropped_one_slot").into()
                    } else {
                        tf(
                            "status.catalog_row_dropped_n_slots",
                            &[&dropped.to_string()],
                        )
                    };
                }
                if let Some(new_default) = default_moved {
                    self.status = match new_default {
                        Some(id) => tf("status.catalog_default_moved", &[&id]),
                        None => t("status.catalog_default_removed").into(),
                    };
                }
            }
            CatalogCmd::Cancel => {
                if self.held_form.is_some() {
                    self.restore_held_form();
                } else {
                    self.overlay = Overlay::None;
                }
            }
            CatalogCmd::Save => self.save_catalog_editor(),
        }
    }

    fn handle_slot_key(&mut self, key: KeyEvent) {
        let cmd = match &mut self.overlay {
            Overlay::SlotEditor { editor, .. } => editor.handle_key(key),
            _ => return,
        };
        match cmd {
            SlotCmd::Continue => {}
            SlotCmd::Cancel => {
                if self.held_form.is_some() {
                    self.restore_held_form();
                } else {
                    self.overlay = Overlay::None;
                }
            }
            SlotCmd::Save => self.save_slot_editor(),
            SlotCmd::Fetch => self.start_slot_fetch(),
        }
    }

    fn start_slot_fetch(&mut self) {
        let kind = match &self.overlay {
            Overlay::SlotEditor { editor, .. } => PickerKind::Slot {
                key: editor.focused_slot_key().unwrap_or("default"),
            },
            _ => return,
        };
        let Some((url, api_key, protocol)) = self.held_form.as_ref().and_then(Form::fetch_creds)
        else {
            self.status = t("status.no_edit").into();
            return;
        };
        let Overlay::SlotEditor { editor } =
            std::mem::replace(&mut self.overlay, Overlay::FetchingModels)
        else {
            return;
        };
        self.held_slots = Some(editor);
        self.fetch_kind = Some(kind);
        self.fetch_rx = Some(models::spawn_fetch(url, api_key, protocol));
    }

    fn restore_held_form(&mut self) {
        if let Some(form) = self.held_form.take() {
            self.overlay = Overlay::Form(form);
        } else {
            self.overlay = Overlay::None;
        }
    }

    fn restore_after_picker(&mut self) {
        if let Some(editor) = self.held_slots.take() {
            self.overlay = Overlay::SlotEditor { editor };
        } else {
            self.restore_held_form();
        }
    }

    fn save_catalog_editor(&mut self) {
        let Overlay::CatalogEditor { editor, .. } =
            std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return;
        };
        let rows: Vec<_> = editor
            .rows
            .iter()
            .filter(|r| !r.id.trim().is_empty())
            .cloned()
            .collect();
        let default_id = editor.default_id();
        let slot_owner = editor.slot_owner.clone();
        if let Some(form) = self.held_form.as_mut() {
            form.catalog = rows;
            // Project slot_owner into form.slots. We only copy bindings
            // whose target id still exists in the saved catalog.
            let ids: std::collections::BTreeSet<&str> =
                form.catalog.iter().map(|r| r.id.as_str()).collect();
            form.slots = slot_owner
                .into_iter()
                .filter(|(_, id)| ids.contains(id.as_str()))
                .map(|(k, id)| (k.to_string(), id))
                .collect();
            if let Some(mid) = default_id {
                if let Some(f) = form
                    .fields
                    .iter_mut()
                    .find(|f| f.storage == Some(crate::adapter::FieldStorage::Model))
                {
                    f.value = mid;
                }
            }
            form.refresh_meta_summaries();
        }
        self.restore_held_form();
    }

    fn save_slot_editor(&mut self) {
        let Overlay::SlotEditor { editor, .. } =
            std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return;
        };
        let model = if editor.default_model.trim().is_empty() {
            None
        } else {
            Some(editor.default_model.clone())
        };
        let slots = editor
            .values
            .iter()
            .filter(|(k, v)| {
                crate::adapter::models::known_slot(k).is_some() && !v.trim().is_empty()
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Some(form) = self.held_form.as_mut() {
            form.slots = slots;
            if let Some(mid) = model {
                if let Some(f) = form
                    .fields
                    .iter_mut()
                    .find(|f| f.storage == Some(crate::adapter::FieldStorage::Model))
                {
                    f.value = mid;
                }
            }
            form.refresh_meta_summaries();
        }
        self.restore_held_form();
    }

    fn save_snippet(&mut self) {
        let Overlay::SnippetEditor(page) = &mut self.overlay else {
            return;
        };
        let value = match page.parsed_snippet() {
            Ok(v) => v,
            Err(e) => {
                page.error = Some(e);
                return;
            }
        };
        let extras = page.extras.clone();
        let snippet = crate::store::normalize_snippet(Some(value));
        let Overlay::SnippetEditor(_) = std::mem::replace(&mut self.overlay, Overlay::None) else {
            return;
        };
        if let Some(form) = self.held_form.as_mut() {
            form.snippet = snippet;
            form.quick_extras = extras;
            form.refresh_meta_summaries();
        }
        self.restore_held_form();
    }

    fn start_job(&mut self, job: Job) {
        if self.sync_rx.is_some() || matches!(self.overlay, Overlay::Syncing) {
            return;
        }
        match job {
            Job::Push | Job::Pull if cloud::local_sync(&self.paths).is_none() => {
                self.status = t("status.sync_unconfigured").into();
                return;
            }
            Job::GistPush | Job::GistPull => {
                let configured =
                    gist::local_gist(&self.paths).is_some_and(|g| !g.gist_id.is_empty());
                if !configured {
                    self.status = t("status.gist_unconfigured").into();
                    return;
                }
            }
            _ => {}
        }
        self.help = false;
        self.overlay = Overlay::Syncing;
        self.sync_rx = Some(sync::spawn(self.paths.clone(), job));
    }

    fn start_speed_test(&mut self) {
        if self.speed_rx.is_some() {
            return;
        }
        let Some(p) = self.providers().get(self.selected).cloned() else {
            return;
        };
        // official rows have no endpoint to probe
        if p.official || p.base_url.trim().is_empty() {
            self.status = tf("status.test_err", &[&p.name, t("status.test_no_endpoint")]);
            return;
        }
        let store = self.store.clone();
        let id = p.id.clone();
        let name = p.name.clone();
        self.speed_name = Some(name.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let res = crate::webdav::block_on(async {
                crate::speedtest::test_provider_by_id(&store, &id).await
            });
            let _ = tx.send(res.map_err(|e| format!("{e:#}")));
        });
        self.speed_rx = Some(rx);
        self.status = tf("status.testing", &[&name]);
    }

    pub fn poll_speed(&mut self) {
        let msg = {
            let Some(rx) = &self.speed_rx else { return };
            match rx.try_recv() {
                Ok(v) => Some(v),
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
            }
        };
        let name = self.speed_name.take();
        self.speed_rx = None;
        match msg {
            Some(Ok(r)) => {
                self.status = tf(
                    "status.test_ok",
                    &[
                        &r.name,
                        &r.latency
                            .map(|l| l.as_millis().to_string())
                            .unwrap_or_default(),
                        &r.status.map(|s| s.to_string()).unwrap_or_default(),
                    ],
                );
            }
            _ => {
                let detail = msg
                    .and_then(|m| m.err())
                    .unwrap_or_else(|| "interrupted".into());
                self.status = tf("status.test_err", &[&name.unwrap_or_default(), &detail]);
            }
        }
    }

    fn queue_try_launch(&mut self) {
        let Some(p) = self.providers().get(self.selected).cloned() else {
            return;
        };
        match TryJob::for_provider(&self.store, &p.id) {
            Ok(job) => {
                self.help = false;
                self.status = tf("status.try_starting", &[&job.provider_name]);
                self.pending_try = Some(job);
            }
            Err(e) => {
                self.help = false;
                self.status = format!("{e:#}");
            }
        }
    }

    /// The event loop takes this and runs it with the terminal suspended.
    pub fn take_pending_try(&mut self) -> Option<TryJob> {
        self.pending_try.take()
    }

    pub fn note_try_result(&mut self, name: String, result: Result<std::process::ExitStatus>) {
        match result {
            Ok(status) => {
                self.status = tf(
                    "status.try_done",
                    &[
                        &name,
                        &status
                            .code()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "signal".into()),
                    ],
                );
            }
            Err(e) => self.status = tf("status.try_failed", &[&format!("{e:#}")]),
        }
    }

    pub fn poll_sync(&mut self) -> bool {
        self.poll_speed();
        if self.poll_fetch() {
            return true;
        }
        let msg = {
            let Some(rx) = &self.sync_rx else {
                return false;
            };
            match rx.try_recv() {
                Ok(v) => Some(Ok(v)),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => Some(Err(())),
            }
        };
        self.sync_rx = None;
        if matches!(self.overlay, Overlay::Syncing) {
            self.overlay = Overlay::None;
        }
        match msg {
            Some(Ok(out)) => self.apply_outcome(out),
            Some(Err(())) => self.status = t("status.sync_interrupted").into(),
            None => {}
        }
        true
    }

    fn poll_fetch(&mut self) -> bool {
        let msg = {
            let Some(rx) = &self.fetch_rx else {
                return false;
            };
            match rx.try_recv() {
                Ok(v) => Some(v),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => Some(Err("interrupted".into())),
            }
        };
        self.fetch_rx = None;
        let kind = self.fetch_kind.take().unwrap_or(PickerKind::Catalog);
        match msg {
            Some(Ok(ids)) => {
                let pre = self.picker_preselect(&kind);
                let mut picker = ModelPicker::with_preselect(kind, ids, &pre);
                if matches!(kind, PickerKind::Catalog) {
                    if let Some(form) = self.held_form.as_ref() {
                        picker = picker.with_meta(&form.catalog);
                    }
                }
                self.overlay = Overlay::ModelPicker(picker);
            }
            Some(Err(e)) => {
                self.status = e;
                self.restore_after_picker();
            }
            None => {}
        }
        true
    }

    /// Selections to re-show in a fresh fetch picker: the focused slot's
    /// current id (slot-editor path) or the form's saved catalog / model.
    fn picker_preselect(&self, kind: &PickerKind) -> Vec<String> {
        let non_empty = |v: Option<String>| v.filter(|s| !s.is_empty()).into_iter().collect();
        if let Some(ed) = self.held_slots.as_ref() {
            let v = match kind {
                PickerKind::Slot { key } if *key != "default" => ed.values.get(*key).cloned(),
                _ => Some(ed.default_model.clone()),
            };
            return non_empty(v);
        }
        let Some(form) = self.held_form.as_ref() else {
            return Vec::new();
        };
        match kind {
            PickerKind::Catalog => form.catalog.iter().map(|e| e.id.clone()).collect(),
            PickerKind::Slot { .. } => non_empty(
                form.fields
                    .iter()
                    .find(|f| f.storage == Some(crate::adapter::FieldStorage::Model))
                    .map(|f| f.value.clone()),
            ),
        }
    }

    fn apply_outcome(&mut self, out: Outcome) {
        match out {
            Outcome::Setup(r) => match r {
                Ok(()) => {
                    self.setup_form = None;
                    self.sync_local = cloud::local_sync(&self.paths);
                    self.status = t("status.sync_configured").into();
                }
                Err(e) => {
                    if let Some(mut form) = self.setup_form.take() {
                        form.error = Some(e.to_string());
                        self.overlay = Overlay::Form(form);
                    } else {
                        self.status = tf("status.setup_failed", &[&format!("{e:#}")]);
                    }
                }
            },
            Outcome::GistSetup(r) => match r {
                Ok(id) => {
                    self.setup_form = None;
                    self.gist_local = gist::local_gist(&self.paths);
                    self.status = tf("status.gist_configured", &[&id]);
                }
                Err(e) => {
                    if let Some(mut form) = self.setup_form.take() {
                        form.error = Some(e.to_string());
                        self.overlay = Overlay::Form(form);
                    } else {
                        self.status = tf("status.setup_failed", &[&format!("{e:#}")]);
                    }
                }
            },
            Outcome::Push(r) => match r {
                Ok(sha) => {
                    self.sync_local = cloud::local_sync(&self.paths);
                    self.status = tf("status.pushed", &[&sha]);
                    if matches!(self.page, Page::Data) {
                        self.refresh_backups();
                    }
                }
                Err(e) => self.status = tf("status.push_failed", &[&format!("{e:#}")]),
            },
            Outcome::GistPush(r) => match r {
                Ok(sha) => {
                    self.gist_local = gist::local_gist(&self.paths);
                    self.status = tf("status.pushed", &[&sha]);
                }
                Err(e) => self.status = tf("status.push_failed", &[&format!("{e:#}")]),
            },
            Outcome::Pull(r) => {
                // Disk (store.json / webdav.json) may already be updated when
                // re-apply fails; always refresh TUI memory like restore.
                self.sync_local = cloud::local_sync(&self.paths);
                match r {
                    Ok(sha) => self.finish_disk_sync(tf("status.pulled", &[&sha])),
                    Err(e) => self.finish_disk_sync(tf("status.pull_failed", &[&format!("{e:#}")])),
                }
            }
            Outcome::GistPull(r) => {
                // Same as the WebDAV pull: disk may already carry the new
                // store when re-apply failed; always refresh TUI memory.
                self.gist_local = gist::local_gist(&self.paths);
                match r {
                    Ok(sha) => self.finish_disk_sync(tf("status.pulled", &[&sha])),
                    Err(e) => self.finish_disk_sync(tf("status.pull_failed", &[&format!("{e:#}")])),
                }
            }
        }
    }

    fn finish_disk_sync(&mut self, primary: String) {
        if let Some(e) = self.reload_store() {
            self.status = format!("{primary}; {e}");
        } else {
            self.status = primary;
        }
    }

    fn flash_with_skips(primary: String, skipped: &[AppId]) -> String {
        if skipped.is_empty() {
            return primary;
        }
        let extra: Vec<String> = skipped
            .iter()
            .map(|app| {
                let name = adapter::get(*app).map(|a| a.display_name()).unwrap_or("?");
                tf("status.skip_uninitialized", &[name])
            })
            .collect();
        format!("{primary} · {}", extra.join(" · "))
    }

    fn reload_store(&mut self) -> Option<String> {
        match Store::load(&self.paths) {
            Ok(store) => {
                self.store = store;
                self.focus_current();
                None
            }
            Err(e) => Some(tf("status.reload_failed", &[&format!("{e:#}")])),
        }
    }
}

fn overlay_form(overlay: &Overlay) -> Result<&Form, anyhow::Error> {
    match overlay {
        Overlay::Form(form) => Ok(form),
        _ => anyhow::bail!("no form"),
    }
}

fn set_form_error(overlay: &mut Overlay, e: impl std::fmt::Display) {
    if let Overlay::Form(form) = overlay {
        form.error = Some(e.to_string());
    }
}

/// Status-bar note for a post-apply reload. Only a daemon apmux refreshed
/// (or failed to refresh) needs saying; otherwise the generic restart hint
/// in the caller still applies.
fn reload_note(reload: &ReloadOutcome) -> Option<String> {
    match reload {
        ReloadOutcome::Restarted { .. } => Some(t("status.reloaded_short").into()),
        ReloadOutcome::Failed(_) => Some(t("status.reload_failed_short").into()),
        ReloadOutcome::NotApplicable | ReloadOutcome::NotRunning => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::FieldKind;
    use crate::store::Provider;
    use crate::tui::keymap::Action;
    use crate::tui::view;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn sample(paths: Paths) -> App {
        let mut store = Store::empty();
        store.providers.insert(
            "packy".into(),
            Provider {
                id: "packy".into(),
                name: "PackyCode".into(),
                app: AppId::Claude,
                base_url: "https://example.com".into(),
                api_key: "sk-test-key-abcd".into(),
                model: None,
                extras: Default::default(),
                ..Provider::blank(AppId::Claude)
            },
        );
        store.providers.insert(
            "other".into(),
            Provider {
                id: "other".into(),
                name: "Other".into(),
                app: AppId::Claude,
                base_url: "https://example.com".into(),
                api_key: "sk-other-keyxxx".into(),
                model: None,
                extras: Default::default(),
                ..Provider::blank(AppId::Claude)
            },
        );
        store.current.insert(AppId::Claude, "packy".into());
        let mut app = App::new(paths, store);
        app.settings.apps_mode = settings::AppsMode::Manual;
        app.settings.visible = settings::all_apps();
        app
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn compact(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn registry_drives_tab_count() {
        let td = tempfile::tempdir().unwrap();
        let app = sample(Paths::for_test(td.path()));
        assert_eq!(app.app_count(), adapter::registry().len());
        assert_eq!(app.tab_titles().len(), adapter::registry().len());
        assert_eq!(app.tab_titles()[0], "Claude");
        for title in app.tab_titles() {
            assert!(
                !title.chars().next().is_some_and(|c| c.is_ascii_digit()),
                "tab titles must not be numbered: {title}"
            );
        }
    }

    #[test]
    fn enter_switches_provider_via_use_provider() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        assert_eq!(app.current_id(), Some("packy"));
        app.selected = 1;
        app.handle_action(Action::Switch);
        assert_eq!(
            app.store.current.get(&AppId::Claude).map(String::as_str),
            Some("other")
        );
        assert!(app.status.contains("Other"));
        assert!(
            app.status.contains("config folder") || app.status.contains("nothing was written"),
            "{}",
            app.status
        );
    }

    #[test]
    fn brackets_cycle_registry_tabs() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        assert_eq!(app.current_app(), AppId::Claude);
        app.handle_action(Action::NextApp);
        assert_eq!(app.current_app(), adapter::registry()[1].id());
        app.handle_action(Action::PrevApp);
        assert_eq!(app.current_app(), AppId::Claude);
        app.app_idx = app.app_count() - 1;
        app.focus_current();
        assert_eq!(
            app.current_app(),
            adapter::registry()[app.app_count() - 1].id()
        );
        app.handle_key(key(KeyCode::Char(']')));
        assert_eq!(app.current_app(), AppId::Claude);
        app.handle_key(key(KeyCode::Char('[')));
        assert_eq!(
            app.current_app(),
            adapter::registry()[app.app_count() - 1].id()
        );
    }

    #[test]
    fn render_masks_keys_and_highlights_current() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(text.contains("PackyCode"));
        assert!(compact.contains("Providers"), "{text}");
        assert!(!text.contains("sk-test-key-abcd"));
        assert!(!text.contains("sk-other-keyxxx"));
        app.handle_action(Action::ToggleHelp);
        terminal.draw(|f| view::draw(f, &mut app)).unwrap();
        let help = buffer_text(&terminal);
        let help_compact: String = help.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            help_compact.contains("Help") || help_compact.contains("Shortcuts"),
            "{help}"
        );
        assert!(help.contains("Enter"));
        assert!(!help.contains("..="), "{help}");
        assert!(help.contains('[') && help.contains(']'), "{help}");
        assert!(!help.contains("1-"), "{help}");
        assert!(help.contains('a') && help.contains('e') && help.contains('d'));
        assert!(help.contains('b') && help.contains('r') && help.contains('s'));
    }

    #[test]
    fn footer_keeps_hints_after_cancel() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        assert_eq!(app.hint(), crate::tui::keymap::hint_bar(KeyMode::List));
        assert!(app.status.is_empty());

        app.handle_action(Action::Edit);
        assert!(matches!(app.overlay, Overlay::Form(_)));
        assert_eq!(app.hint(), t("ui.form_hint"));

        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(app.hint(), crate::tui::keymap::hint_bar(KeyMode::List));
        assert!(app.status.is_empty());

        app.handle_action(Action::Delete);
        assert_eq!(app.hint(), t("ui.confirm_hint"));
        app.handle_confirm_key(key(KeyCode::Esc));
        assert_eq!(app.hint(), crate::tui::keymap::hint_bar(KeyMode::List));
        assert!(app.status.is_empty());

        app.handle_action(Action::OpenData);
        assert_eq!(app.hint(), crate::tui::keymap::hint_bar(KeyMode::Data));
        assert!(app.status.is_empty());

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Enter"), "{text}");
        assert!(!text.contains("Cancelled"), "{text}");
        app.page = Page::Providers;
        terminal.draw(|f| view::draw(f, &mut app)).unwrap();
        let list = buffer_text(&terminal);
        assert!(list.contains("add") && list.contains("edit"), "{list}");
        assert!(!list.contains("Cancelled"), "{list}");
        assert!(!list.contains("warning:"), "{list}");
    }

    #[test]
    fn settings_toggles_lang_and_manual_apps() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths.clone());
        crate::i18n::set(crate::i18n::Lang::En);
        app.handle_action(Action::OpenSettings);
        assert!(matches!(app.page, Page::Settings));
        app.settings_sel = crate::tui::pages::settings::ROW_LANG;
        app.handle_action(Action::ToggleSetting);
        assert_eq!(crate::i18n::lang(), crate::i18n::Lang::Zh);
        app.settings_sel = crate::tui::pages::settings::ROW_MODE;
        app.handle_action(Action::ToggleSetting);
        assert_eq!(app.settings.apps_mode, settings::AppsMode::Auto);
        let loaded = settings::Settings::load(&paths).unwrap();
        assert_eq!(loaded.lang.as_deref(), Some("zh"));
        assert_eq!(loaded.apps_mode, settings::AppsMode::Auto);
        crate::i18n::set(crate::i18n::Lang::En);
    }

    #[test]
    fn add_form_is_fields_driven_and_submits() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        app.handle_action(Action::Add);
        let Overlay::Form(form) = &app.overlay else {
            panic!("expected form");
        };
        assert_eq!(
            form.fields.len(),
            adapter::get(AppId::Claude).unwrap().fields().len() + 3
        );
        assert!(form.fields.iter().any(|f| f.key == "apply_snippet"));
        for f in &form.fields {
            match f.key {
                "name" => {}
                "base_url" => {}
                "api_key" => assert!(matches!(f.kind, FieldKind::Secret)),
                "model" => assert!(!f.required),
                _ => {}
            }
        }
        let Overlay::Form(form) = &mut app.overlay else {
            panic!("form");
        };
        for f in &mut form.fields {
            match f.key {
                "name" => f.value = "NewCo".into(),
                "base_url" => f.value = "https://api.example.com".into(),
                "api_key" => {
                    f.secret_keep = false;
                    f.value = "sk-new-key-zzzz".into();
                }
                "model" => f.value.clear(),
                _ => {}
            }
        }
        app.submit_form();
        assert!(matches!(app.overlay, Overlay::None), "{}", app.status);
        let p = app.store.providers.get("newco").expect("added");
        assert_eq!(p.name, "NewCo");
        assert_eq!(p.model, None);
        assert_eq!(p.api_key, "sk-new-key-zzzz");
    }

    #[test]
    fn required_model_blocks_submit() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        app.app_idx = adapter::registry()
            .iter()
            .position(|a| a.id() == AppId::Pi)
            .unwrap();
        app.focus_current();
        app.handle_action(Action::Add);
        {
            let Overlay::Form(form) = &mut app.overlay else {
                panic!("form");
            };
            for f in &mut form.fields {
                match f.key {
                    "name" => f.value = "PiProv".into(),
                    "base_url" => f.value = "https://api.example.com".into(),
                    "api_key" => f.value = "sk-pi-key-xxxx".into(),
                    "model" => f.value.clear(),
                    _ => {}
                }
            }
        }
        app.submit_form();
        let Overlay::Form(form) = &app.overlay else {
            panic!("must stay on form");
        };
        let err = form.error.as_deref().unwrap_or("");
        assert!(err.contains("must not be empty"), "{err}");
        assert!(!app.store.providers.contains_key("piprov"));
    }

    #[test]
    fn edit_keeps_secret_and_clears_optional_model() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        app.store.providers.get_mut("packy").unwrap().model = Some("old".into());
        app.handle_action(Action::Edit);
        {
            let Overlay::Form(form) = &mut app.overlay else {
                panic!("form");
            };
            let key = form.fields.iter().find(|f| f.key == "api_key").unwrap();
            assert!(key.secret_keep);
            for f in &mut form.fields {
                if f.key == "model" {
                    f.value.clear();
                }
                if f.key == "name" {
                    f.value = "Renamed".into();
                }
            }
        }
        app.submit_form();
        assert!(matches!(app.overlay, Overlay::None), "{}", app.status);
        let p = app.store.providers.get("packy").unwrap();
        assert_eq!(p.api_key, "sk-test-key-abcd");
        assert_eq!(p.model, None);
        assert_eq!(p.name, "Renamed");
    }

    #[test]
    fn editing_a_row_leaves_the_cursor_on_it() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        // Edit the row that is *not* current and rename it. The name no longer
        // matches anything, so focusing by display name would snap the cursor
        // back to `packy`; focusing by id keeps it on the row just edited.
        app.focus_id("other");
        app.handle_action(Action::Edit);
        {
            let Overlay::Form(form) = &mut app.overlay else {
                panic!("form");
            };
            form.fields
                .iter_mut()
                .find(|f| f.key == "name")
                .unwrap()
                .value = "Renamed".into();
        }
        app.submit_form();
        assert!(matches!(app.overlay, Overlay::None), "{}", app.status);
        assert_eq!(app.store.providers.get("other").unwrap().name, "Renamed");
        assert_eq!(app.providers()[app.selected].id, "other");
    }

    #[test]
    fn delete_requires_confirm() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        app.handle_action(Action::Delete);
        assert!(matches!(app.overlay, Overlay::ConfirmDelete { .. }));
        app.handle_confirm_key(key(KeyCode::Char('n')));
        assert!(app.store.providers.contains_key("packy"));
        app.handle_action(Action::Delete);
        app.handle_confirm_key(key(KeyCode::Char('y')));
        assert!(!app.store.providers.contains_key("packy"));
        assert!(app.status.contains("PackyCode"));
    }

    #[test]
    fn data_page_draws_backup_list_and_sync_block() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths.clone());
        app.store.save(&paths).unwrap();
        backup::create(&paths, None).unwrap();
        app.handle_action(Action::OpenData);
        assert!(matches!(app.page, Page::Data));
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| view::draw(f, &mut app)).unwrap();
        let text = buffer_text(&term);
        assert!(text.contains("Data"), "{text}");
        assert!(text.contains("Backups"), "{text}");
        assert!(text.contains("Sync"), "{text}");
        // restore flow still works from the merged page
        app.handle_key(key(KeyCode::Enter));
    }

    #[test]
    fn speed_test_reports_testing_then_result() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths.clone());
        // make row 0 the official row: it must be rejected, not probed
        if let Some(p) = app.store.providers.get_mut("packy") {
            p.official = true;
        }
        app.store.save(&paths).unwrap();

        app.selected = 0;
        app.handle_action(Action::SpeedTest);
        assert!(
            app.status.contains("no endpoint") || app.status.contains("端点"),
            "{}",
            app.status
        );
        assert!(app.speed_rx.is_none());

        // row 1 probes a local mock endpoint — never the network
        let srv = crate::webdav::mock::MockServer::start();
        if let Some(p) = app.store.providers.get_mut("other") {
            p.base_url = srv.collection_url("/v1");
        }
        app.selected = 1;
        app.handle_action(Action::SpeedTest);
        assert!(app.speed_rx.is_some(), "expected a probe to start");
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            app.poll_speed();
            if app.speed_rx.is_none() {
                break;
            }
        }
        assert!(app.speed_rx.is_none(), "probe never finished");
        assert!(
            app.status.contains("HTTP") || app.status.contains("不可达"),
            "{}",
            app.status
        );
    }

    #[test]
    fn try_launch_queues_job_and_rejects_official() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths.clone());
        if let Some(p) = app.store.providers.get_mut("packy") {
            p.official = true;
        }
        app.store.save(&paths).unwrap();
        // official row: rejected with a message, nothing queued
        app.selected = 0;
        app.handle_action(Action::TryLaunch);
        assert!(app.pending_try.is_none());
        // normal row: job staged (bin resolution may fail if codex absent —
        // then status carries the error instead)
        app.selected = 1;
        app.handle_action(Action::TryLaunch);
        match app.take_pending_try() {
            Some(job) => assert!(!job.provider_name.is_empty()),
            None => assert!(!app.status.is_empty(), "expected error status"),
        }
    }

    #[test]
    fn applied_switch_appends_restart_hint() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths.clone());
        app.store.save(&paths).unwrap();
        app.selected = 0;
        let was_official = {
            let id = app.current_id().unwrap();
            app.store.providers.get(id).map(|p| p.official) == Some(true)
        };
        // pick the non-current row so the switch actually writes live config
        app.selected = 1;
        if was_official {
            app.selected = 0;
        }
        app.handle_action(Action::Switch);
        assert!(
            app.status.contains("restart to apply") || app.status.contains("config folder"),
            "{}",
            app.status
        );
    }

    #[test]
    fn backup_and_restore_pages() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths.clone());
        app.store.save(&paths).unwrap();
        // b snapshots from within the Data page (no longer a global key)
        app.handle_action(Action::OpenData);
        assert!(matches!(app.page, Page::Data));
        app.handle_action(Action::Backup);
        assert!(app.status.contains("Backed up"), "{}", app.status);
        assert!(!app.backups.is_empty());
        assert!(app.backups.iter().any(|e| e.timestamp));
        backup::create(&paths, Some("named-one")).unwrap();
        app.refresh_backups();
        assert!(app
            .backups
            .iter()
            .any(|e| e.name == "named-one" && !e.timestamp));
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        let compact = compact(&text);
        assert!(text.contains("named-one"), "{text}");
        assert!(
            compact.contains("timestamp") || compact.contains("named"),
            "{text}"
        );
        app.backup_sel = app
            .backups
            .iter()
            .position(|e| e.name == "named-one")
            .unwrap();
        app.handle_action(Action::Restore);
        assert!(matches!(app.overlay, Overlay::ConfirmRestore { .. }));
        app.handle_confirm_key(key(KeyCode::Char('y')));
        assert!(
            app.status.contains("Restored") || app.status.contains("named-one"),
            "{}",
            app.status
        );
    }

    #[test]
    fn sync_page_and_static_overlay() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        app.handle_action(Action::OpenData);
        assert!(matches!(app.page, Page::Data));
        app.handle_action(Action::ToggleHelp);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view::draw(f, &mut app)).unwrap();
        let help = buffer_text(&terminal);
        let help_c = compact(&help);
        assert!(
            help_c.contains("push") || help_c.contains("setup"),
            "{help}"
        );
        app.help = false;
        app.overlay = Overlay::Syncing;
        terminal.draw(|f| view::draw(f, &mut app)).unwrap();
        let busy = buffer_text(&terminal);
        let busy_c = compact(&busy);
        assert!(busy_c.contains("Syncing"), "{busy}");
        assert!(!busy.contains("◐") && !busy.contains("⠋"), "{busy}");
        app.handle_key(key(KeyCode::Char('p')));
        assert!(matches!(app.overlay, Overlay::Syncing));
        assert!(!app.handle_key(key(KeyCode::Char('a'))));
        assert!(app.handle_key(key(KeyCode::Char('q'))));
    }

    #[test]
    fn form_models_field_opens_editor() {
        // Claude's `models` field now opens the CatalogEditor (not the
        // SlotEditor). For codex/pi/opencode it's still the model picker.
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        app.handle_action(Action::Edit);
        {
            let Overlay::Form(form) = &mut app.overlay else {
                panic!("form");
            };
            form.focus = form
                .fields
                .iter()
                .position(|f| f.key == "models")
                .expect("models");
        }
        app.handle_form_key(key(KeyCode::Char(' ')));
        // The overlay is either CatalogEditor (Claude) or the picker (others).
        assert!(matches!(
            app.overlay,
            Overlay::CatalogEditor { .. } | Overlay::ModelPicker { .. }
        ));
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.overlay, Overlay::Form(_)));
    }

    #[test]
    fn contextual_help_changes_with_page() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        let list = crate::tui::help::text(&app);
        assert!(list.contains("add"));
        app.page = Page::Data;
        let d = crate::tui::help::text(&app);
        assert!(d.contains("restore") && d.contains("push") || d.contains("apmux-sync"));
        app.page = Page::Settings;
        let set = crate::tui::help::text(&app);
        assert!(set.contains("Language") || set.contains("detection") || set.contains("Space"));
        app.overlay = Overlay::Syncing;
        let busy = crate::tui::help::text(&app);
        assert!(busy.contains("Sync"));
    }

    #[test]
    fn sync_setup_form_uses_submitted_url() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        app.handle_action(Action::OpenData);
        app.handle_action(Action::SyncSetup);
        {
            let Overlay::Form(form) = &mut app.overlay else {
                panic!("setup form");
            };
            form.fields
                .iter_mut()
                .find(|f| f.key == "url")
                .unwrap()
                .value = "https://webdav.example.com/".into();
            let ns = form.fields.iter().find(|f| f.key == "namespace").unwrap();
            assert_eq!(ns.value, crate::webdav::NAMESPACE);
            assert!(ns.readonly);
            let (url, user, pass) = {
                for f in &mut form.fields {
                    match f.key {
                        "username" => f.value = "user".into(),
                        "password" => {
                            f.secret_keep = false;
                            f.value = "pass".into();
                        }
                        _ => {}
                    }
                }
                form.sync_setup().unwrap()
            };
            assert_eq!(url, "https://webdav.example.com/");
            assert_eq!(user, "user");
            assert_eq!(pass, "pass");
        }
    }

    #[test]
    fn sync_setup_mkcol_persists_submitted_url() {
        use std::time::{Duration, Instant};

        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let srv = crate::webdav::mock::MockServer::start();
        let url = srv.collection_url("/dav");
        let mut app = sample(paths.clone());
        app.handle_action(Action::OpenData);
        app.handle_action(Action::SyncSetup);
        {
            let Overlay::Form(form) = &mut app.overlay else {
                panic!("setup form");
            };
            for f in &mut form.fields {
                match f.key {
                    "url" => f.value = url.clone(),
                    "username" => f.value = "u".into(),
                    "password" => {
                        f.secret_keep = false;
                        f.value = "p".into();
                    }
                    _ => {}
                }
            }
        }
        app.submit_form();
        assert!(app.is_syncing());
        let start = Instant::now();
        while app.is_syncing() && start.elapsed() < Duration::from_secs(8) {
            if app.poll_sync() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!app.is_syncing(), "{}", app.status);
        assert!(
            app.status.contains("configured") || app.status.contains("Sync"),
            "{}",
            app.status
        );
        let local = cloud::local_sync(&paths).expect("webdav.json");
        assert_eq!(local.url, url);
        assert!(!local.url.contains("apmux-sync"));
        let log = srv.methods();
        assert!(
            log.iter()
                .any(|l| l.contains("MKCOL") && l.contains("/dav/apmux-sync")),
            "{log:?}"
        );
    }

    #[test]
    fn pull_reapply_failure_still_reloads_store() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths.clone());
        app.store.save(&paths).unwrap();

        let mut remote = Store::empty();
        remote.providers.insert(
            "from-cloud".into(),
            Provider {
                id: "from-cloud".into(),
                name: "Cloud".into(),
                app: AppId::Claude,
                base_url: "https://example.com".into(),
                api_key: "sk-from-cloudxx".into(),
                model: None,
                extras: Default::default(),
                ..Provider::blank(AppId::Claude)
            },
        );
        remote.save(&paths).unwrap();

        app.apply_outcome(super::Outcome::Pull(Err(anyhow::anyhow!(
            "store pulled but re-apply failed"
        ))));
        assert!(
            app.store.providers.contains_key("from-cloud"),
            "TUI must reload pulled store on re-apply err: {:?}",
            app.store.providers.keys().collect::<Vec<_>>()
        );
        assert!(!app.store.providers.contains_key("packy"));
        assert!(app.status.contains("Pull failed"), "{}", app.status);
    }

    #[test]
    fn reload_store_surfaces_load_error() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths.clone());
        app.store.save(&paths).unwrap();
        std::fs::write(paths.store_file(), b"not-json{{{").unwrap();
        app.apply_outcome(super::Outcome::Pull(Ok("abc".into())));
        assert!(app.status.contains("reload"), "{}", app.status);
        assert!(
            app.store.providers.contains_key("packy"),
            "memory stays on load failure"
        );
    }

    #[test]
    fn deleting_catalog_row_with_slot_binding_surfaces_status() {
        // If a deleted row owns a slot binding, the status bar reports
        // how many slots were cleared so the user isn't left wondering
        // where the sonnet went.
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);

        // Build a CatalogEditor with two rows; assign sonnet+opus to row 1.
        use crate::store::ModelEntry;
        use crate::tui::pages::models::CatalogEditor;
        let mut p = crate::store::Provider::blank(crate::store::AppId::Claude);
        p.id = "p".into();
        p.name = "P".into();
        p.base_url = "https://x".into();
        p.api_key = "sk".into();
        p.model = Some("a".into());
        p.slots.insert("sonnet".into(), "b".into());
        p.slots.insert("opus".into(), "b".into());
        p.catalog = vec![
            ModelEntry {
                id: "a".into(),
                ..ModelEntry::default()
            },
            ModelEntry {
                id: "b".into(),
                ..ModelEntry::default()
            },
        ];
        let editor = CatalogEditor::from_provider(crate::adapter::models::CLAUDE_FIELDS, &p);
        app.overlay = Overlay::CatalogEditor { editor };

        // Sanity: two slot bindings seeded, row 0 focused.
        match &app.overlay {
            Overlay::CatalogEditor { editor, .. } => {
                assert_eq!(editor.slot_owner.len(), 2);
                assert_eq!(editor.row, 0);
            }
            _ => unreachable!(),
        }

        // Move to row 1 (the slot-bearing row) and delete it.
        app.handle_catalog_key(key(KeyCode::Char('j')));
        app.handle_catalog_key(key(KeyCode::Char('d')));

        match &app.overlay {
            Overlay::CatalogEditor { editor, .. } => {
                assert_eq!(editor.rows.len(), 1, "row deleted");
                assert!(editor.slot_owner.is_empty(), "slots cleared");
            }
            _ => unreachable!(),
        }
        assert!(
            app.status.contains("2") && app.status.contains("slot"),
            "status should mention 2 slots, got: {}",
            app.status
        );
    }

    #[test]
    fn popover_unassign_does_not_spurious_status() {
        // Cancelling a slot binding in the popover (Enter on an
        // already-assigned slot) must NOT trigger any "row deleted" or
        // "default moved" status. Previously the app's Continue path
        // compared slot_owner.len() before/after and mis-attributed the
        // popover's remove to a row delete.
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);

        use crate::store::ModelEntry;
        use crate::tui::pages::models::CatalogEditor;
        let mut p = crate::store::Provider::blank(crate::store::AppId::Claude);
        p.id = "p".into();
        p.name = "P".into();
        p.base_url = "https://x".into();
        p.api_key = "sk".into();
        p.model = Some("a".into());
        p.slots.insert("sonnet".into(), "b".into());
        p.catalog = vec![
            ModelEntry {
                id: "a".into(),
                ..ModelEntry::default()
            },
            ModelEntry {
                id: "b".into(),
                ..ModelEntry::default()
            },
        ];
        app.overlay = Overlay::CatalogEditor {
            editor: CatalogEditor::from_provider(crate::adapter::models::CLAUDE_FIELDS, &p),
        };

        // Baseline: no spurious status before any keys.
        assert!(app.status.is_empty() || app.status == "Status");

        // Move to row 1 (the slot-bearing row), then open the popover
        // and re-toggle sonnet. The popover must NOT surface a status
        // message — only `delete_row` is allowed to set pending fields.
        app.handle_catalog_key(key(KeyCode::Char('j'))); // row 0 -> row 1
        {
            let Overlay::CatalogEditor { editor, .. } = &mut app.overlay else {
                unreachable!()
            };
            let col = editor
                .fields
                .iter()
                .position(|f| matches!(f, crate::adapter::models::CatalogField::Slots))
                .unwrap();
            editor.col = col;
        }
        app.handle_catalog_key(key(KeyCode::Char(' '))); // open popover on row 1
        app.handle_catalog_key(key(KeyCode::Char('j'))); // cursor 0 -> 1 (sonnet)
        app.handle_catalog_key(key(KeyCode::Char(' '))); // toggle sonnet off
        app.handle_catalog_key(key(KeyCode::Enter)); // close popover

        match &app.overlay {
            Overlay::CatalogEditor { editor, .. } => {
                assert!(
                    !editor.slot_owner.contains_key("sonnet"),
                    "popover unassign fired"
                );
                assert_eq!(editor.rows.len(), 2, "no row was deleted");
            }
            _ => unreachable!(),
        }
        // The bug: app previously fired "已删行：清理 1 个 slot 绑定"
        // here. With the explicit pending_dropped_slots field, the
        // status bar must NOT have been touched.
        assert!(
            !app.status.contains("已删行")
                && !app.status.contains("Row deleted")
                && !app.status.contains("Default"),
            "popover unassign must not surface row-deleted status; got: {}",
            app.status
        );
    }

    #[test]
    fn deleting_default_row_surfaces_default_moved() {
        // Deleting the Default row should set status to "Default moved
        // to <id>" so the user notices provider.model was reassigned.
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);

        use crate::store::ModelEntry;
        use crate::tui::pages::models::CatalogEditor;
        let mut p = crate::store::Provider::blank(crate::store::AppId::Claude);
        p.id = "p".into();
        p.name = "P".into();
        p.base_url = "https://x".into();
        p.api_key = "sk".into();
        p.model = Some("a".into());
        p.catalog = vec![
            ModelEntry {
                id: "a".into(),
                ..ModelEntry::default()
            },
            ModelEntry {
                id: "b".into(),
                ..ModelEntry::default()
            },
        ];
        app.overlay = Overlay::CatalogEditor {
            editor: CatalogEditor::from_provider(crate::adapter::models::CLAUDE_FIELDS, &p),
        };

        // Default is row 0 (a). Press d to delete.
        app.handle_catalog_key(key(KeyCode::Char('d')));
        match &app.overlay {
            Overlay::CatalogEditor { editor, .. } => {
                assert_eq!(editor.rows.len(), 1);
                assert_eq!(editor.default_idx, 0, "b takes the deleted row's index");
            }
            _ => unreachable!(),
        }
        assert!(
            app.status.contains("b")
                && (app.status.contains("Default") || app.status.contains("默认")),
            "status should mention new default 'b'; got: {}",
            app.status
        );
    }

    #[test]
    fn sync_push_and_pull_open_confirm_overlay() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let srv = crate::webdav::mock::MockServer::start();
        let url = srv.collection_url("/dav");
        // Stage a configured WebDAV so open_sync_confirm has credentials to read.
        let mut app = sample(paths.clone());
        app.handle_action(Action::OpenData);
        app.handle_action(Action::SyncSetup);
        {
            let Overlay::Form(form) = &mut app.overlay else {
                panic!("setup form");
            };
            for f in &mut form.fields {
                match f.key {
                    "url" => f.value = url.clone(),
                    "username" => f.value = "u".into(),
                    "password" => {
                        f.secret_keep = false;
                        f.value = "p".into();
                    }
                    _ => {}
                }
            }
        }
        app.submit_form();
        assert!(app.is_syncing());
        let start = std::time::Instant::now();
        while app.is_syncing() && start.elapsed() < std::time::Duration::from_secs(8) {
            if app.poll_sync() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!app.is_syncing(), "{}", app.status);
        assert!(cloud::local_sync(&paths).is_some());

        // p → ConfirmSync (Push) overlay
        app.handle_action(Action::SyncPush);
        assert!(
            matches!(
                app.overlay,
                Overlay::ConfirmSync {
                    kind: SyncKind::Push,
                    ..
                }
            ),
            "expected ConfirmSync Push, got {:?}",
            overlay_label(&app.overlay)
        );
        // Render so the popup text is exercised; "never" must show when last_sync_at is empty
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        term.draw(|f| view::draw(f, &mut app)).unwrap();
        let txt = buffer_text(&term);
        assert!(
            txt.contains("Push") || txt.contains("推送"),
            "popup should show push title; got: {txt}"
        );
        assert!(
            txt.contains("never") || txt.contains("从未"),
            "popup should show 'never' when no prior sync; got: {txt}"
        );
        // Esc cancels
        app.handle_confirm_key(key(KeyCode::Esc));
        assert!(matches!(app.overlay, Overlay::None));

        // u → ConfirmSync (Pull)
        app.handle_action(Action::SyncPull);
        assert!(
            matches!(
                app.overlay,
                Overlay::ConfirmSync {
                    kind: SyncKind::Pull,
                    ..
                }
            ),
            "expected ConfirmSync Pull, got {:?}",
            overlay_label(&app.overlay)
        );
        term.draw(|f| view::draw(f, &mut app)).unwrap();
        let txt = buffer_text(&term);
        assert!(
            txt.contains("Pull") || txt.contains("拉取"),
            "popup should show pull title; got: {txt}"
        );
        // y triggers the actual job (overlay → Syncing)
        app.handle_confirm_key(key(KeyCode::Char('y')));
        assert!(
            matches!(app.overlay, Overlay::Syncing),
            "expected Syncing after confirm, got {:?}",
            overlay_label(&app.overlay)
        );
    }

    #[test]
    fn sync_action_without_config_does_not_open_confirm() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        app.handle_action(Action::OpenData);
        // No webdav.json yet — should set status and not open ConfirmSync.
        app.handle_action(Action::SyncPush);
        assert!(
            !matches!(app.overlay, Overlay::ConfirmSync { .. }),
            "must not show confirm when sync is unconfigured: {:?}",
            overlay_label(&app.overlay)
        );
        assert!(!app.status.is_empty());
    }

    #[test]
    fn gist_tab_switches_panel_and_routing() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        app.handle_action(Action::OpenData);
        assert_eq!(app.sync_tab, SyncTab::Webdav);

        // Tab flips the panel between WebDAV and Gist.
        app.handle_action(Action::SyncTab);
        assert_eq!(app.sync_tab, SyncTab::Gist);
        app.handle_action(Action::SyncTab);
        assert_eq!(app.sync_tab, SyncTab::Webdav);

        // On the Gist tab, e opens the Gist setup form.
        app.handle_action(Action::SyncTab);
        app.handle_action(Action::SyncSetup);
        let Overlay::Form(form) = &app.overlay else {
            panic!(
                "expected gist setup form, got {:?}",
                overlay_label(&app.overlay)
            );
        };
        assert_eq!(form.kind, form::FormKind::GistSetup);
        assert!(form.fields.iter().any(|f| f.key == "token"));
        assert!(form.fields.iter().any(|f| f.key == "gist"));
        app.handle_key(key(KeyCode::Esc)); // cancel

        // p/u on the Gist tab refuse to run while unconfigured.
        app.handle_action(Action::SyncPush);
        assert!(
            !matches!(app.overlay, Overlay::ConfirmSync { .. } | Overlay::Syncing),
            "gist push must be refused when unconfigured: {:?}",
            overlay_label(&app.overlay)
        );
        app.handle_action(Action::SyncPull);
        assert!(
            !matches!(app.overlay, Overlay::ConfirmSync { .. } | Overlay::Syncing),
            "gist pull must be refused when unconfigured: {:?}",
            overlay_label(&app.overlay)
        );

        // Tab back to WebDAV: e opens the WebDAV setup form again.
        app.handle_action(Action::SyncTab);
        app.handle_action(Action::SyncSetup);
        let Overlay::Form(form) = &app.overlay else {
            panic!(
                "expected webdav setup form, got {:?}",
                overlay_label(&app.overlay)
            );
        };
        assert_eq!(form.kind, form::FormKind::SyncSetup);
    }

    #[test]
    fn gist_setup_form_submits_token_and_gist() {
        let td = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(td.path());
        let mut app = sample(paths);
        app.handle_action(Action::OpenData);
        app.handle_action(Action::SyncTab);
        app.handle_action(Action::SyncSetup);
        {
            let Overlay::Form(form) = &mut app.overlay else {
                panic!("gist setup form");
            };
            let token = form.fields.iter_mut().find(|f| f.key == "token").unwrap();
            token.secret_keep = false;
            token.value = "ghp_t".into();
            let gist = form.fields.iter_mut().find(|f| f.key == "gist").unwrap();
            gist.value = "https://gist.github.com/owner/abc123def456abc123de".into();
        }
        // Submitting spawns a network job against the real API; assert only
        // that the form hands off to the Syncing overlay with the right kind.
        app.submit_form();
        assert!(
            matches!(app.overlay, Overlay::Syncing),
            "expected Syncing after gist setup submit, got {:?}",
            overlay_label(&app.overlay)
        );
        // Let the job finish in the background (it will fail against the
        // network-less endpoint or succeed creating nothing — either way the
        // TUI must settle: a status flash, or the setup form restored with
        // the error shown inside it).
        let start = std::time::Instant::now();
        while app.is_syncing() && start.elapsed() < std::time::Duration::from_secs(8) {
            if app.poll_sync() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!app.is_syncing(), "job must settle: {}", app.status);
        let settled = !app.status.is_empty()
            || matches!(
                &app.overlay,
                Overlay::Form(f) if f.error.is_some() && f.kind == form::FormKind::GistSetup
            );
        assert!(
            settled,
            "expected status or form error, got {:?} / {:?}",
            overlay_label(&app.overlay),
            app.status
        );
    }

    fn overlay_label(overlay: &Overlay) -> String {
        match overlay {
            Overlay::None => "None".into(),
            Overlay::ConfirmDelete { id, name } => format!("ConfirmDelete({id},{name})"),
            Overlay::ConfirmRestore { name } => format!("ConfirmRestore({name})"),
            Overlay::ConfirmSync { kind, .. } => format!("ConfirmSync({:?})", kind),
            Overlay::Form(_) => "Form".into(),
            Overlay::Syncing => "Syncing".into(),
            Overlay::FetchingModels => "FetchingModels".into(),
            Overlay::ModelPicker(_) => "ModelPicker".into(),
            Overlay::CatalogEditor { .. } => "CatalogEditor".into(),
            Overlay::SlotEditor { .. } => "SlotEditor".into(),
            Overlay::SnippetEditor(_) => "SnippetEditor".into(),
        }
    }
}
