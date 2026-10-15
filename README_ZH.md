# apmux

[English](README.md) 是规范语言。本页为中文译本。

轻量本地工具：在 **Claude Code / Codex / OpenCode / Pi Coding Agent** 之间切换供应商，并做本地轮转备份与 WebDAV 或 GitHub Gist 云备份。无 GUI。无参数启动 TUI，子命令给脚本。

不做 Proxy、MCP 管理、Skills、会话/用量、OAuth 账号、daemon、Gemini CLI。

## 安装

Linux / macOS：

```bash
curl -fsSL https://github.com/iam2r/apmux/releases/latest/download/install.sh | bash
```

Windows（PowerShell）：

```powershell
irm https://github.com/iam2r/apmux/releases/latest/download/install.ps1 | iex
```

Unix 安装到 `~/.local/bin`（可用 `APMUX_INSTALL_DIR` 覆盖）。如果这个目录不在 `PATH` 里，脚本会往 `~/.bashrc` / `~/.zshrc` / fish 配置写入一段受管 PATH。Windows 安装到 `%LOCALAPPDATA%\apmux\bin` 并写入用户 `Path`。Linux 用静态 musl 构建。装好后可用 `apmux update` 从 GitHub Releases 替换当前二进制（`apmux update --check` 只检查）。

```bash
# 指定版本
curl -fsSL https://github.com/iam2r/apmux/releases/latest/download/install.sh | bash -s -- v0.1.0
APMUX_SKIP_PATH=1 bash install.sh   # 只安装，不改 shell rc
```

### 发布

