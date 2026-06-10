# Sink 配置

## 概述

Sink 配置放在 `sinks/` 目录下，wfusion.toml 中通过 `sinks = "sinks"` 指定路径。

```
sinks/
├── infra.d/           # 基础设施 sink
│   ├── default.toml   #   兜底路由
│   ├── error.toml     #   错误兜底
│   └── monitor.toml   #   监控指标（可选）
├── business.d/        # 业务路由 sink
│   └── alerts.toml
├── connectors/        # connector 定义
│   └── sink.d/
│       └── 01-file.toml
└── defaults.toml      # 全局默认值
```

## 业务路由

规则 `yield` 的 `target_window` 匹配 `windows` 列表，命中则走该组 sink。

```toml
# sinks/business.d/alerts.toml
version = "1.0"

[sink_group]
name = "alerts"
windows = ["network_alerts", "security_alerts"]

[[sink_group.sinks]]
connect = "file_json"
name = "alerts_out"
[sink_group.sinks.params]
file = "alerts.ndjson"
```

| 字段 | 说明 |
|------|------|
| `windows` | 匹配的 window 名列表，`["*"]` 匹配所有 |
| `connect` | 引用 `connectors/sink.d/` 中的 connector 名 |
| `name` | sink 实例名 |
| `params` | connector 特定参数 |

## 基础设施

### Default（兜底）

未匹配任何业务路由的 window 走这里。

```toml
# sinks/infra.d/default.toml
[sink_group]
name = "default_infra"
windows = ["*"]

[[sink_group.sinks]]
connect = "file_json"
name = "default_out"
[sink_group.sinks.params]
file = "default.ndjson"
```

### Error（错误兜底）

发送失败时走这里。

```toml
# sinks/infra.d/error.toml
[sink_group]
name = "error_infra"

[[sink_group.sinks]]
connect = "file_json"
name = "error_out"
[sink_group.sinks.params]
file = "error.ndjson"
```

### Monitor（监控指标）

```toml
# sinks/infra.d/monitor.toml
[sink_group]
name = "monitor_infra"
windows = ["*"]

[[sink_group.sinks]]
connect = "file_json"
name = "monitor_out"
[sink_group.sinks.params]
file = "metrics.ndjson"
```

## Connector 定义

```toml
# sinks/connectors/sink.d/01-file.toml
version = "1.0"

[connector]
name = "file_json"
kind = "file"

[connector.params]
format = "ndjson"
```

| `kind` | 说明 |
|--------|------|
| `file` | 本地文件 |
| `tcp` | TCP 流 |
| `syslog-tcp` / `syslog-udp` | Syslog 协议 |
| `kafka` | Kafka topic |
| `arrow-ipc` | Arrow IPC 流 |
| `blackhole` | 丢弃（调试用） |

## 默认值

```toml
# sinks/defaults.toml
batch_size = 1024
batch_timeout_ms = 1000
```
