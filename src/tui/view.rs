use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use crate::adapter::FieldKind;

use crate::i18n::{t, tf};
use crate::mask;

use super::app::{App, Overlay, Page, SyncKind, SyncTab};
use super::edit;
use super::help;
use super::pages::form;
use super::pages::models::{self, CatalogEditor, ModelPicker, PickerKind, SlotEditor};
use super::pages::{backups, settings as settings_page, sync};
use super::theme::Theme;
use crate::adapter::models as admodels;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let theme = Theme::for_app(app.current_app());
    match app.page {
        Page::Providers => {
            draw_tabs(frame, app, chunks[0], theme);
            draw_list(frame, app, chunks[1], theme);
        }
        Page::Data => {
            draw_title(frame, chunks[0], t("ui.data"), theme);
            let areas = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(5), Constraint::Length(8)])
                .split(chunks[1]);
            draw_backups(frame, app, areas[0], theme);
            draw_sync(frame, app, areas[1], theme);
        }
        Page::Settings => {
            draw_title(frame, chunks[0], t("ui.settings"), theme);
            draw_settings(frame, app, chunks[1], theme);
        }
    }
    draw_status(frame, app, chunks[2], theme);
    match &mut app.overlay {
        Overlay::Form(form) => draw_form(frame, form, frame.area(), theme),
        Overlay::ConfirmDelete { id, name } => draw_confirm(
            frame,
            frame.area(),
            t("ui.delete"),
            &tf("confirm.delete", &[id, name]),
            theme,
        ),
        Overlay::ConfirmRestore { name } => draw_confirm(
            frame,
            frame.area(),
            t("ui.restore"),
            &tf("confirm.restore", &[name]),
            theme,
        ),
        Overlay::ConfirmSync {
            kind,
            url,
            last_sync_at,
        } => {
            let title = match kind {
                SyncKind::Push => t("ui.sync_push"),
                SyncKind::Pull => t("ui.sync_pull"),
            };
            let key = match kind {
                SyncKind::Push => "confirm.sync_push",
                SyncKind::Pull => "confirm.sync_pull",
            };
            let last_display = if last_sync_at.is_empty() {
                t("ui.sync_never").to_string()
            } else {
                last_sync_at.get(..19).unwrap_or(last_sync_at).to_string()
            };
            draw_confirm(
                frame,
                frame.area(),
                title,
                &tf(key, &[url, &last_display]),
                theme,
            )
        }
        Overlay::Syncing => draw_busy(frame, frame.area(), t("ui.sync"), sync::syncing(), theme),
        Overlay::FetchingModels => draw_busy(
            frame,
            frame.area(),
            t("ui.models"),
            t("ui.fetching_models"),
            theme,
        ),
        Overlay::ModelPicker(picker) => draw_picker(frame, picker, frame.area(), theme),
        Overlay::CatalogEditor { editor, .. } => draw_catalog(frame, editor, frame.area(), theme),
        Overlay::SlotEditor { editor, .. } => draw_slots(frame, editor, frame.area(), theme),
        Overlay::SnippetEditor(page) => draw_snippet(frame, page, frame.area(), theme),
        Overlay::None => {}
    }
    if app.help {
        draw_help(frame, app, frame.area(), theme);
    }
}

