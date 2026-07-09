# wfadm — WarpFusion Admin CLI

- 状态: Current
- 当前实现: `crates/wfadm`
- 更新时间: 2026-07-09

## 定位

`wfadm` 是 `wfusion` 项目的管理 CLI，负责项目初始化、配置对比、远端版本同步、项目校验、daemon 管理和自更新。它不再作为 `wfusion` / `wfgen` / `wfl` 的通用转发入口。

当前顶层命令:

```text
wfadm
├── init
├── conf
│   ├── diff
│   └── update
├── check
├── engine
│   ├── status
│   └── reload
└── self-update
```

## init

创建本地项目:

```bash
wfadm init --dir . --name my-rules --mode normal
```

`--mode`:

- `full`
- `normal`
- `rules`
- `conf`

从远端仓库 bootstrap:

```bash
wfadm init \
  --dir . \
  --repo https://github.com/example/project.git \
  --version 1.0.0
```

`--repo` 与 `--mode` 互斥。远端 bootstrap 会复用 project remote 同步与校验回滚流程。

## conf diff

比较两组 wfusion 配置:

```bash
wfadm conf diff \
  --config conf/wfusion.toml \
  --overlay conf/overlay.toml \
  --var KEY=VALUE \
  --to-config conf/new.toml \
  --to-overlay conf/new-overlay.toml \
  --to-var KEY=NEW \
  --path-prefix runtime \
  --expanded
```

用途:

- 比较 raw / resolved 配置差异。
- 支持 overlay、变量和 work dir。
- 可限制输出路径前缀。

## conf update

从 `[project_remote]` 同步 managed dirs:

```bash
wfadm conf update \
  --work-root . \
  --version 1.0.1 \
  --group models \
  --json
```

语义:

- 从 `<work-root>/conf/wfusion.toml` 读取 `[project_remote]`。
- single-repo 可不传 `--group`。
- dual-repo 使用 `--group models|infra`。
- 同步流程为 lock → snapshot → git sync → validate → rollback on failure。

输出字段:

- requested version
- current version
- resolved tag
- from / to revision
- changed

## check

验证项目完整性:

```bash
wfadm check --dir .
```

检查范围包括:

- wfusion 配置加载
- sources / connectors / sinks
- WFS schema
- WFL rule
- WFG scenario
- rule 与 window/schema 引用关系

## engine status

查询 daemon admin API:

```bash
wfadm engine status \
  --config conf/wfusion.toml
```

或显式指定 endpoint:

```bash
wfadm engine status \
  --admin-url http://127.0.0.1:19080 \
  --token-file runtime/admin_api.token \
  --json
```

默认从 `conf/wfusion.toml` 读取:

- `[admin_api].bind`
- `[admin_api.auth].token_file`

展示字段:

- instance id
- version
- accepting_commands
- reloading
- project_version

## engine reload

触发在线 reload / publish:

```bash
wfadm engine reload \
  --config conf/wfusion.toml \
  --wait true \
  --timeout-ms 15000
```

在线发布:

```bash
wfadm engine reload \
  --update \
  --version 1.0.1 \
  --group models \
  --wait false \
  --reason "release"
```

参数:

| 参数 | 说明 |
|------|------|
| `--admin-url` | 覆盖配置中的 admin API 地址 |
| `--token-file` | 覆盖配置中的 token 文件 |
| `--wait <true|false>` | 是否等待 reload 结果，默认 `true` |
| `--timeout-ms` | 等待超时，默认 `15000` |
| `--update` | reload 前执行 project remote sync |
| `--version` | update 目标版本，要求 `--update` |
| `--group` | dual-repo group，要求 `--update` |
| `--reason` | 写入 daemon 日志 |
| `--request-id` | 发送 `X-Request-Id` |
| `--json` | 原样输出 daemon JSON |

旧参数:

- `--update-remote` 已移除，使用 `--update`。
- `--full` 已移除，Admin API 不触发进程级重启。

## self-update

```bash
wfadm self-update
```

从 stable update manifest 下载并替换当前二进制。

## 设计约束

- CLI 参数名与 Admin API 协议保持一致。
- `engine reload` 不做本地 project remote sync；所有在线发布都由 daemon 持锁执行。
- `conf update` 和 `engine reload --update` 共享 `wf-project-remote` 的 sync / lock / rollback 语义。
- `engine reload --wait false` 必须能发送非阻塞请求，不能退化成 clap 无值 flag。

## 参考

- `crates/wfadm/src/main.rs`
- `crates/wfadm/src/conf.rs`
- `crates/wfadm/src/engine.rs`
- `docs/design/admin_api_reload_design.md`
- `docs/design/project_remote_alignment.md`