发布由维护者驱动，仍基于 **changeset 文件**（工具为 [Knope](https://knope.tech)），不解析提交消息：

1. 所有开发都在 `develop` 分支（`main` 仅用于发布）。贡献者**不需要**添加变更文件——是否发版、写什么进 changelog 由维护者决定，在 `develop` 上添加 `.changeset/<name>.md`：

   ```markdown
   ---
   apmux: minor        # minor | patch | major
   ---
   写进 changelog 的一句话。
   ```

2. 把变更文件推到 `develop` 后，CI 消费这些文件、开 **Release PR**（`release` → `main`，改 `Cargo.toml` 和 `CHANGELOG.md`）。
3. 用 **merge commit** 合并（不要 squash / rebase）。push 到 `main` 时，若 `apmux/vX.Y.Z` 尚不存在则打 tag、构建安装包，并把 `main` 回合并到 `develop`。本地无需安装任何工具。

从源码（Rust stable）：

```bash
git clone https://github.com/iam2r/apmux.git
cd apmux
cargo install --path .
# 或: cargo build --release   → target/release/apmux
```

配置目录默认 `$HOME/.apmux`（`store.json`、`webdav.json`、`gist.json`、`backups/`）。可用 `APMUX_CONFIG_DIR` 覆盖。

## 从 cc-switch 迁移

`apmux import` 把 Claude / Codex / OpenCode 供应商从 `~/.cc-switch/cc-switch.db` 拷进 `store.json`，并把 WebDAV 账号从 `~/.cc-switch/settings.json` 写进 `webdav.json`（只拷 `baseUrl`；忽略 cc-switch 的 `remoteRoot` / `cc-switch-sync`）。跳过 Gemini、Grok、空的官方项。**不**写各 CLI 的 live 文件，也不 MKCOL / 拉取远端快照。store 里已有的 Pi 等其它 app 会保留。已有的 `webdav.json` 默认不动，加 `--force` 才覆盖。

```bash
apmux import --dry-run
apmux import
apmux list
apmux use <name>
```

默认是 merge（跳过已有 id）。`--force` 覆盖冲突 id、已导入 app 的 `current`，以及 `webdav.json`（先做时间戳备份）。报告里的密钥是掩码。

## 用法

```bash
apmux                              # TUI
apmux list [--app <app>] [--json]
apmux current [--app <app>] [--json]
apmux use <name> [--app <app>]
apmux add --app <app> --name <name> --base-url <url> --api-key <key> \
        [--model <id>] [--extra key=value]... [--apply-snippet]
apmux edit <name> [--app] [--name] [--base-url] [--api-key] \
        [--model <id> | --clear-model] [--extra key=value]... \
        [--apply-snippet | --no-apply-snippet]
apmux snippet <name> [--app <app>] [--set '<json>' | --clear]
apmux delete <name> [--app] [--yes]
apmux backup [--name <name>]
apmux restore <name> [--yes] [--no-apply]
apmux backups
apmux sync setup --backend webdav --url <webdav-root> --username <user> --password <pass>
apmux sync setup --backend gist --token <github-token> [--gist <id或URL>]
apmux sync push [--backend webdav|gist] [--force]
apmux sync pull [--backend webdav|gist] [--force]
apmux sync status [--backend webdav|gist]
apmux import [--db <path>] [--settings <path>] [--dry-run] [--force]
apmux update [--version <tag>]
apmux update --check [--json]
```

`<app>`：`claude` / `codex` / `opencode` / `pi`。退出码：`0` 成功，`1` 用户/校验错误，`2` I/O 或网络。

`list` / `current` 默认掩码 API key（前 4 + `…` + 后 4；短于 8 则全 `*`）。`--json` 同样掩码。`APMUX_SHOW_SECRETS=1` 会打印完整密钥——**危险**，会进入终端滚动缓冲和 CI 日志，仅本地调试。

CLI 不弹交互提示（没有 dialoguer）。缺必填 flag 直接非 0 退出。人用 TUI。

TUI 默认英文。`--lang zh`、`APMUX_LANG=zh` 或系统 `LANG`/`LC_ALL` 为 `zh*` 时使用中文界面。clap `--help` 始终为英文。

## TUI 快捷键

无参数 `apmux` 进入 TUI。`?` 显示当前页帮助。

主列表：

| 键 | 动作 |
|----|------|
| `[` `]` 或 Tab | 上一个 / 下一个应用 |
| `j` `k` 或 ↑ ↓ | 在列表里上下移动 |
| `Enter` | 切换供应商 |
| `a` | 添加 |
| `e` | 编辑 |
| `d` | 删除（确认） |
| `b` | 立即备份（时间戳） |
| `r` | 备份页 |
| `s` | 同步页 |
| `g` | 设置（语言、自动/手动检测 agent） |
| `?` | 帮助 |
| `q` / `Esc` | 退出 / 关闭浮层 |

表单：`Tab` / ↑ ↓ 换字段，空格切换选项，**在 Model 字段上会拉取模型列表**，在 **模型目录 / 模型档位 / 公共配置** 上 Enter 或空格打开编辑器，其它字段 Enter 提交，Esc 取消。密钥字段掩码；编辑时留空 = 保留原值。可选模型留空会清掉模型；必填模型不能为空。

目录类应用（OpenCode、Pi、Codex）：在 Model 上空格拉取，空格勾选，Enter 进入目录编辑（`id` / `label` / `context_window` / `max_tokens`；Codex 没有 max_tokens 列）。Claude：表单的「模型档位」打开档位表；空格给当前档位选一个 id；`a` 把该 id 复制到其它档位（含默认）。

**公共配置**在添加/编辑表单里，是该供应商自己的 JSON。上面是内置勾选项（Claude 隐藏署名 / Teammates / Tool Search / 思考强度 / 禁用自动升级；Codex Goal mode），勾选会写进 JSON。表单里的「应用公共配置」，或 `apmux add/edit --apply-snippet`。片段先合并，自有字段后写，后者赢。`apmux snippet <name>` 查看/设置/清除该供应商的 JSON。

**内置官方行**（`claude-official`、`codex-official`）会自动就位：切换过去即交还给 CLI 的原生订阅登录（Claude Code 登录 / ChatGPT 登录）。这两行不可编辑或删除——选中后按 Enter 切换即可。

备份页：Enter 恢复（确认），`b` 立即备份，Esc 返回。

同步页：`e` 设置，`p` 推送，`u` 拉取。进行中显示静态「同步中…」（无动画）；此时除 `q` 外忽略键盘。

设置页（`g`）：Space/Enter 改当前行。应用检测默认**自动**：只有 PATH 上找得到 CLI 才算——残留的配置目录不算数。可改成**手动**逐个显示/隐藏。语言存在 `$APMUX_CONFIG_DIR/settings.json`。

## WebDAV

`--url` 是 WebDAV 根。文件始终写在内置命名空间 `apmux-sync` 下。TUI 单独显示这一行，不能改。

```bash
apmux sync setup \
  --url 'https://webdav.example.com/' \
  --username 'you' \
  --password '<密码>'
```

`--url` / `--username` / `--password` 始终必填。非 localhost 的 `http://` 会被拒绝。Setup 会 MKCOL `{url}/apmux-sync`，并保存**你提交的根 URL**。

TUI：`s` → `e`（Sync 面板 WebDAV 页），填 URL / 用户 / 密码。命名空间单独一行只读（`apmux-sync`）。

`push` / `pull` 覆盖 store 前会打一份时间戳备份。冲突时拒绝 push，用 `pull` 或 `--force`。`status` 永不打印密码。

凭证在 `$APMUX_CONFIG_DIR/webdav.json`，权限 `0600`。

## GitHub Gist

同一对 `store.json` + `manifest.json`，改存到**私有 gist** 而不是 WebDAV 目录。

```bash
apmux sync setup --backend gist --token '<github-token>' [--gist <id或URL>]
apmux sync push --backend gist [--force]
apmux sync pull --backend gist [--force]
apmux sync status --backend gist
```

`setup` 会创建一个以当前本地 store 为初始内容的 gist；或按 description 里的同步格式标记找到已有的 gist——换一台机器也能自动发现同一个 gist。`--gist <id或URL>` 直接指定某个 gist、跳过搜索。每个 `sync` 子命令的 `--backend` 都默认 `webdav`；push/pull/status 同样接受该参数，两个后端可以并存使用。

token 需要 **Gists 读写**权限（建议用只有这一项权限的 fine-grained PAT）。gist 一律创建为私密；`status` 永不打印 token。凭证在 `$APMUX_CONFIG_DIR/gist.json`，权限 `0600`。

TUI：Data 页的 Sync 面板有 WebDAV/Gist 两个 tab（`Tab` 切换）；`e` 设置当前 tab 对应的后端，`p` 推送，`u` 拉取。

`push` / `pull` 行为与 WebDAV 后端完全一致：覆盖前打时间戳备份、冲突拒绝 push、`--force` 强制覆盖。

## 项目 `opencode.json` 遮蔽

apmux **只写全局** live 文件：

| 应用 | 全局 live |
|------|-----------|
| Claude | `~/.claude/settings.json`（仅当目录已存在） |
| Codex | `~/.codex/config.toml` + `auth.json` |
| OpenCode | `~/.config/opencode/opencode.json` |
| Pi | `~/.pi/agent/models.json` + `settings.json` |

OpenCode 自身会优先读**项目目录**的 `opencode.json` / `opencode.jsonc`，从而遮蔽全局配置。apmux 不扫描、不改写项目文件。若某个仓库有自己的 `opencode.json`，在该仓库里跑 OpenCode 可能看不到 apmux 的切换。Pi 同理：不碰项目 `.pi/`。

目标 CLI 尚未初始化（解析后的配置目录不存在）时，apmux 会记下 `current`，但**不**创建该目录、不写 live。

## 不要并发写

v1 **没有** `store.json` 锁。两个 `apmux use` / `edit` / `delete` / `restore` / `sync push|pull` 并行会丢更新（原子 rename 只防半写）。单用户工具：不要同时跑两个会写 store 的 apmux 进程。

## 密钥与权限

- `store.json`、`webdav.json`、备份文件：每次保存后 Unix `chmod 0600`；apmux 目录 `0700`。
- 含密钥的 live 文件（Claude `settings.json`、Codex `auth.json`、OpenCode `opencode.json`、Pi `models.json`）：新文件 `0600`；已存在则保留原权限。
- TUI/CLI 默认掩码 API key。日志不记录 `api_key`、密码、Authorization 头。

## 测试隔离

测试一律注入 tempfile `Paths`，**禁止**写宿主机真实目录：

- `~/.claude`
- `~/.codex`
- `~/.config/opencode`
- `~/.pi`
- 以及真实 `~/.apmux`

`cargo test` 若 apply/写入会落到上述路径且 `Paths.home` 不是 tempfile，会 panic。不要用改进程 `HOME` 当主隔离手段。

```bash
cargo test
cargo fmt
cargo clippy --all-targets -- -D warnings
```

`0600` 权限断言仅 `#[cfg(unix)]`；Windows 上跳过 mode 检查。

## Windows

优先用上面的 PowerShell 一键安装。GitHub Actions:PR 走策略门禁 + `ubuntu-latest`（fmt + clippy + test）和 Windows（`cargo test`）;发布 bot 打 `apmux/vX.Y.Z` 标签并发布 Linux / macOS / Windows 的 Release(手工推 `v*` 标签也会触发同样的构建)。

Windows 上：

- 原子写仍是同目录 tmp + rename（覆盖已有目标）。
- `chmod 0600` / `0700` **失败则忽略**；v1 不做 Windows ACL。
- 路径通过注入的 `Paths`，不依赖进程 `HOME` / `USERPROFILE` 做测试隔离。

## 参与贡献

欢迎提交 issue 与 PR,流程见 [CONTRIBUTING.md](CONTRIBUTING.md)。所有 PR 目标分支为 `develop`,CI 自动运行检查,无需附带 change file。

## 许可证

MIT。