fn draw_title(frame: &mut Frame, area: Rect, title: &str, theme: Theme) {
    frame.render_widget(
        Block::bordered()
            .border_style(theme.accent())
            .title(Span::styled(
                title,
                theme.accent().add_modifier(Modifier::BOLD),
            )),
        area,
    );
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    let titles = app.tab_titles();
    let tabs = Tabs::new(titles)
        .select(app.app_idx)
        .block(
            Block::bordered()
                .border_style(theme.accent())
                .title(Span::styled(t("ui.apps"), theme.accent())),
        )
        .style(theme.fg(theme.dim))
        .highlight_style(theme.selected());
    frame.render_widget(tabs, area);
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect, theme: Theme) {
    let current = app.current_id().map(str::to_string);
    let items: Vec<ListItem> = app
        .providers()
        .into_iter()
        .map(|p| {
            let is_current = current.as_deref() == Some(p.id.as_str());
            let mark = if is_current { " *" } else { "  " };
            let secret = if p.official {
                t("list.official").to_string()
            } else {
                mask::mask_key(&p.api_key)
            };
            let line = Line::from(vec![
                Span::raw(format!("{:<24} ", p.name)),
                Span::styled(secret, theme.fg(theme.dim)),
                Span::styled(
                    mark,
                    if is_current {
                        theme.current_mark()
                    } else {
                        Style::default()
                    },
                ),
            ]);
            ListItem::new(line)
        })
        .collect();
    let title = tf("ui.providers_title", &[adapter_label(app)]);
    let list = List::new(items)
        .block(
            Block::bordered()
                .border_style(theme.accent())
                .title(Span::styled(title, theme.accent())),
        )
        .highlight_style(theme.selected())
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn adapter_label(app: &App) -> &'static str {
    crate::adapter::get(app.current_app())
        .map(|a| a.display_name())
        .unwrap_or("?")
}

fn draw_backups(frame: &mut Frame, app: &mut App, area: Rect, theme: Theme) {
    let items: Vec<ListItem> = app
        .backups
        .iter()
        .map(|e| ListItem::new(backups::row(e)))
        .collect();
    let list = List::new(items)
        .block(
            Block::bordered()
                .border_style(theme.accent())
                .title(Span::styled(t("ui.backups"), theme.accent())),
        )
        .highlight_style(theme.selected())
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut app.backup_state);
}

fn draw_sync(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    // Tabs live in the panel title row: "Sync WebDAV|Gist".
    let tab_style = |on: bool| {
        if on {
            theme.selected()
        } else {
            theme.fg(theme.dim)
        }
    };
    let title = Line::from(vec![
        Span::styled(format!("{} ", t("ui.sync")), theme.accent()),
        Span::styled(
            t("ui.backend_webdav").to_string(),
            tab_style(app.sync_tab == SyncTab::Webdav),
        ),
        Span::styled("|", theme.fg(theme.dim)),
        Span::styled(
            t("ui.backend_gist").to_string(),
            tab_style(app.sync_tab == SyncTab::Gist),
        ),
    ]);
    let body = match app.sync_tab {
        SyncTab::Webdav => match &app.sync_local {
            None => t("ui.webdav_unconfigured").to_string(),
            Some(s) => {
                let last = if s.last_sync_at.is_empty() {
                    "-"
                } else {
                    s.last_sync_at.as_str()
                };
                format!(
                    "url: {}\nnamespace: {}\nusername: {}\nlast_pulled: {}\nlast_pushed: {}\nlast_sync_at: {last}",
                    crate::webdav::redact_url(&s.url),
                    crate::webdav::NAMESPACE,
                    s.username,
                    s.last_pulled_sha256,
                    s.last_pushed_sha256
                )
            }
        },
        SyncTab::Gist => match &app.gist_local {
            None => t("ui.gist_unconfigured").to_string(),
            Some(g) => {
                let last = if g.last_sync_at.is_empty() {
                    "-"
                } else {
                    g.last_sync_at.as_str()
                };
                format!(
                    "gist: https://gist.github.com/{}\nlast_pulled: {}\nlast_pushed: {}\nlast_sync_at: {last}",
                    g.gist_id, g.last_pulled_sha256, g.last_pushed_sha256
                )
            }
        },
    };
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .style(theme.fg(theme.fg))
            .block(Block::bordered().border_style(theme.accent()).title(title)),
        area,
    );
}

fn draw_settings(frame: &mut Frame, app: &mut App, area: Rect, theme: Theme) {
    let detected = app.detected_apps();
    let apps = crate::settings::all_apps();
    let mut items = vec![
        setting_item(t("settings.language"), settings_page::lang_value(), theme),
        setting_item(
            t("settings.apps_mode"),
            settings_page::mode_value(&app.settings),
            theme,
        ),
    ];
    for app_id in &apps {
        let name = crate::adapter::get(*app_id)
            .map(|a| a.display_name())
            .unwrap_or("?");
        let value = settings_page::app_value(&app.settings, detected.contains(app_id), *app_id);
        items.push(setting_item(name, &value, theme));
    }
    let list = List::new(items)
        .block(
            Block::bordered()
                .border_style(theme.accent())
                .title(Span::styled(t("ui.settings"), theme.accent())),
        )
        .highlight_style(theme.selected())
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut app.settings_state);
}

