# 数据源配置 (`[[sources]]`)

数据源通过 `wf-connector-api` 的 `BatchSource` trait 接入，底层由 `wp-core-connectors` 提供实现（File / TCP / Kafka 等）。

## 文件（批处理 / 回放）

```toml
[[sources]]
type = "file"
name = "events_source"      # 可选，默认 file_{N}
path = "data/events.ndjson"
stream = "netflow"           # 匹配 schema 中的 window.stream
format = "ndjson"            # ndjson | arrow-ipc
enabled = true
```

### 支持的格式

| format | 说明 | 适用场景 |
|--------|------|---------|
| `ndjson` | 一行一个 JSON 对象 | 回放、调试 |
| `arrow-ipc` | 标准 Arrow IPC File | 大规模批量导入 |

### 字段映射

NDJSON 的字段名和值须与 schema 兼容：

- 字段名：与 schema `fields` 中的名称一致
- 时间字段：ISO 8601 格式字符串，如 `2026-01-01T00:00:00Z`
- IP 字段：字符串，如 `"10.0.0.1"`
- 数字字段：整数 `443` 或字符串 `"443"` 均可

## TCP（实时）

```toml
[[sources]]
type = "tcp"
name = "netflow_input"   # 可选，默认 tcp_{N}
listen = "0.0.0.0:9800"
enabled = true           # 默认 true
```

每帧格式：长度前缀 + tag（stream name）+ Arrow IPC RecordBatch。

引擎接收后按 tag 匹配 schema 中的 `stream` 字段，路由到订阅该 stream 的窗口。

## Kafka

```toml
[[sources]]
type = "kafka"
name = "netflow_kafka"        # 可选，默认 kafka_{N}
brokers = ["localhost:9092"]
topic = "netflow"
group_id = "wfusion"           # 默认 "wfusion"
stream = "netflow"              # 匹配 schema 中的 window.stream
format = "ndjson"               # ndjson | arrow-ipc
enabled = true
```

> **依赖**：Kafka 源需要 `rdkafka` crate，当前为占位实现。

每条 Kafka message 按 `format` 解析：
- `ndjson`：message payload 为单个 NDJSON 事件
- `arrow-ipc`：message payload 为 Arrow IPC RecordBatch

## 多源

支持同时配置多种源，引擎并行消费：

```toml
[[sources]]
type = "tcp"
listen = "0.0.0.0:9800"

[[sources]]
type = "file"
path = "data/historical.ndjson"
stream = "netflow"
format = "ndjson"
```

## 底层实现

| Source type | 实现 |
|---|---|
| File | `wp_core_connectors::sources::batch::file::FileBatchSource` |
| TCP | `wp_core_connectors::sources::batch::tcp::TcpBatchSource` (Arrow IPC) |
| Kafka | 规划中（通过 `wp_core_connectors` 扩展） |

数据管道：`BatchSource::receive_batch()` → `Vec<RecordBatch>` → `Router::route()` → `Window`。
