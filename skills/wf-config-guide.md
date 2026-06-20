# AI Agent 指南：WarpFusion 配置

> 本文档供 AI agent（Claude / GPT / Cursor / Zed agent 等）在创建、修改或
> 排查 `wfusion.toml` 配置时遵循。涵盖正确的参数名、取值和常见陷阱。
>
> 适用版本：`wp-core-connectors` ≥ 0.5.2，`wf-runtime` ≥ 0.1.15。
>
> 人类可读的完整配置参考见 [`docs/configuration.md`](../configuration.md)。

## 核心规则

### 1. 永远用 `data_format`，不用 `format`

```toml
# ✅ 正确
data_format = "arrow_framed"

# ❌ 错误 — format 已废弃，不会被读取
format = "arrow_framed"
```

### 2. `port` 必须是字符串

```toml
# ✅ 正确
port = "9800"

# ❌ 错误 — 整数会导致 TOML 解析失败
port = 9800
```

### 3. TCP source 用 `addr` + `port`，不用 `listen`

```toml
# ✅ 正确 — connector-based TCP source
[[sources]]
type = "tcp"
addr = "127.0.0.1"
port = "9800"
framing = "len"
data_format = "arrow_framed"

# ❌ 错误 — 旧版 listen URL，connector 不读取
listen = "tcp://127.0.0.1:9800"
```

### 4. `stream` 的必填 / 可选规则

| `data_format` | `stream` 必填？ | 原因 |
|---|---|---|
| `ndjson` | **必填** | 纯文本，数据中无 stream 标识 |
| `arrow_ipc` | **必填** | 原始 IPC Stream，无 tag 头 |
| `arrow_framed` | **可选** | 帧头中的 tag 就是 stream 名；留空则用 tag |

```toml
# arrow_framed + stream 为空 → 用帧 tag 路由
stream = ""
data_format = "arrow_framed"

# ndjson → stream 必填
stream = "syslog"
data_format = "ndjson"
```

## data_format 语义

| 值 | 线格式 | stream 来源 | 典型发送方 |
|---|---|---|---|
| `ndjson` | JSON Lines 文本 | `stream` 参数（必填） | 手写 / wfgen |
| `arrow_ipc` | 原始 Arrow IPC Stream (schema + batch + EOS) | `stream` 参数（必填） | 第三方 Arrow 工具 |
| `arrow_framed` | wp_arrow 帧 `[4B tag_len][tag][Arrow IPC Stream]` | 帧内 tag，或 `stream` 覆盖 | **wparse** `encode_ipc(tag, batch)` |

## TCP source 完整参数

```toml
[[sources]]
type = "tcp"
name = "ingress"
addr = "127.0.0.1"            # 绑定地址，默认 0.0.0.0
port = "9800"                 # 端口（字符串），默认 9000
framing = "len"               # len | line | auto
data_format = "arrow_framed"  # ndjson | arrow_ipc | arrow_framed
stream = ""                   # 可选
```

`framing` 控制字节级分帧：
- `len` — RFC6587 octet-counting：`<len><SP><payload>`
- `line` — 按换行符分帧
- `auto` — 自动检测（优先尝试 len，回退 line）

## File source 完整参数

```toml
[[sources]]
type = "file"
name = "seed"
path = "data/events.ndjson"   # 相对 config dir 或 --work-dir
stream = "syslog"             # 路由 stream 名
data_format = "ndjson"        # ndjson | csv | arrow_framed | arrow_ipc
```

## 完整配置模板

```toml
mode = "daemon"                          # daemon | batch
sinks = "sinks"                          # sink 目录

[[sources]]
type = "tcp"
name = "ingress"
addr = "127.0.0.1"
port = "9800"
framing = "len"
data_format = "arrow_framed"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/*.wfs"                # glob，相对 config dir
rules   = "rules/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.auth_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"

[vars]
FAIL_THRESHOLD = "3"

[logging]
level = "info"
format = "plain"                         # plain | json
file = "logs/wfusion.log"
[logging.modules]
"wf_runtime::receiver" = "info"
```

## CLI 常用命令

```bash
# 启动引擎
wfusion run -c conf/wfusion.toml

# 叠加 overlay
wfusion run -c conf/wfusion.toml --overlay conf/local.toml

# 注入变量
wfusion run -c conf/wfusion.toml --var CASE_PATH=/data/case-a

# 覆盖工作目录
wfusion run -c conf/wfusion.toml --work-dir /path/to/project

# 检查配置（不启动）
wfusion config render -c conf/wfusion.toml
wfusion config vars -c conf/wfusion.toml
wfusion config diff -c base.toml --to-config overlay.toml
```

## 排查清单

当配置出错时，按以下顺序检查：

1. `data_format` 是否拼写正确？（不是 `format`）
2. `port` 是否是字符串？
3. TCP source 是否用了 `addr`+`port`？（不是 `listen`）
4. `stream` 是否填写？（`ndjson`/`arrow_ipc` 必填）
5. `arrow_framed` 的 tag 是否匹配 `.wfs` 中 window 的 `stream` 声明？
6. 路径是否相对 config dir？需要用 `${WORK_DIR}` 还是 `${CONFIG_DIR}`？
7. 运行 `wfusion config render -c <path>` 检查最终合并后的配置。