fn setting_item(label: &str, value: &str, theme: Theme) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::raw(format!("{label:<16} ")),
        Span::styled(value.to_string(), theme.fg(theme.ok)),
    ]))
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    let hint = app.hint();
    let flash = if app.status.is_empty() {
        Line::from("")
    } else {
        Line::from(Span::styled(app.status.as_str(), theme.flash(&app.status)))
    };
    let p = Paragraph::new(flash).wrap(Wrap { trim: true }).block(
        Block::bordered()
            .border_style(theme.accent())
            .title(Span::styled(hint, theme.fg(theme.dim))),
    );
    frame.render_widget(p, area);
}

fn draw_form(frame: &mut Frame, form: &form::Form, area: Rect, theme: Theme) {
    let mut lines: Vec<Line> = Vec::new();
    for (i, f) in form.fields.iter().enumerate() {
        let mark = if i == form.focus && !f.readonly {
            ">"
        } else {
            " "
        };
        let req = if f.required { "*" } else { " " };
        let focused = i == form.focus;
        let mut spans = vec![Span::styled(
            format!("{mark}{req}{}: ", crate::i18n::t(f.label)),
            if f.readonly {
                theme.fg(theme.dim)
            } else {
                theme.fg(theme.fg)
            },
        )];
        spans.extend(form::value_spans(
            f,
            focused,
            if focused && !matches!(f.kind, FieldKind::Select(_)) {
                theme.fg(theme.fg)
            } else if focused {
                theme.accent()
            } else if f.readonly {
                theme.fg(theme.dim)
            } else {
                theme.fg(theme.fg)
            },
        ));
        lines.push(Line::from(spans));
    }
    if let Some(err) = &form.error {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("! {err}")));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(t("ui.form_hint")));
    let h = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(6);
    let w = 64.min(area.width.saturating_sub(4)).max(24);
    let popup = centered(area, w, h);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .border_style(theme.accent())
                .title(Span::styled(form.title(), theme.accent())),
        ),
        popup,
    );
}

fn draw_confirm(frame: &mut Frame, area: Rect, title: &str, msg: &str, theme: Theme) {
    let text = format!("{msg}\n\n{}", t("ui.confirm_hint"));
    let popup = centered(area, 52.min(area.width.saturating_sub(4)).max(24), 7);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: true }).block(
            Block::bordered()
                .border_style(theme.fg(theme.warn))
                .title(Span::styled(title, theme.fg(theme.warn))),
        ),
        popup,
    );
}

fn draw_busy(frame: &mut Frame, area: Rect, title: &str, body: &str, theme: Theme) {
    let popup = centered(area, 28.min(area.width.saturating_sub(4)).max(12), 5);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(body).block(
            Block::bordered()
                .border_style(theme.accent())
                .title(Span::styled(title, theme.accent())),
        ),
        popup,
    );
}

