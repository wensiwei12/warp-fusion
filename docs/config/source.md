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

## Kafka

```toml
[[sources]]
type = "kafka"
name = "netflow_kafka"        # 可选，默认 kafka_{N}
brokers = ["localhost:9092"]   # bootstrap servers
topic = "netflow"              # 消费的 topic
group_id = "wfusion"           # consumer group ID（默认 "wfusion"）
stream = "netflow"              # 匹配 schema 中的 window.stream
format = "ndjson"               # ndjson | arrow-ipc
enabled = true
```

> **依赖**：Kafka 源需要 `rdkafka` crate。当前为占位实现，需在 `wf-runtime/Cargo.toml` 中添加 `rdkafka` 依赖并实现 consumer.poll() 循环。

每条 Kafka message 按 `format` 解析：
- `ndjson`：message payload 为单个 NDJSON 事件
- `arrow-ipc`：message payload 为 Arrow IPC RecordBatch

## 多源

支持同时配置 TCP、文件和 Kafka 源，引擎并行消费：

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
