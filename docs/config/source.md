# 数据源配置 (`[[sources]]`)

## TCP（实时）

```toml
[[sources]]
type = "tcp"
name = "netflow_input"   # 可选，默认 tcp_{N}
listen = "0.0.0.0:9800"
enabled = true           # 默认 true
```

每帧格式：4 字节长度前缀 + tag（stream name，`u32` LE）+ Arrow RecordBatch（`wp_arrow` IPC 编码）。

引擎接收后按 tag 匹配 schema 中的 `stream` 字段，路由到订阅该 stream 的窗口。

## 文件（批处理 / 回放）

```toml
[[sources]]
type = "file"
name = "events_source"      # 可选，默认 file_{N}
path = "data/events.ndjson"
stream = "netflow"           # 匹配 schema 中的 window.stream
format = "ndjson"            # ndjson | csv | arrow-ipc | arrow-framed
enabled = true
```

### 支持的格式

| format | 说明 | 适用场景 |
|--------|------|---------|
| `ndjson` | 一行一个 JSON 对象 | 小规模回放、调试 |
| `csv` | 逗号分隔，首行为 header | 从传统 SIEM/日志系统导入 |
| `arrow-ipc` | 标准 Arrow IPC File | 大规模批量导入，性能最优 |
| `arrow-framed` | `wp_arrow` IPC 帧格式 | 与 TCP 同格式，录制回放一致 |

### 字段映射

NDJSON/CSV 的字段名和值须与 schema 兼容：

- 字段名：与 schema `fields` 中的名称一致
- 时间字段：ISO 8601 格式字符串，如 `2026-01-01T00:00:00Z`
- IP 字段：字符串，如 `"10.0.0.1"`

## 多源

支持同时配置多个 TCP 和文件源，引擎并行消费：

```toml
[[sources]]
type = "tcp"
listen = "0.0.0.0:9800"

[[sources]]
type = "tcp"
listen = "0.0.0.0:9801"

[[sources]]
type = "file"
path = "data/historical.ndjson"
stream = "netflow"
format = "ndjson"
```