fn draw_picker(frame: &mut Frame, picker: &mut ModelPicker, area: Rect, theme: Theme) {
    let w = 72.min(area.width.saturating_sub(4)).max(24);
    let h = 16.min(area.height.saturating_sub(2)).max(6);
    let popup = centered(area, w, h);
    // Viewport = popup inner minus filter/error rows reserved below.
    let inner_h = popup.height.saturating_sub(2) as usize;
    let reserve = usize::from(picker.filtering || !picker.filter.is_empty())
        + 2 * usize::from(picker.error.is_some());
    picker.page_rows = inner_h.saturating_sub(reserve).max(1);
    picker.ensure_visible();
    let vis = picker.visible();
    let start = picker.scroll.min(vis.len());
    let end = (start + picker.page_rows).min(vis.len());
    let mut lines: Vec<Line> = Vec::new();
    if picker.filtering || !picker.filter.is_empty() {
        let mut spans = vec![Span::styled("/ ", theme.fg(theme.fg))];
        if picker.filtering {
            spans.extend(edit::caret_spans(
                &picker.filter,
                picker.filter_cursor,
                theme.fg(theme.fg),
            ));
        } else {
            spans.push(Span::styled(picker.filter.clone(), theme.fg(theme.fg)));
        }
        lines.push(Line::from(spans));
    }
    for &idx in &vis[start..end] {
        let cur = if idx == picker.cursor { ">" } else { " " };
        let id = &picker.ids[idx];
        let text = match picker.kind {
            PickerKind::Catalog => {
                let mark = toggle_mark(picker.selected.contains(&idx));
                format!("{cur}{mark} {id}")
            }
            PickerKind::Slot { .. } => format!("{cur} {id}"),
        };
        lines.push(Line::from(text));
    }
    if let Some(err) = &picker.error {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("! {err}")));
    }
    let title = match vis.iter().position(|&i| i == picker.cursor) {
        Some(pos) => format!("{} {}/{}", t("ui.model_picker"), pos + 1, vis.len()),
        None => t("ui.model_picker").to_string(),
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .border_style(theme.accent())
                .title(Span::styled(title, theme.accent())),
        ),
        popup,
    );
}

fn catalog_cell_text(
    editor: &CatalogEditor,
    i: usize,
    field: &crate::adapter::models::CatalogField,
) -> String {
    let row = &editor.rows[i];
    match field {
        crate::adapter::models::CatalogField::Id => row.id.clone(),
        crate::adapter::models::CatalogField::Label => row.label.clone().unwrap_or_default(),
        crate::adapter::models::CatalogField::ContextWindow => row
            .context_window
            .map(|n| n.to_string())
            .unwrap_or_default(),
        crate::adapter::models::CatalogField::MaxTokens => {
            row.max_tokens.map(|n| n.to_string()).unwrap_or_default()
        }
        // Comma-joined list of the slot aliases that own this row.
        crate::adapter::models::CatalogField::Slots => {
            let names: Vec<&str> = admodels::CLAUDE_SLOTS
                .iter()
                .filter(|s| {
                    editor
                        .slot_owner
                        .get(s.key)
                        .map(|owner| owner == &row.id)
                        .unwrap_or(false)
                })
                .map(|s| t(s.label))
                .collect();
            if names.is_empty() {
                String::new()
            } else {
                names.join(",")
            }
        }
        crate::adapter::models::CatalogField::TargetModelId => {
            row.target_model_id.clone().unwrap_or_default()
        }
    }
}

/// Width of the visible text produced by caret_spans for `text`/`cursor`:
/// character count plus one when the cursor sits past the end (the
/// underlined insertion-point space).
fn caret_visible_width(text: &str, cursor: usize) -> usize {
    let c = edit::clamp(text, cursor);
    if c >= text.chars().count() {
        c + 1
    } else {
        c
    }
}

/// Pad `text` to exactly `width` display columns, truncating with an
/// ellipsis when it overflows.
fn pad_cell(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        let mut out = text.to_string();
        out.extend(std::iter::repeat_n(' ', width - count));
        out
    } else if width == 0 {
        String::new()
    } else {
        let mut out: String = text.chars().take(width - 1).collect();
        out.push('\u{2026}');
        out
    }
}

