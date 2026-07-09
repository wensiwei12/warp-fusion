# 数据源配置 (`[[sources]]`)

格式与 `wp-core-connectors` 一致，connector 特有参数可以直接放在 `[[sources]]` 中，也兼容 `[sources.params]` 子表；`[sources.params_override]` 是 `[sources.params]` 的别名。

如果同一个参数同时出现在 `[[sources]]` 平铺字段和 `[sources.params]` 中，平铺字段优先。`vars` 是配置加载保留字段，不能作为 source 参数名使用。

每个来源都支持 `enable` 开关，默认 `true`。`enable = false` 的来源不会启动，也不会参与 daemon/batch 模式的运行参数校验；名称仍需保持唯一，避免重新启用时产生冲突。

```toml
[[sources]]
connect = "kafka_src"
enable = false
key = "reserved_kafka"
stream = "netflow"
brokers = "localhost:9092"
topic = "netflow"
data_format = "ndjson"
```

## 文件（批处理 / 回放）

```toml
[[sources]]
type = "file"
enable = true
key = "events_source"            # 可选标识符（默认 file_{N}）

[sources.params]
path = "data/events.ndjson"
stream = "netflow"               # 匹配 schema 中的 window.stream
format = "ndjson"                # ndjson | arrow_ipc
```

### 支持的格式

| format | 说明 | 适用场景 |
|--------|------|---------|
| `ndjson` | 一行一个 JSON 对象 | 回放、调试 |
| `arrow_ipc` | 标准 Arrow IPC File | 大规模批量导入 |

## TCP（实时）

```toml
[[sources]]
type = "tcp"
enable = true
key = "netflow_input"

[sources.params]
listen = "tcp://0.0.0.0:9800"
```

每帧格式：长度前缀 + tag（stream name）+ Arrow IPC RecordBatch。

## Kafka

```toml
[[sources]]
type = "kafka"
enable = true
key = "netflow_kafka"

[sources.params]
brokers = "localhost:9092"
topic = "netflow"
group_id = "wfusion"
stream = "netflow"
format = "ndjson"
```

> 需 `rdkafka` crate，当前为占位实现。

## 多源

```toml
[[sources]]
type = "tcp"
enable = true

[sources.params]
listen = "tcp://0.0.0.0:9800"

[[sources]]
type = "file"
enable = false

[sources.params]
path = "data/historical.ndjson"
stream = "netflow"
format = "ndjson"
```

## 底层实现

| type | 实现 crate | 说明 |
|------|-----------|------|
| `file` | `wp_core_connectors` | `FileBatchSource` → NDJSON → Arrow RecordBatch |
| `tcp` | `wp_core_connectors` | `TcpBatchSource` → Arrow IPC |
| `kafka` | 规划中 | 通过 `wp_core_connectors` 扩展 |
