//! UI strings. English is the default. Chinese is a translation.
//!
//! Resolution: `--lang` / `APMUX_LANG` > `LC_ALL`/`LANG` (zh* → Chinese) > English.
//! CLI clap help stays English. TUI and TUI-driven prompts use this table.

use std::cell::Cell;
use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Zh,
}

thread_local! {
    static LANG: Cell<Lang> = const { Cell::new(Lang::En) };
}

pub fn lang() -> Lang {
    LANG.with(Cell::get)
}

pub fn set(lang: Lang) {
    LANG.with(|c| c.set(lang));
}

/// Apply an explicit value (`en`/`zh`) or fall back to the process locale.
pub fn init(explicit: Option<&str>) {
    init_chain(explicit, None);
}

/// TUI: `--lang` / `APMUX_LANG` > saved settings > locale > English.
pub fn init_tui(explicit: Option<&str>, saved: Option<&str>) {
    init_chain(explicit, saved);
}

pub fn parse_tag(raw: &str) -> Option<Lang> {
    parse(Some(raw))
}

fn init_chain(explicit: Option<&str>, saved: Option<&str>) {
    if let Some(l) = parse(explicit) {
        set(l);
        return;
    }
    if let Some(l) = parse(crate::name::read_env(crate::name::ENV_LANG).as_deref()) {
        set(l);
        return;
    }
    if let Some(l) = parse(saved) {
        set(l);
        return;
    }
    set(from_locale());
}

fn parse(raw: Option<&str>) -> Option<Lang> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    let primary = lower.split(['-', '_', '.']).next().unwrap_or(&lower);
    match primary {
        "zh" | "cn" | "zh-cn" | "zh-hans" => Some(Lang::Zh),
        "en" | "c" | "posix" => Some(Lang::En),
        _ if primary.starts_with("zh") => Some(Lang::Zh),
        _ if primary.starts_with("en") => Some(Lang::En),
        _ => None,
    }
}

fn from_locale() -> Lang {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(lang) = parse(env::var(var).ok().as_deref()) {
            return lang;
        }
    }
    Lang::En
}

pub fn t(key: &'static str) -> &'static str {
    match lang() {
        Lang::En => en(key),
        Lang::Zh => zh(key).unwrap_or_else(|| en(key)),
    }
}

pub fn tf(key: &'static str, args: &[&str]) -> String {
    let mut s = t(key).to_string();
    for (i, arg) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), arg);
    }
    s
}