fn draw_catalog(frame: &mut Frame, editor: &CatalogEditor, area: Rect, theme: Theme) {
    // --- adaptive column widths -----------------------------------------
    // Each column grows to fit its header and every row's content (clamped),
    // then shrinks together if the terminal is too narrow. This replaces the
    // old fixed 16-wide grid where long ids collided with later columns.
    const MIN_COL: usize = 8;
    const MAX_COL: usize = 34;
    let mut widths: Vec<usize> = editor
        .fields
        .iter()
        .map(|f| models::field_label(*f).chars().count())
        .collect();
    for (c, field) in editor.fields.iter().enumerate() {
        for i in 0..editor.rows.len() {
            let len = catalog_cell_text(editor, i, field).chars().count();
            widths[c] = widths[c].max(len);
        }
    }
    for w in &mut widths {
        *w = (*w + 2).clamp(MIN_COL, MAX_COL);
    }

    // Fit into the popup: borders(2) + row prefix(2). Shrink the widest
    // columns first until everything fits or we hit MIN_COL.
    let avail = (area.width.saturating_sub(4) as usize).saturating_sub(2);
    let mut total: usize = widths.iter().sum();
    while total + 2 * widths.len() > avail && widths.iter().any(|w| *w > MIN_COL) {
        let max_w = *widths.iter().max().unwrap();
        for w in widths.iter_mut() {
            if *w == max_w && *w > MIN_COL {
                *w -= 1;
            }
        }
        total = widths.iter().sum();
    }
    let want_w = (total as u16 + 2 + 4)
        .min(area.width.saturating_sub(4))
        .max(24);

    // --- header -----------------------------------------------------------
    // same ">*" prefix width as data rows so columns line up vertically
    let mut header_spans: Vec<Span> = vec![Span::styled("  ", theme.fg(theme.dim))];
    for (c, field) in editor.fields.iter().enumerate() {
        header_spans.push(Span::styled(
            pad_cell(models::field_label(*field), widths[c]),
            theme.fg(theme.dim),
        ));
    }
    let mut lines = vec![Line::from(header_spans)];

    // --- rows -------------------------------------------------------------
    for i in 0..editor.rows.len() {
        let star = if i == editor.default_idx { "*" } else { " " };
        let cur = if i == editor.row { ">" } else { " " };
        let mut spans = vec![Span::styled(format!("{cur}{star}"), theme.fg(theme.fg))];
        let cell_style = theme.fg(theme.fg);
        for (c, field) in editor.fields.iter().enumerate() {
            let editing_cell = editor.editing && i == editor.row && c == editor.col;
            let raw = catalog_cell_text(editor, i, field);
            let selected = i == editor.row && c == editor.col;
            if editing_cell {
                // Bracketed like the selected cell; inner width is fixed so
                // entering edit mode never shifts the rest of the row.
                let inner = widths[c].saturating_sub(2).max(1);
                spans.push(Span::styled("[", cell_style));
                // Tail window: keep the cursor visible inside the fixed width.
                let cur_c = edit::clamp(&editor.buf, editor.buf_cursor);
                let start = cur_c.saturating_sub(inner.saturating_sub(1)).min(cur_c);
                let window: String = editor.buf.chars().skip(start).take(inner).collect();
                let shown = caret_visible_width(&window, cur_c - start);
                spans.extend(edit::caret_spans(&window, cur_c - start, cell_style));
                if shown < inner {
                    spans.push(Span::styled(" ".repeat(inner - shown), cell_style));
                }
                spans.push(Span::styled("]", cell_style));
            } else if selected {
                spans.push(Span::styled(
                    format!("[{}]", pad_cell(&raw, widths[c] - 2)),
                    cell_style,
                ));
            } else {
                spans.push(Span::styled(pad_cell(&raw, widths[c]), cell_style));
            }
        }
        lines.push(Line::from(spans));
    }
    popup_lines_w(frame, area, t("ui.catalog"), lines, theme, 18, want_w);

    // --- in-cell popovers ---------------------------------------------------
    // The slot-assignment and target-model-id pickers are editor state, not
    // separate overlays; draw them on top of the catalog grid.
    if let Some(picker) = editor.slot_picker.as_ref() {
        let lines: Vec<Line> = admodels::CLAUDE_SLOTS
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let cur = if i == picker.cursor { ">" } else { " " };
                let own = editor
                    .rows
                    .get(picker.row)
                    .map(|r| r.id.as_str())
                    .unwrap_or("");
                let mark = editor
                    .slot_owner
                    .get(s.key)
                    .map(|owner| owner == own)
                    .unwrap_or(false);
                Line::from(Span::styled(
                    format!("{cur}{} {}", toggle_mark(mark), t(s.label)),
                    theme.fg(theme.fg),
                ))
            })
            .collect();
        popup_lines_w(frame, area, t("field.slot_assignment"), lines, theme, 8, 28);
    }
    if let Some(picker) = editor.target_picker.as_ref() {
        let mut lines = Vec::new();
        // Item 0 is "(none)" — clears the row's target_model_id.
        let mark_idx = picker.mark;
        let mark_of = |i: usize| toggle_mark(mark_idx == Some(i));
        let none_cur = if picker.cursor == 0 { ">" } else { " " };
        lines.push(Line::from(Span::styled(
            format!("{none_cur}{} (none)", mark_of(0)),
            theme.fg(theme.fg),
        )));
        for (i, id) in admodels::KNOWN_CLAUDE_MODEL_IDS.iter().enumerate() {
            let cur = if i + 1 == picker.cursor { ">" } else { " " };
            lines.push(Line::from(Span::styled(
                format!("{cur}{} {id}", mark_of(i + 1)),
                theme.fg(theme.fg),
            )));
        }
        popup_lines_w(frame, area, t("field.target_model_id"), lines, theme, 8, 36);
    }
}

