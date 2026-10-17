//! Model picker, catalog editor, slot table, and snippet editor overlays.

use std::collections::BTreeSet;
use std::sync::mpsc::{self, Receiver};

use crossterm::event::{KeyCode, KeyEvent};

use crate::adapter::models::{self, CatalogField, ModelUi};
use crate::i18n::t;
use crate::store::{AppId, ModelEntry};
use crate::tui::edit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Catalog,
    Slot { key: &'static str },
}

pub struct ModelPicker {
    pub kind: PickerKind,
    pub ids: Vec<String>,
    pub selected: BTreeSet<usize>,
    /// Known catalog metadata by id, carried through fetch so confirming
    /// keeps label/context_window/max_tokens of previously saved rows.
    pub meta: std::collections::BTreeMap<String, ModelEntry>,
    pub cursor: usize,
    pub filter: String,
    pub filter_cursor: usize,
    pub filtering: bool,
    pub error: Option<String>,
    /// First visible row in `visible()`, updated while drawing.
    pub scroll: usize,
    /// Rows that fit in the list viewport. 0 until the first draw.
    pub page_rows: usize,
}

impl ModelPicker {
    /// Pre-check catalog ids already chosen; for slots, park the cursor on
    /// the first id that equals the slot's current value.
    pub fn with_preselect(kind: PickerKind, ids: Vec<String>, pre: &[String]) -> Self {
        let mut p = Self {
            kind,
            ids,
            selected: BTreeSet::new(),
            meta: Default::default(),
            cursor: 0,
            filter: String::new(),
            filter_cursor: 0,
            filtering: false,
            error: None,
            scroll: 0,
            page_rows: 0,
        };
        let mut placed_cursor = false;
        for (i, id) in p.ids.iter().enumerate() {
            if !pre.iter().any(|s| s == id) {
                continue;
            }
            match p.kind {
                PickerKind::Catalog => {
                    p.selected.insert(i);
                }
                PickerKind::Slot { .. } if !placed_cursor => {
                    p.cursor = i;
                    placed_cursor = true;
                }
                _ => {}
            }
        }
        p
    }

    /// Remember catalog metadata (from the held form) so confirming a
    /// fetched list keeps label/context_window/max_tokens of saved rows.
    pub fn with_meta(mut self, entries: &[ModelEntry]) -> Self {
        self.meta = entries
            .iter()
            .filter(|e| !e.id.trim().is_empty())
            .map(|e| (e.id.clone(), e.clone()))
            .collect();
        self
    }