fn en(key: &'static str) -> &'static str {
    match key {
        "ui.apps" => "Apps",
        "ui.providers" => "Providers",
        "ui.providers_title" => "Providers — {0}",
        "ui.data" => "Data",
        "ui.backups" => "Backups",
        "ui.sync" => "Sync",
        "ui.settings" => "Settings",
        "ui.status" => "Status",
        "ui.keys" => "Keys",
        "ui.help" => "Help",
        "ui.delete" => "Delete",
        "ui.restore" => "Restore",
        "ui.syncing" => "Syncing…",
        "ui.sync_push" => "Push",
        "ui.sync_pull" => "Pull",
        "ui.sync_never" => "never",
        "ui.timestamp" => "timestamp",
        "ui.named" => "named",
        "ui.webdav_unconfigured" => "WebDAV is not configured. Press e to open setup.",
        "ui.gist_unconfigured" => "Gist sync is not configured. Press e to add a token.",
        "ui.form_hint" => "Tab / ↑↓ fields  Space cycle/fetch/open  Enter submit  Esc cancel",
        "ui.confirm_hint" => "y confirm  n/Esc cancel",
        "ui.keep_previous" => "(keep current)",
        "ui.namespace" => "Namespace",
        "ui.username" => "Username",
        "ui.password" => "Password",
        "ui.token" => "Token",
        "ui.gist_id" => "Gist ID",
        "ui.gist_setup" => "Gist setup",
        "ui.backend_webdav" => "WebDAV",
        "ui.backend_gist" => "Gist",
        "ui.add_provider" => "Add provider",
        "ui.edit_provider" => "Edit provider",
        "ui.sync_setup" => "Sync setup",
        "ui.models" => "Models",
        "ui.fetching_models" => "Fetching models…",
        "ui.model_picker" => "Model list",
        "ui.catalog" => "Model catalog",
        "ui.slots" => "Model slots",
        "ui.snippet" => "Common snippet",

        "field.name" => "Name",
        "field.base_url" => "Base URL",
        "field.api_key" => "API Key",
        "field.model" => "Model",
        "field.api_key_field" => "Key field",
        "field.protocol" => "Protocol",
        "field.wire_api" => "Wire API",
        "field.npm" => "SDK package",
        "field.api" => "API type",
        "field.label" => "Label",
        "field.context_window" => "Context",
        "field.max_tokens" => "Max tokens",
        "field.slot_assignment" => "Slots",
        "field.target_model_id" => "Target model id",
        "field.apply_snippet" => "Apply snippet",
        "field.snippet" => "Snippet",
        "field.catalog" => "Catalog",
        "field.slots" => "Slots",
        "field.models_empty" => "empty",

        "quick.builtin" => "Built-in (quick config)",
        "quick.json" => "JSON",
        "quick.toml" => "TOML",
        "quick.edit_json" => "edit",
        "quick.hide_attribution" => "Hide AI attribution",
        "quick.teammates" => "Teammates",
        "quick.tool_search" => "Tool Search",
        "quick.effort_max" => "Effort max",
        "quick.disable_autoupdate" => "Disable auto-upgrade",
        "quick.unknown_model_reactive" => "Unknown model: wait for API",
        "quick.goal_mode" => "Goal mode",
        "quick.sandbox_network" => "Sandbox network access",
        "quick.remote_compaction" => "Remote compaction",

        "slot.default" => "Default",
        "slot.haiku" => "Haiku",
        "slot.sonnet" => "Sonnet",
        "slot.opus" => "Opus",
        "slot.fable" => "Fable",
        "slot.subagent" => "Subagent",

        "hint.switch_app" => "switch app",
        "hint.move" => "move",
        "hint.select" => "select",
        "hint.use" => "use",
        "hint.add" => "add",
        "hint.edit" => "edit",
        "hint.delete" => "delete (asks first)",
        "hint.data" => "data",
        "hint.settings" => "settings",
        "hint.help" => "help",
        "hint.toggle" => "change value",
        "hint.snapshot" => "snapshot",
        "hint.setup" => "sync setup",
        "hint.sync_tab" => "switch WebDAV/Gist",
        "hint.push" => "push",
        "hint.pull" => "pull",
        "hint.restore" => "restore (asks first)",
        "hint.back" => "back",
        "hint.speed_test" => "test latency",
        "hint.try" => "trial run",
        "status.testing" => "Testing {0}…",
        "status.test_ok" => "{0}: {1} ms (HTTP {2})",
        "status.test_err" => "{0} unreachable — {1}",
        "status.try_failed" => "Trial failed: {0}",
        "status.try_starting" => "Launching trial of {0}… (the CLI takes over this terminal)",
        "status.test_no_endpoint" => "no endpoint to test",
        "hint.quit" => "quit",
        "help.keys_title" => "Keys",
        "help.data_footnote" => "Files go under the built-in namespace apmux-sync (shown, not editable).",
        "help.settings_footnote" => "Auto detection looks for the CLI on PATH, a config folder, or providers already saved in apmux. Manual mode shows or hides each app; at least one stays visible.",
        "status.hint_picker" => "j/k move  PgUp/PgDn page  Space toggle  / filter  ←→ cursor  Enter confirm  Esc cancel",
        "status.hint_catalog" => "j/k row  Tab column  e edit  n new  d delete  * default  Enter save  Esc cancel",
        "status.hint_catalog_popover" => "j/k move  Space toggle  Enter done  Esc cancel",
        "status.hint_slots" => "j/k  e edit  Space fetch  a copy to all  Enter save  Esc cancel",
        "status.hint_snippet" => "j/k checkboxes  Space toggle  Tab body  Ctrl+S save  Esc cancel",
        "status.snippet_saved" => "Snippet saved",
        "status.hint_help" => "? or Esc close",
        "status.hint_syncing" => "Working…  q quit",
        "status.cancelled" => "Cancelled",
        "status.catalog_row_dropped_one_slot" => "Row deleted: cleared 1 slot binding.",
        "status.catalog_row_dropped_n_slots" => "Row deleted: cleared {0} slot bindings.",
        "status.catalog_default_moved" => "Default row deleted; new default is {0}.",
        "status.catalog_default_removed" => "Default row deleted; catalog is now empty.",
        "status.switch_failed" => "Switch failed: {0}",
        "status.no_switch" => "Select a provider first, or press a to add one.",
        "status.switched_skip" => "Using {0}. {1} has no config folder yet, so nothing was written to disk.",
        "status.skip_uninitialized" => "{0} has no config folder yet, so nothing was written to disk.",
        "status.try_done" => "Trial of {0} finished (exit {1}) — live configs untouched.",
        "status.switched" => "Using {0}",
        "status.restart_short" => "restart to apply",
        "status.restart_long" => "{0} reads its config at startup — restart it (or start a new session) to pick up the new provider.",
        "status.reloaded_short" => "app-server restarted",
        "status.reloaded_long" => "Restarted the Codex app-server daemon: already-open Codex sessions pick up the new provider now.",
        "status.reload_failed_short" => "app-server restart failed — restart Codex by hand",
        "status.reload_failed_long" => "Couldn’t restart the Codex app-server daemon ({0}) — restart Codex to pick up the new provider.",
        "status.no_edit" => "No provider to edit yet. Press a to add one.",
        "status.no_delete" => "No provider to delete.",
        "status.official_protected" => "'{0}' is built-in (official subscription); switch with Enter — it cannot be edited or deleted.",
        "list.official" => "(official)",
        "status.backed_up" => "Backed up {0}",
        "status.backup_failed" => "Backup failed: {0}",
        "status.no_restore" => "Pick a backup first.",
        "status.deleted" => "Deleted {0}",
        "status.delete_failed" => "Delete failed: {0}",
        "status.added" => "Added {0}",
        "status.updated" => "Updated {0}",
        "status.sync_unconfigured" => "Sync isn't set up yet. Press e to add WebDAV.",
        "status.sync_interrupted" => "Sync interrupted",
        "status.sync_configured" => "Sync configured",
        "status.gist_configured" => "Gist configured: {0}",
        "status.gist_unconfigured" => "Gist sync isn't set up yet. Press e to add a token.",
        "status.setup_failed" => "Setup failed: {0}",
        "status.pushed" => "Pushed {0}",
        "status.push_failed" => "Push failed: {0}",
        "status.pulled" => "Pulled {0}",
        "status.pull_failed" => "Pull failed: {0}",
        "status.reload_failed" => "Couldn’t reload saved providers: {0}",
        "status.restored" => "Restored {0}",
        "status.restore_failed" => "Restore failed: {0}",

        "confirm.delete" => "Delete provider {0} ({1})?",
        "confirm.restore" => "Restore from backup {0}? This replaces your current provider list.",
        "confirm.sync_push" => "Push local store to {0}? Last sync: {1}.",
        "confirm.sync_pull" => "Pull remote store from {0}? Last sync: {1}.",

        "form.required" => "{0} must not be empty",
        "form.invalid" => "Invalid value for {0}",
        "form.url_empty" => "URL must not be empty",
        "form.user_empty" => "Username must not be empty",
        "form.pass_empty" => "Password must not be empty",
        "form.token_empty" => "Token must not be empty",

        "settings.language" => "Language",
        "settings.apps_mode" => "App detection",
        "settings.mode_auto" => "auto",
        "settings.mode_manual" => "manual",
        "settings.detected" => "detected",
        "settings.hidden" => "not detected",
        "settings.on" => "on",
        "settings.off" => "off",

        "help.syncing" => {
            "Syncing\n\nWait for this to finish, or press q to quit apmux."
        }
        "help.confirm" => "Confirm\n\ny            yes\nn or Esc     no",
        "help.list" => "\
Keys\n\n\
[ ] or Tab           previous / next app\n\
j k or ↑ ↓           move in the list\n\
Enter                use this provider\n\
a                    add\n\
e                    edit\n\
d                    delete (asks first)\n\
r or s               data (backups & sync)\n\
g                    settings\n\
?                    this help\n\
q                    quit\n\
Esc                  close this help, or quit",
        "help.data" => "\
Data — backups & sync\n\
Backups:\n\
j k or ↑ ↓           move\n\
Enter                restore (asks first)\n\
b                    snapshot now\n\
Sync (Tab switches WebDAV / Gist):\n\
e                    set up the active backend\n\
p                    push\n\
u                    pull\n\
Esc                  back to the list\n\
q                    quit\n\
\n\
WebDAV files go under the built-in namespace apmux-sync (shown, not editable).\n\
Gist keeps the same files in one secret gist; it never pushes on setup.",
        "help.settings" => "\
Settings\n\n\
j k or ↑ ↓           move\n\
Space or Enter       change the value\n\
Esc                  back to the list\n\
q                    quit\n\n\
Auto detection looks for the CLI on PATH,\n\
a config folder, or providers already saved in apmux.\n\
Manual lets you show or hide each app. At least one stays visible.",
        "help.sync_setup" => "\
Sync setup\n\n\
Tab / ↑ ↓            move between fields\n\
Enter                submit (creates the apmux-sync folder under this URL)\n\
Esc                  cancel\n\n\
Leave the password empty to keep the current secret.\n\
Namespace apmux-sync is built-in and cannot be edited.",
        "help.gist_setup" => "\
Gist setup\n\n\
Tab / ↑ ↓            move between fields\n\
Enter                submit (finds or creates your secret sync gist)\n\
Esc                  cancel\n\n\
The token needs Gists read/write access.\n\
Leave the token empty to keep the stored one.\n\
Gist ID is optional: it pins an existing gist\n\
(id or URL) and skips the marker search.",
        "help.form" => "\
Form\n\n\
Tab / ↑ ↓            move between fields\n\
Space                cycle options, fetch models, or open catalog/slots/snippet\n\
Enter                submit the form (Space opens sub-editors)\n\
Esc                  cancel\n\n\
{0}\
Leave an optional model empty to clear it.\n\
A required model can’t be empty.",
        "help.form_keep" => "Leave the secret empty to keep the current value.\n",
        "help.picker" => "\
Model list\n\n\
j k or ↑ ↓           move\n\
PgUp PgDn            page (no wrap)\n\
Space                toggle (catalog)\n\
/                    filter (←→ Home End edit)\n\
Enter                confirm\n\
Esc                  cancel",
        "help.catalog" => "\
Catalog\n\n\
j k                  row\n\
Tab                  column\n\
e                    edit cell\n\
n                    new row\n\
d                    delete row\n\
*                    mark default\n\
Enter                save\n\
Esc                  cancel",
        "help.slots" => "\
Slots\n\n\
j k                  move\n\
e                    edit\n\
Space                fetch models\n\
a                    copy this id to all slots\n\
Enter                save\n\
Esc                  cancel",
        "help.snippet" => "\
Common snippet\n\n\
Built-in checkboxes compose the config body.\n\
j k                  move among checkboxes\n\
Space                toggle a built-in item\n\
Tab                  edit the body (JSON, or TOML for Codex)\n\
Ctrl+S               save\n\
Esc                  cancel",

        _ => key,
    }
}

fn zh(key: &'static str) -> Option<&'static str> {
    Some(match key {
        "ui.apps" => "应用",
        "ui.providers" => "供应商",
        "ui.providers_title" => "供应商 — {0}",
        "ui.data" => "数据管理",
        "ui.backups" => "备份",
        "ui.sync" => "同步",
        "ui.settings" => "设置",
        "ui.status" => "状态",
        "ui.keys" => "按键",
        "ui.help" => "帮助",
        "ui.delete" => "删除",
        "ui.restore" => "恢复",
        "ui.syncing" => "同步中…",
        "ui.sync_push" => "推送",
        "ui.sync_pull" => "拉取",
        "ui.sync_never" => "从未",
        "ui.timestamp" => "时间戳",
        "ui.named" => "命名",
        "ui.webdav_unconfigured" => "尚未配置 WebDAV。按 e 打开设置。",
        "ui.gist_unconfigured" => "尚未配置 Gist 同步。按 e 填写 token。",
        "ui.form_hint" => "Tab / ↑↓ 换字段  空格 切换/拉取/打开  Enter 提交  Esc 取消",
        "ui.confirm_hint" => "y 确认  n/Esc 取消",
        "ui.keep_previous" => "（保留原值）",
        "ui.namespace" => "命名空间",
        "ui.username" => "用户名",
        "ui.password" => "密码",
        "ui.token" => "Token",
        "ui.gist_id" => "Gist ID",
        "ui.gist_setup" => "Gist 设置",
        "ui.backend_webdav" => "WebDAV",
        "ui.backend_gist" => "Gist",
        "ui.add_provider" => "添加供应商",
        "ui.edit_provider" => "编辑供应商",
        "ui.sync_setup" => "同步设置",
        "ui.models" => "模型",
        "ui.fetching_models" => "正在获取模型…",
        "ui.model_picker" => "模型列表",
        "ui.catalog" => "模型目录",
        "ui.slots" => "模型档位",
        "ui.snippet" => "公共配置片段",

        "field.name" => "名称",
        "field.base_url" => "Base URL",
        "field.api_key" => "API Key",
        "field.model" => "模型",
        "field.api_key_field" => "密钥字段",
        "field.protocol" => "协议",
        "field.wire_api" => "Wire API",
        "field.npm" => "SDK 包",
        "field.api" => "API 类型",
        "field.label" => "显示名",
        "field.context_window" => "上下文",
        "field.max_tokens" => "最大输出",
        "field.slot_assignment" => "角色",
        "field.target_model_id" => "目标模型 ID",
        "field.apply_snippet" => "应用公共配置",
        "field.snippet" => "公共配置",
        "field.catalog" => "模型目录",
        "field.slots" => "模型档位",
        "field.models_empty" => "空",

        "quick.builtin" => "内置（快捷配置）",
        "quick.json" => "JSON",
        "quick.toml" => "TOML",
        "quick.edit_json" => "编辑",
        "quick.hide_attribution" => "隐藏 AI 署名",
        "quick.teammates" => "Teammates",
        "quick.tool_search" => "Tool Search",
        "quick.effort_max" => "思考强度最大",
        "quick.disable_autoupdate" => "禁用自动升级",
        "quick.unknown_model_reactive" => "未知模型：等待 API 报错",
        "quick.goal_mode" => "Goal mode",
        "quick.sandbox_network" => "沙箱网络访问",
        "quick.remote_compaction" => "远程压缩",

        "slot.default" => "默认",
        "slot.haiku" => "Haiku",
        "slot.sonnet" => "Sonnet",
        "slot.opus" => "Opus",
        "slot.fable" => "Fable",
        "slot.subagent" => "子代理",

        "hint.switch_app" => "切换应用",
        "hint.move" => "移动",
        "hint.select" => "选择",
        "hint.use" => "启用",
        "hint.add" => "添加",
        "hint.edit" => "编辑",
        "hint.delete" => "删除（会再确认）",
        "hint.data" => "数据管理",
        "hint.settings" => "设置",
        "hint.help" => "帮助",
        "hint.toggle" => "更改",
        "hint.snapshot" => "快照",
        "hint.setup" => "同步设置",
        "hint.sync_tab" => "切换 WebDAV/Gist",
        "hint.push" => "推送",
        "hint.pull" => "拉取",
        "hint.restore" => "恢复（会再确认）",
        "hint.back" => "返回",
        "hint.speed_test" => "测延迟",
        "hint.try" => "试驾",
        "status.testing" => "正在测试 {0}…",
        "status.test_ok" => "{0}:{1} 毫秒（HTTP {2}）",
        "status.test_err" => "{0} 不可达——{1}",
        "status.try_failed" => "试驾失败：{0}",
        "status.try_starting" => "正在启动 {0} 试驾……（CLI 将接管当前终端）",
        "status.test_no_endpoint" => "没有可测试的端点",
        "hint.quit" => "退出",
        "help.keys_title" => "快捷键",
        "help.data_footnote" => "WebDAV 文件写在内置命名空间 apmux-sync 下（只显示，不能改）。Gist 把同样的文件放进一个秘密 gist。",
        "help.settings_footnote" => "自动检测会找 PATH 上的 CLI、配置目录，以及 apmux 里已经存过的供应商。手动模式可以逐个显示或隐藏，至少留一个。",
        "status.hint_picker" => {
            "j/k 移动  PgUp/PgDn 翻页  空格 勾选  / 过滤  ←→ 光标  Enter 确认  Esc 取消"
        }
        "status.hint_catalog" => {
            "j/k 行  Tab 列  e 编辑  n 新增  d 删除  * 默认  Enter 保存  Esc 取消"
        }
        "status.hint_catalog_popover" => "j/k 移动  空格 勾选  Enter 完成  Esc 取消",
        "status.hint_slots" => "j/k  e 编辑  空格 拉取  a 同步到全部档位  Enter 保存  Esc 取消",
        "status.hint_snippet" => "j/k 勾选  空格 切换  Tab 正文  Ctrl+S 保存  Esc 取消",
        "status.snippet_saved" => "公共配置已保存",
        "status.hint_help" => "? 或 Esc 关闭",
        "status.hint_syncing" => "处理中…  q 退出",
        "status.cancelled" => "已取消",
        "status.catalog_row_dropped_one_slot" => "已删行：清理 1 个 slot 绑定。",
        "status.catalog_row_dropped_n_slots" => "已删行：清理 {0} 个 slot 绑定。",
        "status.catalog_default_moved" => "已删默认行：新默认是 {0}。",
        "status.catalog_default_removed" => "已删默认行：目录已空。",
        "status.switch_failed" => "切换失败: {0}",
        "status.no_switch" => "先选一个供应商，或按 a 添加。",
        "status.switched_skip" => "已选用 {0}。{1} 还没有配置目录，所以没有改任何文件。",
        "status.skip_uninitialized" => "{0} 还没有配置目录，已跳过写入。",
        "status.try_done" => "试驾 {0} 已结束（退出码 {1}）——线上配置未改动。",
        "status.switched" => "已选用 {0}",
        "status.restart_short" => "重启生效",
        "status.restart_long" => "{0} 在启动时读取配置——重启后才会使用新的供应商。",
        "status.reloaded_short" => "后台服务已重启",
        "status.reloaded_long" => "已重启 Codex 后台服务（app-server）：已打开的 Codex 会话现在就能用上新的供应商。",
        "status.reload_failed_short" => "后台服务重启失败——请手动重启 Codex",
        "status.reload_failed_long" => "Codex 后台服务（app-server）重启失败（{0}）——请手动重启 Codex 才能用上新的供应商。",
        "status.no_edit" => "还没有可编辑的供应商。按 a 添加。",
        "status.no_delete" => "没有可删除的供应商。",
        "status.official_protected" => "“{0}”是内置官方订阅行，按 Enter 即可切换；不可编辑或删除。",
        "list.official" => "（官方）",
        "status.backed_up" => "已备份 {0}",
        "status.backup_failed" => "备份失败: {0}",
        "status.no_restore" => "先选一个备份。",
        "status.deleted" => "已删除 {0}",
        "status.delete_failed" => "删除失败: {0}",
        "status.added" => "已添加 {0}",
        "status.updated" => "已更新 {0}",
        "status.sync_unconfigured" => "还没配置同步。按 e 填写 WebDAV。",
        "status.sync_interrupted" => "同步中断",
        "status.sync_configured" => "同步已配置",
        "status.gist_configured" => "Gist 已配置: {0}",
        "status.gist_unconfigured" => "还没配置 Gist 同步。按 e 填写 token。",
        "status.setup_failed" => "设置失败: {0}",
        "status.pushed" => "已推送 {0}",
        "status.push_failed" => "推送失败: {0}",
        "status.pulled" => "已拉取 {0}",
        "status.pull_failed" => "拉取失败: {0}",
        "status.reload_failed" => "重新加载供应商列表失败: {0}",
        "status.restored" => "已恢复 {0}",
        "status.restore_failed" => "恢复失败: {0}",

        "confirm.delete" => "删除供应商 {0}（{1}）？",
        "confirm.restore" => "从备份 {0} 恢复？会替换当前的供应商列表。",
        "confirm.sync_push" => "推送本地 store 到 {0}？上次同步：{1}。",
        "confirm.sync_pull" => "从 {0} 拉取远程 store？上次同步：{1}。",

        "form.required" => "{0} 不能为空",
        "form.invalid" => "{0} 取值无效",
        "form.url_empty" => "URL 不能为空",
        "form.user_empty" => "用户名不能为空",
        "form.pass_empty" => "密码不能为空",
        "form.token_empty" => "Token 不能为空",

        "settings.language" => "语言",
        "settings.apps_mode" => "应用检测",
        "settings.mode_auto" => "自动",
        "settings.mode_manual" => "手动",
        "settings.detected" => "已检测到",
        "settings.hidden" => "未检测到",
        "settings.on" => "显示",
        "settings.off" => "隐藏",

        "help.syncing" => "正在同步\n\n等它完成，或按 q 退出 apmux。",
        "help.confirm" => "确认\n\ny            是\nn 或 Esc     否",
        "help.sync_setup" => {
            "\
同步设置\n\n\
Tab / ↑ ↓            字段间移动\n\
Enter                提交（在此 URL 下创建 apmux-sync 目录）\n\
Esc                  取消\n\n\
密钥留空 = 保留原密码。\n\
命名空间 apmux-sync 是内置的，不能改。"
        }
        "help.gist_setup" => {
            "\
Gist 设置\n\n\
Tab / ↑ ↓            字段间移动\n\
Enter                提交（查找或创建你的秘密同步 gist）\n\
Esc                  取消\n\n\
Token 需要 Gists 读写权限。\n\
Token 留空 = 保留已存的 token。\n\
Gist ID 可选：填了就固定用这个 gist\n\
（支持 id 或 URL），跳过标记搜索。"
        }
        "help.form" => {
            "\
表单\n\n\
Tab / ↑ ↓            字段间移动\n\
空格                 切换选项、拉取模型、或打开目录/档位/公共配置\n\
Enter                提交表单（空格打开子编辑器）\n\
Esc                  取消\n\n\
{0}\
可选模型留空会清掉模型。\n\
必填模型不能为空。"
        }
        "help.form_keep" => "密钥留空会保留原来的值。\n",
        "help.picker" => {
            "\
模型列表\n\n\
j k 或 ↑ ↓           移动\n\
PgUp PgDn            翻页（不循环）\n\
空格                 勾选（目录）\n\
/                    过滤（←→ Home End 编辑）\n\
Enter                确认\n\
Esc                  取消"
        }
        "help.catalog" => {
            "\
模型目录\n\n\
j k                  行\n\
Tab                  列\n\
e                    编辑单元格\n\
n                    新增行\n\
d                    删除行\n\
*                    标为默认\n\
Enter                保存\n\
Esc                  取消"
        }
        "help.slots" => {
            "\
模型档位\n\n\
j k                  移动\n\
e                    编辑\n\
空格                 拉取模型\n\
a                    把当前 id 复制到全部档位\n\
Enter                保存\n\
Esc                  取消"
        }
        "help.snippet" => {
            "\
公共配置片段\n\n\
上面的勾选是内置快捷项，会写进下面的正文。\n\
j k                  在勾选项间移动\n\
空格                 开关内置项\n\
Tab                  编辑正文（JSON，Codex 为 TOML）\n\
Ctrl+S               保存\n\
Esc                  取消"
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_english() {
        set(Lang::En);
        assert_eq!(t("ui.sync"), "Sync");
        assert_eq!(t("ui.keys"), "Keys");
        assert_eq!(tf("status.switched", &["packy"]), "Using packy");
    }

    #[test]
    fn zh_falls_back_to_en_for_unknown_keys() {
        set(Lang::Zh);
        assert_eq!(t("ui.sync"), "同步");
        assert_eq!(t("this.key.does.not.exist"), "this.key.does.not.exist");
        set(Lang::En);
    }

    #[test]
    fn parse_locale_tags() {
        assert_eq!(parse(Some("zh_CN.UTF-8")), Some(Lang::Zh));
        assert_eq!(parse(Some("zh-Hans")), Some(Lang::Zh));
        assert_eq!(parse(Some("en_US")), Some(Lang::En));
        assert_eq!(parse(Some("C")), Some(Lang::En));
        assert_eq!(parse(Some("de_DE")), None);
    }
}