fn draw_slots(frame: &mut Frame, editor: &SlotEditor, area: Rect, theme: Theme) {
    let mut lines = Vec::new();
    for i in 0..editor.row_count() {
        let cur = if i == editor.row { ">" } else { " " };
        let (label, value) = if i == 0 {
            (t("slot.default"), editor.default_model.as_str())
        } else if let Some(s) = editor.slots.get(i - 1) {
            (
                t(s.label),
                editor.values.get(s.key).map(String::as_str).unwrap_or(""),
            )
        } else {
            ("", "")
        };
        let editing_row = editor.editing && i == editor.row;
        let mut line_spans = vec![Span::styled(
            format!("{cur}{label:<12} "),
            theme.fg(theme.fg),
        )];
        if editing_row {
            line_spans.extend(edit::caret_spans(
                &editor.buf,
                editor.buf_cursor,
                theme.fg(theme.fg),
            ));
        } else {
            line_spans.push(Span::styled(value.to_string(), theme.fg(theme.fg)));
        }
        lines.push(Line::from(line_spans));
    }
    popup_lines(frame, area, t("ui.slots"), lines, theme, 14);
}

/// Checkbox glyph for toggle lists. Brackets keep the empty box visible, so
/// every row still reads as something Space can toggle; the check replaces
/// the `x` the toggles used to draw.
fn toggle_mark(on: bool) -> &'static str {
    if on {
        "[✓]"
    } else {
        "[ ]"
    }
}

fn draw_snippet(
    frame: &mut Frame,
    page: &super::pages::quick::SnippetPage,
    area: Rect,
    theme: Theme,
) {
    let snippet = page
        .parsed_snippet()
        .unwrap_or_else(|_| serde_json::json!({}));
    let items = page.items();
    let mut lines: Vec<Line> = Vec::new();
    if !items.is_empty() {
        lines.push(Line::from(Span::styled(
            t("quick.builtin"),
            theme.fg(theme.dim),
        )));
        for (i, item) in items.iter().enumerate() {
            let cur = if page.cursor == i { ">" } else { " " };
            let mark = toggle_mark(page.item_on(item, &snippet));
            lines.push(Line::from(format!("{cur}{mark} {}", t(item.label))));
        }
        lines.push(Line::from(""));
    }
    let json_cur = if page.json_focused() { ">" } else { " " };
    lines.push(Line::from(Span::styled(
        format!("{json_cur}{}", t(page.body_label())),
        theme.fg(theme.dim),
    )));
    let body_style = theme.fg(theme.fg);
    if page.json_focused() {
        // Locate the cursor's line; only that line carries the caret spans,
        // so the underline marks a position instead of occupying a character.
        let c = crate::tui::edit::clamp(&page.text, page.text_cursor);
        let mut consumed = 0usize;
        let mut caret_done = false;
        for seg in page.text.split('\n') {
            let seg_len = seg.chars().count();
            if !caret_done && c <= consumed + seg_len {
                lines.push(Line::from(edit::caret_spans(seg, c - consumed, body_style)));
                caret_done = true;
            } else {
                lines.push(Line::from(Span::styled(seg.to_string(), body_style)));
            }
            consumed += seg_len + 1; // + newline
        }
    } else {
        for line in page.text.lines() {
            lines.push(Line::from(Span::styled(line.to_string(), body_style)));
        }
    }
    if let Some(err) = &page.error {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("! {err}")));
    }
    popup_lines(frame, area, t("ui.snippet"), lines, theme, 16);
}