    pub fn visible(&self) -> Vec<usize> {
        self.ids
            .iter()
            .enumerate()
            .filter(|(_, id)| {
                self.filter.is_empty()
                    || id
                        .to_ascii_lowercase()
                        .contains(&self.filter.to_ascii_lowercase())
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Paste into the filter input while it is focused (bracketed paste).
    pub fn paste_filter(&mut self, text: &str) {
        if self.filtering {
            edit::paste(&mut self.filter, &mut self.filter_cursor, text);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PickerCmd {
        if self.filtering {
            return self.handle_filter_key(key);
        }
        match key.code {
            KeyCode::Esc => PickerCmd::Cancel,
            KeyCode::Char('/') => {
                self.filtering = true;
                self.filter_cursor = edit::len(&self.filter);
                PickerCmd::Continue
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_vis(1);
                PickerCmd::Continue
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_vis(-1);
                PickerCmd::Continue
            }
            KeyCode::PageDown => {
                self.page(1);
                PickerCmd::Continue
            }
            KeyCode::PageUp => {
                self.page(-1);
                PickerCmd::Continue
            }
            KeyCode::Home => {
                self.jump_vis(0);
                PickerCmd::Continue
            }
            KeyCode::End => {
                self.jump_vis(isize::MAX);
                PickerCmd::Continue
            }
            KeyCode::Char(' ') if matches!(self.kind, PickerKind::Catalog) => {
                self.toggle_cursor();
                PickerCmd::Continue
            }
            KeyCode::Enter => match self.kind {
                PickerKind::Catalog => PickerCmd::ConfirmCatalog(
                    self.selected
                        .iter()
                        .filter_map(|i| self.ids.get(*i).cloned())
                        .map(|id| {
                            let mut entry = self.meta.get(&id).cloned().unwrap_or_default();
                            entry.id = id;
                            entry
                        })
                        .collect(),
                ),
                PickerKind::Slot { key } => {
                    let id = self.ids.get(self.cursor).cloned().unwrap_or_default();
                    PickerCmd::ConfirmSlot { key, id }
                }
            },
            _ => PickerCmd::Continue,
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) -> PickerCmd {
        // Space picks the highlighted catalog row straight out of the filter,
        // so `/gpt` + Space is the whole "narrow down, take it" gesture. The
        // typed filter stays on screen. Slot pickers take a literal space:
        // their ids are free text.
        if matches!(key.code, KeyCode::Char(' ')) && matches!(self.kind, PickerKind::Catalog) {
            self.filtering = false;
            if self.visible().contains(&self.cursor) {
                self.toggle_cursor();
            }
            return PickerCmd::Continue;
        }
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            self.filtering = false;
            return PickerCmd::Continue;
        }
        if edit::key(&mut self.filter, &mut self.filter_cursor, key) {
            self.after_filter();
        } else {
            match key.code {
                KeyCode::Up => self.move_vis(-1),
                KeyCode::Down => self.move_vis(1),
                KeyCode::PageUp => self.page(-1),
                KeyCode::PageDown => self.page(1),
                _ => {}
            }
        }
        PickerCmd::Continue
    }

    /// Flip the catalog checkbox on the highlighted row.
    fn toggle_cursor(&mut self) {
        if self.selected.contains(&self.cursor) {
            self.selected.remove(&self.cursor);
        } else {
            self.selected.insert(self.cursor);
        }
    }

    fn after_filter(&mut self) {
        let vis = self.visible();
        if vis.is_empty() {
            self.scroll = 0;
            return;
        }
        if !vis.contains(&self.cursor) {
            self.cursor = vis[0];
        }
        self.ensure_visible();
    }

    fn move_vis(&mut self, delta: isize) {
        let vis = self.visible();
        if vis.is_empty() {
            return;
        }
        let pos = vis.iter().position(|i| *i == self.cursor).unwrap_or(0);
        let n = vis.len() as isize;
        let next = vis[((pos as isize + delta).rem_euclid(n)) as usize];
        self.cursor = next;
        self.ensure_visible();
    }

    fn page(&mut self, dir: isize) {
        let vis = self.visible();
        if vis.is_empty() {
            return;
        }
        let pos = vis.iter().position(|i| *i == self.cursor).unwrap_or(0) as isize;
        let n = vis.len() as isize;
        // 0 until the first draw; then jump by the real viewport (no wrap).
        let rows = if self.page_rows == 0 {
            10
        } else {
            self.page_rows
        };
        let step = (rows.min(n.max(1) as usize) as isize) * dir;
        let next = (pos + step).clamp(0, n - 1) as usize;
        self.cursor = vis[next];
        self.ensure_visible();
    }

    fn jump_vis(&mut self, pos: isize) {
        let vis = self.visible();
        if vis.is_empty() {
            return;
        }
        let i = if pos <= 0 {
            0
        } else {
            vis.len().saturating_sub(1)
        };
        self.cursor = vis[i];
        self.ensure_visible();
    }

    pub fn ensure_visible(&mut self) {
        let vis = self.visible();
        if vis.is_empty() {
            self.scroll = 0;
            return;
        }
        let pos = vis.iter().position(|&i| i == self.cursor).unwrap_or(0);
        let page = self.page_rows.max(1);
        if pos < self.scroll {
            self.scroll = pos;
        } else if pos >= self.scroll.saturating_add(page) {
            self.scroll = pos + 1 - page;
        }
        let max_scroll = vis.len().saturating_sub(page.min(vis.len()));
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }
    }
}

#[derive(Debug)]
pub enum PickerCmd {
    Continue,
    Cancel,
    ConfirmCatalog(Vec<ModelEntry>),
    ConfirmSlot { key: &'static str, id: String },
}

/// Parse a token-count field. Empty → None (unset). Values < `min` are
/// treated as garbage (typically placeholder text) and rejected so they
/// don't pollute aggregate computations like the catalog-wide `min()` over
/// `context_window` that drives `CLAUDE_CODE_MAX_CONTEXT_TOKENS`.
fn parse_window(raw: &str, min: u64) -> Option<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let n: u64 = trimmed.parse().ok()?;
    if n < min {
        None
    } else {
        Some(n)
    }
}

pub struct CatalogEditor {
    pub fields: &'static [CatalogField],
    pub rows: Vec<ModelEntry>,
    pub default_idx: usize,
    pub row: usize,
    pub col: usize,
    pub editing: bool,
    pub buf: String,
    pub buf_cursor: usize,
    /// Slot assignment (haiku/sonnet/opus/fable/subagent) per row, by id.
    /// A row is "assigned" to a slot iff `slot_owner[slot] == Some(row.id)`.
    /// Maintained alongside `rows` so the catalog is the SSOT for ids/metadata
    /// and `slot_owner` is the SSOT for slot-to-id binding.
    pub slot_owner: std::collections::BTreeMap<&'static str, String>,
    /// Popover state for the slot-assignment editor (open on a non-default row).
    pub slot_picker: Option<SlotPickerState>,
    /// Popover state for the target-model-id picker (lists
    /// `KNOWN_CLAUDE_MODEL_IDS`; first item is "(none)" to clear).
    pub target_picker: Option<TargetPickerState>,
    /// Number of slot bindings cleared by the most recent `delete_row`.
    /// `app` consumes it after each `Continue` and the editor zeroes it;
    /// other code paths (popover toggles, picker changes) leave it at 0
    /// so the app's "row deleted" status only fires for actual row deletes.
    pub pending_dropped_slots: usize,
    /// If the most recent `delete_row` removed the Default row, the new
    /// default row's id (or `None` if the catalog is now empty). `app`
    /// consumes this to surface a "Default moved to …" message.
    pub deleted_default_to: Option<Option<String>>,
}

#[derive(Debug, Clone)]
pub struct SlotPickerState {
    pub row: usize,
    pub cursor: usize,
}

#[derive(Debug, Clone)]
pub struct TargetPickerState {
    pub row: usize,
    pub cursor: usize,
    /// Slot marked via space (like the catalog ModelPicker's checkboxes);
    /// Enter commits the mark instead of wherever the cursor idles.
    pub mark: Option<usize>,
}

impl CatalogEditor {
    pub fn new(
        fields: &'static [CatalogField],
        rows: Vec<ModelEntry>,
        default_id: Option<&str>,
    ) -> Self {
        let default_idx = default_id
            .and_then(|id| rows.iter().position(|r| r.id == id))
            .unwrap_or(0);
        Self {
            fields,
            rows,
            default_idx,
            row: 0,
            col: 0,
            editing: false,
            buf: String::new(),
            buf_cursor: 0,
            slot_owner: std::collections::BTreeMap::new(),
            slot_picker: None,
            target_picker: None,
            pending_dropped_slots: 0,
            deleted_default_to: None,
        }
    }

    /// Build a `CatalogEditor` seeded from an existing `Provider`: the
    /// catalog rows, the Default id, and the slot → id bindings.
    pub fn from_provider(
        fields: &'static [CatalogField],
        provider: &crate::store::Provider,
    ) -> Self {
        let mut ed = Self::new(fields, provider.catalog.clone(), provider.model.as_deref());
        for slot in models::CLAUDE_SLOTS {
            if let Some(id) = provider
                .slots
                .get(slot.key)
                .map(String::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                ed.slot_owner.insert(slot.key, id.to_string());
            }
        }
        ed
    }

    /// Which row currently owns `slot`? Returns the row index, if any.
    #[allow(dead_code)]
    pub fn slot_row(&self, slot: &str) -> Option<usize> {
        let id = self.slot_owner.get(slot)?;
        self.rows.iter().position(|r| r.id == *id)
    }

    /// Paste into the cell being edited (bracketed paste).
    pub fn paste(&mut self, text: &str) {
        if self.editing {
            edit::paste(&mut self.buf, &mut self.buf_cursor, text);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> CatalogCmd {
        if self.slot_picker.is_some() {
            return self.handle_slot_picker_key(key);
        }
        if self.target_picker.is_some() {
            return self.handle_target_picker_key(key);
        }
        if self.editing {
            match key.code {
                KeyCode::Esc => {
                    self.editing = false;
                }
                KeyCode::Enter => self.commit_edit(),
                _ => {
                    edit::key(&mut self.buf, &mut self.buf_cursor, key);
                }
            }
            return CatalogCmd::Continue;
        }
        match key.code {
            KeyCode::Esc => CatalogCmd::Cancel,
            KeyCode::Enter if key.modifiers.is_empty() => CatalogCmd::Save,
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.rows.is_empty() {
                    self.row = (self.row + 1) % self.rows.len();
                }
                CatalogCmd::Continue
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if !self.rows.is_empty() {
                    self.row = (self.row + self.rows.len() - 1) % self.rows.len();
                }
                CatalogCmd::Continue
            }
            KeyCode::Tab | KeyCode::Right => {
                if !self.fields.is_empty() {
                    self.col = (self.col + 1) % self.fields.len();
                }
                CatalogCmd::Continue
            }
            KeyCode::BackTab | KeyCode::Left => {
                if !self.fields.is_empty() {
                    self.col = (self.col + self.fields.len() - 1) % self.fields.len();
                }
                CatalogCmd::Continue
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                self.begin_edit();
                CatalogCmd::Continue
            }
            KeyCode::Char(' ') => self.handle_space(),
            KeyCode::Char('d') | KeyCode::Char('D') => {
                self.delete_row();
                CatalogCmd::Continue
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.rows.push(ModelEntry::default());
                self.row = self.rows.len() - 1;
                CatalogCmd::Continue
            }
            KeyCode::Char('*') => {
                if self.row < self.rows.len() {
                    self.default_idx = self.row;
                }
                CatalogCmd::Continue
            }
            _ => CatalogCmd::Continue,
        }
    }

    fn handle_space(&mut self) -> CatalogCmd {
        let Some(field) = self.fields.get(self.col).copied() else {
            return CatalogCmd::Continue;
        };
        match field {
            CatalogField::TargetModelId => {
                // Open the target-model-id picker. Cursor parks on the
                // current value if any, else the first known id.
                if self.row < self.rows.len() {
                    let cursor = self
                        .rows
                        .get(self.row)
                        .and_then(|r| r.target_model_id.as_deref())
                        .and_then(|tid| {
                            models::KNOWN_CLAUDE_MODEL_IDS
                                .iter()
                                .position(|k| *k == tid)
                        })
                        .map(|i| i + 1) // +1 because index 0 is "(none)"
                        .unwrap_or(0);
                    self.target_picker = Some(TargetPickerState {
                        row: self.row,
                        cursor,
                        mark: None,
                    });
                }
                CatalogCmd::Continue
            }
            CatalogField::Slots => {
                if self.row < self.rows.len() {
                    self.slot_picker = Some(SlotPickerState {
                        row: self.row,
                        cursor: 0,
                    });
                }
                CatalogCmd::Continue
            }
            _ => CatalogCmd::Continue,
        }
    }

    fn handle_target_picker_key(&mut self, key: KeyEvent) -> CatalogCmd {
        // Item 0 is "(none)"; items 1..=N are KNOWN_CLAUDE_MODEL_IDS.
        // Space toggles a non-binding single mark (stays open); Enter commits
        // the marked id (or the cursor, when nothing is marked) and closes.
        let total = models::KNOWN_CLAUDE_MODEL_IDS.len() + 1;
        match key.code {
            KeyCode::Esc => {
                self.target_picker = None;
            }
            KeyCode::Char(' ') => {
                if let Some(p) = self.target_picker.as_mut() {
                    p.mark = Some(p.cursor);
                }
            }
            KeyCode::Enter => {
                let Some(picker) = self.target_picker.as_ref() else {
                    return CatalogCmd::Continue;
                };
                let picked = picker.mark.unwrap_or(picker.cursor);
                let row_idx = picker.row;
                let chosen: Option<String> = if picked == 0 {
                    None
                } else {
                    models::KNOWN_CLAUDE_MODEL_IDS
                        .get(picked - 1)
                        .map(|s| s.to_string())
                };
                if let Some(row) = self.rows.get_mut(row_idx) {
                    row.target_model_id = chosen;
                }
                self.target_picker = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(p) = self.target_picker.as_mut() {
                    p.cursor = (p.cursor + 1) % total;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(p) = self.target_picker.as_mut() {
                    p.cursor = (p.cursor + total - 1) % total;
                }
            }
            _ => {}
        }
        CatalogCmd::Continue
    }

    fn handle_slot_picker_key(&mut self, key: KeyEvent) -> CatalogCmd {
        let slots = models::CLAUDE_SLOTS;
        // `picker` is the current state — re-borrow every time we mutate it
        // to avoid overlapping mutable borrows.
        match key.code {
            KeyCode::Esc => {
                self.slot_picker = None;
            }
            KeyCode::Char(' ') => {
                // Toggle the slot under the cursor but keep the popover
                // open so several slots can be assigned in one visit.
                let Some(picker) = self.slot_picker.as_ref() else {
                    return CatalogCmd::Continue;
                };
                let cursor = picker.cursor;
                let row_idx = picker.row;
                let Some(slot) = slots.get(cursor) else {
                    return CatalogCmd::Continue;
                };
                let row_id = self
                    .rows
                    .get(row_idx)
                    .map(|r| r.id.clone())
                    .unwrap_or_default();
                if row_id.is_empty() {
                    return CatalogCmd::Continue;
                }
                if self.slot_owner.get(slot.key).map(String::as_str) == Some(&row_id) {
                    self.slot_owner.remove(slot.key);
                } else {
                    // "搬家": steal the slot from any other row.
                    self.slot_owner.insert(slot.key, row_id);
                }
            }
            KeyCode::Enter => {
                // Enter confirms the current toggles and closes the popover.
                self.slot_picker = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(p) = self.slot_picker.as_mut() {
                    p.cursor = (p.cursor + 1) % slots.len();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(p) = self.slot_picker.as_mut() {
                    p.cursor = (p.cursor + slots.len() - 1) % slots.len();
                }
            }
            _ => {}
        }
        CatalogCmd::Continue
    }

    fn delete_row(&mut self) {
        if self.row >= self.rows.len() {
            return;
        }
        let was_default = self.row == self.default_idx;
        let id = self.rows[self.row].id.clone();
        self.rows.remove(self.row);
        // Drop any slot bindings that pointed at the deleted row, and
        // report exactly how many the row owned (before vs. after the
        // retain) so the app's status bar reflects the deleted row's
        // contribution rather than a coincidental length delta.
        let before = self.slot_owner.len();
        self.slot_owner.retain(|_, v| *v != id);
        self.pending_dropped_slots = before - self.slot_owner.len();
        if was_default {
            if self.rows.is_empty() {
                self.default_idx = 0;
                self.deleted_default_to = Some(None);
            } else {
                // default_idx is the deleted row's index, which now points
                // to the row that took its place (Vec::remove shifts left).
                if self.default_idx >= self.rows.len() {
                    self.default_idx = self.rows.len() - 1;
                }
                let new_default = self
                    .rows
                    .get(self.default_idx)
                    .map(|r| r.id.clone())
                    .filter(|s| !s.trim().is_empty());
                self.deleted_default_to = Some(new_default);
            }
        } else if self.default_idx >= self.rows.len() && !self.rows.is_empty() {
            self.default_idx = self.rows.len() - 1;
        }
        self.row = self.row.min(self.rows.len().saturating_sub(1));
    }

    fn begin_edit(&mut self) {
        let Some(row) = self.rows.get(self.row) else {
            return;
        };
        let Some(field) = self.fields.get(self.col) else {
            return;
        };
        match field {
            CatalogField::Slots | CatalogField::TargetModelId => {
                // Non-text fields use space/enter, not inline editing.
                let _ = self.handle_space();
                return;
            }
            _ => {}
        }
        self.buf_cursor = 0;
        self.buf = match field {
            CatalogField::Id => row.id.clone(),
            CatalogField::Label => row.label.clone().unwrap_or_default(),
            CatalogField::ContextWindow => row
                .context_window
                .map(|n| n.to_string())
                .unwrap_or_default(),
            CatalogField::MaxTokens => row.max_tokens.map(|n| n.to_string()).unwrap_or_default(),
            CatalogField::Slots | CatalogField::TargetModelId => String::new(),
        };
        self.buf_cursor = edit::len(&self.buf);
        self.editing = true;
    }

    fn commit_edit(&mut self) {
        self.editing = false;
        let Some(row) = self.rows.get_mut(self.row) else {
            return;
        };
        let Some(field) = self.fields.get(self.col) else {
            return;
        };
        let v = self.buf.trim();
        match field {
            CatalogField::Id => row.id = v.to_string(),
            CatalogField::Label => {
                row.label = if v.is_empty() {
                    None
                } else {
                    Some(v.to_string())
                };
            }
            CatalogField::ContextWindow => {
                // Reject sub-1000 values so a stray "1" placeholder can't
                // pollute the `min()` over the whole catalog (§8 risk #1).
                // Empty input is "unset" → None.
                row.context_window = parse_window(v, 1_000);
            }
            CatalogField::MaxTokens => {
                row.max_tokens = parse_window(v, 1_000);
            }
            CatalogField::Slots | CatalogField::TargetModelId => {}
        }
    }

    pub fn default_id(&self) -> Option<String> {
        self.rows
            .get(self.default_idx)
            .map(|r| r.id.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                self.rows
                    .iter()
                    .map(|r| r.id.trim())
                    .find(|s| !s.is_empty())
                    .map(str::to_string)
            })
    }

    /// Build the `Provider.{model, slots}` projection from the current
    /// catalog state. Called on save.
    #[allow(dead_code)]
    pub fn project_to_provider(&self, p: &mut crate::store::Provider) {
        p.catalog = self.rows.clone();
        p.model = self.default_id();
        p.slots = std::collections::BTreeMap::new();
        for (slot, id) in &self.slot_owner {
            if let Some(r) = self.rows.iter().find(|r| &r.id == id) {
                if !r.id.trim().is_empty() {
                    p.slots.insert(slot.to_string(), id.clone());
                }
            }
        }
    }
}

pub enum CatalogCmd {
    Continue,
    Cancel,
    Save,
}

pub struct SlotEditor {
    pub slots: &'static [models::SlotSpec],
    pub values: std::collections::BTreeMap<String, String>,
    pub default_model: String,
    pub row: usize, // 0 = default, then slots
    pub editing: bool,
    pub buf: String,
    pub buf_cursor: usize,
}

impl SlotEditor {
    pub fn from_values(
        default_model: String,
        values: std::collections::BTreeMap<String, String>,
    ) -> Self {
        Self {
            slots: models::CLAUDE_SLOTS,
            values,
            default_model,
            row: 0,
            editing: false,
            buf: String::new(),
            buf_cursor: 0,
        }
    }

    pub fn row_count(&self) -> usize {
        1 + self.slots.len()
    }

    pub fn focused_slot_key(&self) -> Option<&'static str> {
        if self.row == 0 {
            None
        } else {
            self.slots.get(self.row - 1).map(|s| s.key)
        }
    }

    /// Paste into the slot value being edited (bracketed paste).
    pub fn paste(&mut self, text: &str) {
        if self.editing {
            edit::paste(&mut self.buf, &mut self.buf_cursor, text);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SlotCmd {
        if self.editing {
            match key.code {
                KeyCode::Esc => {
                    self.editing = false;
                }
                KeyCode::Enter => self.commit(),
                _ => {
                    edit::key(&mut self.buf, &mut self.buf_cursor, key);
                }
            }
            SlotCmd::Continue
        } else {
            match key.code {
                KeyCode::Esc => SlotCmd::Cancel,
                KeyCode::Enter => SlotCmd::Save,
                KeyCode::Char('j') | KeyCode::Down => {
                    self.row = (self.row + 1) % self.row_count();
                    SlotCmd::Continue
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.row = (self.row + self.row_count() - 1) % self.row_count();
                    SlotCmd::Continue
                }
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    self.buf = self.current_value().to_string();
                    self.buf_cursor = edit::len(&self.buf);
                    self.editing = true;
                    SlotCmd::Continue
                }
                KeyCode::Char(' ') => SlotCmd::Fetch,
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    let id = self.current_value().to_string();
                    if !id.is_empty() {
                        self.default_model = id.clone();
                        for s in self.slots {
                            self.values.insert(s.key.to_string(), id.clone());
                        }
                    }
                    SlotCmd::Continue
                }
                _ => SlotCmd::Continue,
            }
        }
    }

    fn current_value(&self) -> &str {
        if self.row == 0 {
            &self.default_model
        } else if let Some(s) = self.slots.get(self.row - 1) {
            self.values.get(s.key).map(String::as_str).unwrap_or("")
        } else {
            ""
        }
    }

    fn commit(&mut self) {
        self.editing = false;
        let v = self.buf.trim().to_string();
        if self.row == 0 {
            self.default_model = v;
        } else if let Some(s) = self.slots.get(self.row - 1) {
            if v.is_empty() {
                self.values.remove(s.key);
            } else {
                self.values.insert(s.key.to_string(), v);
            }
        }
    }
}

pub enum SlotCmd {
    Continue,
    Cancel,
    Save,
    Fetch,
}

pub fn spawn_fetch(
    base_url: String,
    api_key: String,
    protocol: Option<String>,
) -> Receiver<Result<Vec<String>, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let res = models::fetch_models(&base_url, &api_key, protocol.as_deref())
            .map_err(|e| format!("{e:#}"));
        let _ = tx.send(res);
    });
    rx
}

pub fn model_ui_for(app: AppId) -> ModelUi {
    crate::adapter::get(app)
        .map(|a| a.model_ui())
        .unwrap_or(ModelUi::Catalog {
            fields: models::OPENCODE_FIELDS,
        })
}

pub fn field_label(field: CatalogField) -> &'static str {
    match field {
        CatalogField::Id => "id",
        CatalogField::Label => t("field.label"),
        CatalogField::ContextWindow => t("field.context_window"),
        CatalogField::MaxTokens => t("field.max_tokens"),
        CatalogField::Slots => t("field.slot_assignment"),
        CatalogField::TargetModelId => t("field.target_model_id"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Provider;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn parse_window_rejects_sub_threshold_and_garbage() {
        // Empty → None (unset).
        assert_eq!(parse_window("", 1_000), None);
        assert_eq!(parse_window("   ", 1_000), None);
        // Below the min → None (placeholder protection).
        assert_eq!(parse_window("1", 1_000), None);
        assert_eq!(parse_window("999", 1_000), None);
        // Non-numeric → None.
        assert_eq!(parse_window("abc", 1_000), None);
        // At or above the min → Some.
        assert_eq!(parse_window("1000", 1_000), Some(1_000));
        assert_eq!(parse_window("200000", 1_000), Some(200_000));
        // Trims whitespace.
        assert_eq!(parse_window("  50000  ", 1_000), Some(50_000));
    }

    #[test]
    fn context_window_field_rejects_sub_threshold() {
        // Typing "1" into the ContextWindow cell should NOT poison the row:
        // it stays None, so min() over the catalog is unaffected.
        let mut ed = claude_editor_with(
            vec![ModelEntry {
                id: "a".into(),
                ..ModelEntry::default()
            }],
            "a",
        );
        focus_field(&mut ed, crate::adapter::models::CatalogField::ContextWindow);
        ed.handle_key(key(KeyCode::Char('e')));
        for c in "1".chars() {
            ed.handle_key(key(KeyCode::Char(c)));
        }
        ed.handle_key(key(KeyCode::Enter));
        assert!(ed.rows[0].context_window.is_none());
    }

    #[test]
    fn catalog_picker_toggles_and_confirms() {
        let mut p =
            ModelPicker::with_preselect(PickerKind::Catalog, vec!["a".into(), "b".into()], &[]);
        p.handle_key(key(KeyCode::Char(' ')));
        p.handle_key(key(KeyCode::Char('j')));
        p.handle_key(key(KeyCode::Char(' ')));
        match p.handle_key(key(KeyCode::Enter)) {
            PickerCmd::ConfirmCatalog(rows) => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].id, "a");
                assert_eq!(rows[1].id, "b");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn slot_picker_assigns_one() {
        let mut p = ModelPicker::with_preselect(
            PickerKind::Slot { key: "haiku" },
            vec!["x".into(), "y".into()],
            &[],
        );
        p.handle_key(key(KeyCode::Char('j')));
        match p.handle_key(key(KeyCode::Enter)) {
            PickerCmd::ConfirmSlot { key, id } => {
                assert_eq!(key, "haiku");
                assert_eq!(id, "y");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn slot_copy_all() {
        let mut p = Provider::blank(AppId::Claude);
        p.model = Some("sonnet".into());
        let mut ed = SlotEditor::from_values(p.model.clone().unwrap_or_default(), p.slots.clone());
        ed.row = 0;
        ed.handle_key(key(KeyCode::Char('a')));
        assert_eq!(ed.default_model, "sonnet");
        assert_eq!(ed.values.get("haiku").map(String::as_str), Some("sonnet"));
        assert_eq!(ed.values.get("opus").map(String::as_str), Some("sonnet"));
        assert_eq!(
            ed.values.get("subagent").map(String::as_str),
            Some("sonnet")
        );
    }

    #[test]
    fn picker_preselects_held_ids() {
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut p = ModelPicker::with_preselect(
            PickerKind::Catalog,
            ids.clone(),
            &["c".into(), "gone".into()],
        );
        assert_eq!(p.selected, BTreeSet::from([2])); // unknown ids ignored
        match p.handle_key(key(KeyCode::Enter)) {
            PickerCmd::ConfirmCatalog(rows) => assert_eq!(rows[0].id, "c"),
            other => panic!("{other:?}"),
        }
        let s =
            ModelPicker::with_preselect(PickerKind::Slot { key: "opus" }, ids, &["b".to_string()]);
        assert_eq!(s.cursor, 1); // cursor parked on the slot's current id
    }

    #[test]
    fn picker_confirm_keeps_saved_catalog_metadata() {
        // Re-picking from a fetched list must not wipe context windows
        // (or labels) that were already saved on those ids.
        let saved = ModelEntry {
            id: "b".into(),
            label: Some("B".into()),
            context_window: Some(200_000),
            max_tokens: None,
            target_model_id: None,
        };
        let mut p = ModelPicker::with_preselect(
            PickerKind::Catalog,
            vec!["a".into(), "b".into()],
            &["b".into()],
        )
        .with_meta(&[saved]);
        match p.handle_key(key(KeyCode::Enter)) {
            PickerCmd::ConfirmCatalog(rows) => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].id, "b");
                assert_eq!(rows[0].context_window, Some(200_000));
                assert_eq!(rows[0].label.as_deref(), Some("B"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn picker_pages_without_wrap() {
        let ids: Vec<String> = (0..12).map(|i| format!("m{i:02}")).collect();
        let mut p = ModelPicker::with_preselect(PickerKind::Catalog, ids, &[]);
        p.page_rows = 5;
        p.handle_key(key(KeyCode::PageDown));
        assert_eq!(p.cursor, 5); // jumped by one page
        assert!(p.scroll > 0); // scroll follows the cursor
        p.handle_key(key(KeyCode::PageDown));
        assert_eq!(p.cursor, 10); // clamped near the end, no wrap
        p.handle_key(key(KeyCode::PageUp));
        assert_eq!(p.cursor, 5);
    }

    #[test]
    fn filter_caret_edits_middle() {
        let mut p = ModelPicker::with_preselect(
            PickerKind::Slot { key: "default" },
            vec!["a".into(), "b".into()],
            &[],
        );
        p.handle_key(key(KeyCode::Char('/')));
        assert!(p.filtering);
        for c in ['a', 'b', 'c'] {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.filter, "abc");
        p.handle_key(key(KeyCode::Left));
        p.handle_key(key(KeyCode::Left));
        assert_eq!(p.filter_cursor, 1);
        p.handle_key(key(KeyCode::Char('X')));
        assert_eq!(p.filter, "aXbc"); // inserted before 'b'
        assert!(p.visible().is_empty());
        p.handle_key(key(KeyCode::Backspace));
        assert_eq!(p.filter, "abc");
        assert_eq!(
            edit::caret_spans(&p.filter, p.filter_cursor, Default::default())
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<Vec<_>>()
                .join(""),
            "abc" // underline caret adds no character
        );
    }

    #[test]
    fn space_while_filtering_picks_the_highlighted_row() {
        let ids: Vec<String> = (0..5).map(|i| format!("m{i:02}")).collect();
        let mut p = ModelPicker::with_preselect(PickerKind::Catalog, ids, &[]);
        p.handle_key(key(KeyCode::Char('/')));
        p.handle_key(key(KeyCode::Char('3')));
        assert_eq!(p.visible(), vec![3]);
        assert!(p.filtering);
        assert!(matches!(
            p.handle_key(key(KeyCode::Char(' '))),
            PickerCmd::Continue
        ));
        assert!(!p.filtering); // back to plain navigation…
        assert_eq!(p.filter, "3"); // …with the typed filter still applied
        assert!(p.selected.contains(&3)); // and the row picked
        match p.handle_key(key(KeyCode::Enter)) {
            PickerCmd::ConfirmCatalog(rows) => {
                let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
                assert_eq!(ids, ["m03"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn space_without_a_matching_row_picks_nothing() {
        let mut p =
            ModelPicker::with_preselect(PickerKind::Catalog, vec!["a".into(), "b".into()], &[]);
        p.handle_key(key(KeyCode::Char('/')));
        p.handle_key(key(KeyCode::Char('z')));
        assert!(p.visible().is_empty());
        p.handle_key(key(KeyCode::Char(' ')));
        assert!(p.selected.is_empty());
    }

    #[test]
    fn space_while_filtering_is_literal_in_a_slot_picker() {
        let mut p =
            ModelPicker::with_preselect(PickerKind::Slot { key: "default" }, vec!["a".into()], &[]);
        p.handle_key(key(KeyCode::Char('/')));
        p.handle_key(key(KeyCode::Char(' ')));
        assert!(p.filtering);
        assert_eq!(p.filter, " ");
    }

    #[test]
    fn catalog_edit_and_default() {
        let mut ed = CatalogEditor::new(
            crate::adapter::models::OPENCODE_FIELDS,
            vec![ModelEntry {
                id: "gpt-4o".into(),
                ..ModelEntry::default()
            }],
            Some("gpt-4o"),
        );
        ed.handle_key(key(KeyCode::Char('n')));
        assert_eq!(ed.rows.len(), 2);
        ed.handle_key(key(KeyCode::Char('*')));
        assert_eq!(ed.default_idx, 1);
        ed.handle_key(key(KeyCode::Char('d')));
        assert_eq!(ed.rows.len(), 1);
        assert_eq!(ed.default_id().as_deref(), Some("gpt-4o"));
    }

    fn claude_editor_with(rows: Vec<ModelEntry>, default: &str) -> CatalogEditor {
        let mut p = Provider::blank(AppId::Claude);
        p.id = "p".into();
        p.name = "P".into();
        p.base_url = "https://x".into();
        p.api_key = "sk".into();
        p.model = Some(default.into());
        p.catalog = rows;
        CatalogEditor::from_provider(crate::adapter::models::CLAUDE_FIELDS, &p)
    }

    fn focus_field(ed: &mut CatalogEditor, field: CatalogField) {
        let col = ed.fields.iter().position(|f| *f == field).unwrap();
        ed.col = col;
    }

    #[test]
    fn slot_picker_space_multi_select_keeps_popover_open() {
        // space toggles a slot without closing the popover so several
        // slots can be assigned in one visit; Enter only closes.
        let mut ed = claude_editor_with(
            vec![ModelEntry {
                id: "a".into(),
                ..ModelEntry::default()
            }],
            "a",
        );
        ed.row = 0;
        focus_field(&mut ed, CatalogField::Slots);
        ed.handle_key(key(KeyCode::Char(' ')));
        assert!(ed.slot_picker.is_some());

        // toggle haiku (cursor 0) on — popover must stay open
        ed.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(ed.slot_owner.get("haiku").map(String::as_str), Some("a"));
        assert!(ed.slot_picker.is_some());

        // move to sonnet (1) and toggle it on as well
        ed.handle_key(key(KeyCode::Char('j')));
        ed.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(ed.slot_owner.get("sonnet").map(String::as_str), Some("a"));
        assert!(ed.slot_picker.is_some());

        // toggle haiku back off (wrap-around j,k back to 0)
        ed.handle_key(key(KeyCode::Char('k')));
        ed.handle_key(key(KeyCode::Char(' ')));
        assert!(!ed.slot_owner.contains_key("haiku"));
        assert!(ed.slot_picker.is_some());

        // Enter commits and closes
        ed.handle_key(key(KeyCode::Enter));
        assert!(ed.slot_picker.is_none());
        assert_eq!(ed.slot_owner.get("sonnet").map(String::as_str), Some("a"));
    }

    #[test]
    fn target_picker_space_marks_enter_commits_mark() {
        // space marks the cursor item (checkbox semantics); Enter commits
        // the mark even after the cursor moves away from it.
        let mut ed = claude_editor_with(
            vec![ModelEntry {
                id: "a".into(),
                ..ModelEntry::default()
            }],
            "a",
        );
        ed.row = 0;
        focus_field(&mut ed, CatalogField::TargetModelId);
        ed.handle_key(key(KeyCode::Char(' ')));
        assert!(ed.target_picker.is_some());

        // cursor parks at 0 "(none)" (row has no target yet); move to the
        // first known id and space-mark it
        ed.handle_key(key(KeyCode::Char('j')));
        assert_eq!(ed.target_picker.as_ref().unwrap().cursor, 1);
        ed.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(ed.target_picker.as_ref().unwrap().mark, Some(1));
        assert!(ed.target_picker.is_some(), "space keeps the popover open");

        // move the cursor away — the mark must survive
        ed.handle_key(key(KeyCode::Char('j')));
        assert_ne!(ed.target_picker.as_ref().unwrap().cursor, 1);

        // Enter commits the marked id, not the cursor's
        ed.handle_key(key(KeyCode::Enter));
        assert!(ed.target_picker.is_none());
        assert_eq!(
            ed.rows[0].target_model_id.as_deref(),
            Some(models::KNOWN_CLAUDE_MODEL_IDS[0])
        );
    }

    #[test]
    fn claude_slot_assignment_steals() {
        let mut ed = claude_editor_with(
            vec![
                ModelEntry {
                    id: "a".into(),
                    ..ModelEntry::default()
                },
                ModelEntry {
                    id: "b".into(),
                    ..ModelEntry::default()
                },
            ],
            "a",
        );
        // Open the popover on row 1 (non-default) and assign sonnet.
        ed.row = 1;
        focus_field(&mut ed, CatalogField::Slots);
        ed.handle_key(key(KeyCode::Char(' ')));
        assert!(ed.slot_picker.is_some());
        // cursor starts at haiku (0). sonnet is index 1.
        ed.handle_key(key(KeyCode::Char('j')));
        // space toggles the slot assignment and keeps the popover open
        ed.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(ed.slot_owner.get("sonnet").map(String::as_str), Some("b"));
        assert!(ed.slot_picker.is_some());
        ed.handle_key(key(KeyCode::Enter));
        assert!(ed.slot_picker.is_none());

        // Reassign sonnet to a -> b is evicted.
        ed.row = 0;
        focus_field(&mut ed, CatalogField::Slots);
        ed.handle_key(key(KeyCode::Char(' ')));
        ed.handle_key(key(KeyCode::Char('j')));
        ed.handle_key(key(KeyCode::Char(' ')));
        ed.handle_key(key(KeyCode::Enter));
        assert_eq!(ed.slot_owner.get("sonnet").map(String::as_str), Some("a"));
    }

    #[test]
    fn target_picker_picks_known_id_and_lands_on_existing() {
        // Opening the target picker on a row that already has a known target
        // lands the cursor on that target (offset by 1 for the "(none)" entry).
        let mut ed = claude_editor_with(
            vec![
                ModelEntry {
                    id: "a".into(),
                    target_model_id: Some("claude-sonnet-4-6".into()),
                    ..ModelEntry::default()
                },
                ModelEntry {
                    id: "b".into(),
                    ..ModelEntry::default()
                },
            ],
            "a",
        );
        focus_field(&mut ed, CatalogField::TargetModelId);
        // Open on row 0 (already has claude-sonnet-4-6).
        ed.row = 0;
        ed.handle_key(key(KeyCode::Char(' ')));
        let p = ed.target_picker.as_ref().expect("picker open");
        // index 0 in KNOWN is "claude-haiku-3-5"; claude-sonnet-4-6 is
        // further down — the exact index doesn't matter; just confirm
        // it's not the default 0.
        assert!(p.cursor > 0);

        // Pick the first known id (cursor 1 → KNOWN[0]).
        ed.target_picker.as_mut().unwrap().cursor = 1;
        ed.handle_key(key(KeyCode::Enter));
        assert_eq!(
            ed.rows[0].target_model_id.as_deref(),
            Some("claude-haiku-3-5")
        );
        assert!(ed.target_picker.is_none());
    }

    #[test]
    fn target_picker_clears_with_none_entry() {
        let mut ed = claude_editor_with(
            vec![ModelEntry {
                id: "a".into(),
                target_model_id: Some("claude-opus-4-7".into()),
                ..ModelEntry::default()
            }],
            "a",
        );
        focus_field(&mut ed, CatalogField::TargetModelId);
        ed.handle_key(key(KeyCode::Char(' ')));
        // Cursor 0 in the picker = "(none)".
        ed.target_picker.as_mut().unwrap().cursor = 0;
        ed.handle_key(key(KeyCode::Enter));
        assert!(ed.rows[0].target_model_id.is_none());
    }

    #[test]
    fn target_picker_esc_cancels() {
        let mut ed = claude_editor_with(
            vec![ModelEntry {
                id: "a".into(),
                target_model_id: Some("claude-sonnet-4-6".into()),
                ..ModelEntry::default()
            }],
            "a",
        );
        focus_field(&mut ed, CatalogField::TargetModelId);
        ed.handle_key(key(KeyCode::Char(' ')));
        assert!(ed.target_picker.is_some());
        ed.handle_key(key(KeyCode::Esc));
        assert!(ed.target_picker.is_none());
        // Original value preserved.
        assert_eq!(
            ed.rows[0].target_model_id.as_deref(),
            Some("claude-sonnet-4-6")
        );
    }

    #[test]
    fn project_to_provider_writes_model_and_slots() {
        let mut ed = claude_editor_with(
            vec![
                ModelEntry {
                    id: "a".into(),
                    ..ModelEntry::default()
                },
                ModelEntry {
                    id: "b".into(),
                    ..ModelEntry::default()
                },
            ],
            "a",
        );
        ed.row = 1;
        focus_field(&mut ed, CatalogField::Slots);
        ed.handle_key(key(KeyCode::Char(' ')));
        ed.handle_key(key(KeyCode::Char('j')));
        ed.handle_key(key(KeyCode::Char(' ')));
        ed.handle_key(key(KeyCode::Enter));

        let mut p = Provider::blank(AppId::Claude);
        ed.project_to_provider(&mut p);
        assert_eq!(p.model.as_deref(), Some("a"));
        assert_eq!(p.slots.get("sonnet").map(String::as_str), Some("b"));
        // other slots not assigned
        assert!(!p.slots.contains_key("haiku"));
    }

    #[test]
    fn delete_row_drops_slot_bindings() {
        let mut ed = claude_editor_with(
            vec![
                ModelEntry {
                    id: "a".into(),
                    ..ModelEntry::default()
                },
                ModelEntry {
                    id: "b".into(),
                    ..ModelEntry::default()
                },
            ],
            "a",
        );
        ed.row = 1;
        focus_field(&mut ed, CatalogField::Slots);
        ed.handle_key(key(KeyCode::Char(' ')));
        ed.handle_key(key(KeyCode::Char('j')));
        ed.handle_key(key(KeyCode::Char(' ')));
        ed.handle_key(key(KeyCode::Enter));
        assert_eq!(ed.slot_owner.get("sonnet").map(String::as_str), Some("b"));

        ed.row = 1;
        ed.handle_key(key(KeyCode::Char('d')));
        assert_eq!(ed.slot_owner.get("sonnet"), None);
        assert_eq!(ed.rows.len(), 1);
    }

    #[test]
    fn popover_unassign_does_not_set_pending_dropped_slots() {
        // Cancelling a slot binding through the slots popover (Enter on
        // an already-assigned slot) should NOT set `pending_dropped_slots`:
        // the row isn't deleted, the app must not show a "row deleted" message.
        let mut ed = claude_editor_with(
            vec![
                ModelEntry {
                    id: "a".into(),
                    ..ModelEntry::default()
                },
                ModelEntry {
                    id: "b".into(),
                    ..ModelEntry::default()
                },
            ],
            "a",
        );
        // Give row 1 the sonnet slot.
        ed.row = 1;
        focus_field(&mut ed, CatalogField::Slots);
        ed.handle_key(key(KeyCode::Char(' ')));
        ed.handle_key(key(KeyCode::Char('j')));
        ed.handle_key(key(KeyCode::Char(' ')));
        ed.handle_key(key(KeyCode::Enter));
        assert_eq!(ed.slot_owner.get("sonnet").map(String::as_str), Some("b"));
        assert_eq!(ed.pending_dropped_slots, 0);

        // Now open the popover again on row 1 and re-toggle the same slot
        // (cursor starts at haiku; press j to land on sonnet, then space
        // to unassign, Enter to close).
        ed.row = 1;
        focus_field(&mut ed, CatalogField::Slots);
        ed.handle_key(key(KeyCode::Char(' ')));
        ed.handle_key(key(KeyCode::Char('j')));
        ed.handle_key(key(KeyCode::Char(' ')));
        ed.handle_key(key(KeyCode::Enter));
        assert!(!ed.slot_owner.contains_key("sonnet"));
        // Popover cancel must NOT have set the pending field.
        assert_eq!(ed.pending_dropped_slots, 0);
    }

    #[test]
    fn delete_default_row_records_new_default_id() {
        // Deleting the Default row sets `deleted_default_to` to the new
        // default's id, so the app can surface a "Default moved" status.
        let mut ed = claude_editor_with(
            vec![
                ModelEntry {
                    id: "a".into(),
                    ..ModelEntry::default()
                },
                ModelEntry {
                    id: "b".into(),
                    ..ModelEntry::default()
                },
            ],
            "a",
        );
        assert_eq!(ed.default_idx, 0);
        ed.handle_key(key(KeyCode::Char('d')));
        assert_eq!(ed.rows.len(), 1);
        assert_eq!(ed.default_idx, 0, "deleted row shifts; b takes its index");
        assert_eq!(ed.deleted_default_to, Some(Some("b".into())));
    }

    #[test]
    fn delete_only_row_records_default_removed() {
        // Deleting the lone row empties the catalog; `deleted_default_to`
        // is `Some(None)` so the app knows to say "now empty".
        let mut ed = claude_editor_with(
            vec![ModelEntry {
                id: "a".into(),
                ..ModelEntry::default()
            }],
            "a",
        );
        ed.handle_key(key(KeyCode::Char('d')));
        assert!(ed.rows.is_empty());
        assert_eq!(ed.deleted_default_to, Some(None));
    }
}