fn popup_lines(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line>,
    theme: Theme,
    min_h: u16,
) {
    popup_lines_w(frame, area, title, lines, theme, min_h, 72)
}

fn popup_lines_w(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line>,
    theme: Theme,
    min_h: u16,
    want_w: u16,
) {
    let h = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(min_h.min(area.height.saturating_sub(2)).max(6));
    let w = want_w.min(area.width.saturating_sub(4)).max(24);
    let popup = centered(area, w, h);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .border_style(theme.accent())
                .title(Span::styled(title, theme.accent())),
        ),
        popup,
    );
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect, theme: Theme) {
    let text = help::text(app);
    let lines = text.lines().count() as u16;
    let w = 52.min(area.width.saturating_sub(4)).max(24);
    let h = (lines + 2).min(area.height.saturating_sub(2)).max(8);
    let popup = centered(area, w, h);
    frame.render_widget(Clear, popup);
    let help = Paragraph::new(text).wrap(Wrap { trim: false }).block(
        Block::bordered()
            .border_style(theme.accent())
            .title(Span::styled(t("ui.help"), theme.accent())),
    );
    frame.render_widget(help, popup);
}

pub(crate) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w.min(area.width), h.min(area.height))
}

#[cfg(test)]
mod tests {
    #[test]
    fn catalog_popovers_are_rendered_when_open() {
        // Regression: handle_space/begin_edit set slot_picker/target_picker
        // state, but draw_catalog never drew them — the popover was open,
        // invisible, and ate every keypress. Both popovers must now paint.
        use crate::adapter::models::CLAUDE_FIELDS;
        use crate::store::ModelEntry;
        use crate::tui::pages::models::{CatalogEditor, SlotPickerState, TargetPickerState};
        let mut editor = CatalogEditor::new(
            CLAUDE_FIELDS,
            vec![ModelEntry {
                id: "hilinkup/z-ai/glm-5.3".into(),
                target_model_id: Some("claude-sonnet-4-6".into()),
                ..ModelEntry::default()
            }],
            Some("hilinkup/z-ai/glm-5.3"),
        );
        editor.row = 0;
        editor.col = 4; // TargetModelId
        editor.target_picker = Some(TargetPickerState {
            row: 0,
            cursor: 1,
            mark: None,
        });

        let render = |editor: &CatalogEditor| -> String {
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            let theme = Theme::for_app(AppId::Claude);
            terminal
                .draw(|f| draw_catalog(f, editor, f.area(), theme))
                .unwrap();
            (0..30)
                .map(|y| row(&terminal, y))
                .collect::<Vec<_>>()
                .join("\n")
        };

        // Target picker: mutually exclusive with the slot picker in practice
        // (an open popover intercepts every key), so render them separately.
        let target_screen = render(&editor);
        assert!(
            target_screen.contains("claude-sonnet-4-6"),
            "target picker rows"
        );
        assert!(
            target_screen.contains("(none)"),
            "target picker (none) item"
        );
        assert!(
            target_screen.contains("Target model id"),
            "target picker title"
        );

        editor.target_picker = None;
        editor.col = 3; // Slots
        editor.slot_picker = Some(SlotPickerState { row: 0, cursor: 0 });
        let slot_screen = render(&editor);
        for slot in ["Haiku", "Sonnet", "Opus", "Fable", "Subagent"] {
            assert!(slot_screen.contains(slot), "slot picker {slot}");
        }
    }

    #[test]
    fn catalog_shows_slot_and_target_values_inline() {
        // The Slots / TargetModelId columns used to render always-empty text;
        // the grid must show the row's slot aliases and target id.
        use crate::adapter::models::CLAUDE_FIELDS;
        use crate::store::ModelEntry;
        use crate::tui::pages::models::CatalogEditor;
        let mut editor = CatalogEditor::new(
            CLAUDE_FIELDS,
            vec![ModelEntry {
                id: "glm".into(),
                target_model_id: Some("claude-sonnet-4-6".into()),
                ..ModelEntry::default()
            }],
            Some("glm"),
        );
        editor.slot_owner.insert("sonnet", "glm".into());
        editor.slot_owner.insert("subagent", "glm".into());

        let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
        let theme = Theme::for_app(AppId::Claude);
        terminal
            .draw(|f| draw_catalog(f, &editor, f.area(), theme))
            .unwrap();
        let joined: String = (0..20)
            .map(|y| row(&terminal, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Sonnet"), "slot alias shown inline");
        assert!(joined.contains("Subagent"), "slot alias shown inline");
        assert!(joined.contains("claude-sonnet-4-6"), "target shown inline");
    }

    #[test]
    fn catalog_editing_keeps_columns_aligned() {
        use crate::adapter::models::OPENCODE_FIELDS;
        use crate::store::ModelEntry;
        use crate::tui::pages::models::CatalogEditor;
        let make = || {
            CatalogEditor::new(
                OPENCODE_FIELDS,
                vec![ModelEntry {
                    id: "claude-opus-4-5-20250901".into(),
                    label: Some("Opus 4.5".into()),
                    context_window: Some(200000),
                    max_tokens: Some(32000),
                    target_model_id: None,
                }],
                None,
            )
        };

        let render = |editing: bool| -> Vec<String> {
            let mut editor = make();
            editor.row = 0;
            editor.col = 1;
            editor.editing = editing;
            if editing {
                editor.buf = "Renamed Opus".into();
                editor.buf_cursor = editor.buf.chars().count();
            }
            let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
            let theme = Theme::for_app(AppId::Claude);
            terminal
                .draw(|f| draw_catalog(f, &editor, f.area(), theme))
                .unwrap();
            (0..12).map(|y| row(&terminal, y)).collect()
        };

        let idle = render(false);
        let editing = render(true);

        // The context-window value must sit at the exact same column whether
        // the label cell next to it is idle or being edited.
        let dump = |lines: &Vec<String>| {
            lines
                .iter()
                .map(|l| format!("{:?}", l))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let idle_x = idle
            .iter()
            .find_map(|l| l.find("200000"))
            .unwrap_or_else(|| panic!("ctx in idle:\n{}", dump(&idle)));
        let edit_x = editing
            .iter()
            .find_map(|l| l.find("200000"))
            .unwrap_or_else(|| panic!("ctx in editing:\n{}", dump(&editing)));
        assert_eq!(idle_x, edit_x, "columns shifted on entering edit mode");

        // Header and data rows share the same grid.
        let header_line = idle.iter().find(|l| l.contains("Context")).unwrap();
        let header_x = header_line.find("Context").unwrap();
        assert_eq!(idle_x, header_x, "data cell not under its header");
    }

    use super::*;
    use crate::store::AppId;
    use ratatui::{backend::TestBackend, Terminal};

    fn row(term: &Terminal<TestBackend>, y: u16) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn picker_draws_one_page_with_filter_caret() {
        let mut picker = ModelPicker::with_preselect(
            PickerKind::Catalog,
            (0..30).map(|i| format!("m{i:02}")).collect(),
            &[],
        );
        picker.filter = "m0".into();
        picker.filter_cursor = 1;
        picker.filtering = true;
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| draw_picker(f, &mut picker, f.area(), Theme::for_app(AppId::Claude)))
            .unwrap();
        assert_eq!(picker.page_rows, 5); // popup inner 6 - filter row
        let joined: String = (0..10)
            .map(|y| row(&term, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("/ m0")); // filter text intact; caret is an underline style now
        assert!(!joined.contains('_')); // no inserted caret glyph anywhere
        assert!(joined.contains(">[ ] m00")); // cursor row, empty checkbox
        assert!(!joined.contains("[x]")); // the check is a glyph, not a letter
        assert!(!joined.contains("m05")); // second page stays hidden
        assert!(joined.contains("1/10")); // title position

        picker.selected.insert(0);
        term.draw(|f| draw_picker(f, &mut picker, f.area(), Theme::for_app(AppId::Claude)))
            .unwrap();
        let joined: String = (0..10)
            .map(|y| row(&term, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains(">[✓] m00")); // selected row carries the check
    }
}
