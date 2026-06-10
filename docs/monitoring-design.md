# WarpFusion 数据监控方案

## 现状

内置了一套 hand-rolled 指标系统（`wf-runtime/src/metrics.rs`，1103 行），已覆盖 40+ 指标：

### 已有指标

**Receiver（接入层）**
| 指标 | 类型 | 说明 |
|------|------|------|
| `receiver_connections_total` | counter | TCP 连接数 |
| `receiver_frames_total` | counter | IPC 帧数 |
| `receiver_rows_total` | counter | 事件行数 |
| `receiver_decode_errors_total` | counter | 解码失败 |
| `receiver_read_errors_total` | counter | 读取失败 |
| `receiver_decode_seconds` | histogram | 解码延迟 |

**Router（路由层）**
| 指标 | 类型 | 说明 |
|------|------|------|
| `router_route_calls_total` | counter | 路由调用次数 |
| `router_delivered_total` | counter | 成功投递 |
| `router_dropped_late_total` | counter | 迟到丢弃 |
| `router_skipped_non_local_total` | counter | 非本地跳过 |
| `router_route_errors_total` | counter | 路由失败 |

**Rule（规则引擎）** — per-rule
| 指标 | 类型 | 说明 |
|------|------|------|
| `rule_events_total` | counter | 送入状态机的事件数 |
| `rule_matches_total` | counter | 命中次数 |
| `rule_instances` | gauge | 活跃实例数 |
| `rule_cursor_gap_total` | counter | cursor gap（驱逐导致） |
| `rule_scan_timeout_seconds` | histogram | 超时扫描耗时 |
| `rule_flush_seconds` | histogram | 关闭冲刷耗时 |

**Alert（告警）** — per-rule
| 指标 | 类型 | 说明 |
|------|------|------|
| `alert_emitted_total` | counter | 告警产出数 |
| `alert_channel_send_failed_total` | counter | 通道发送失败 |
| `alert_serialize_failed_total` | counter | 序列化失败 |
| `alert_dispatch_total` | counter | 分发到 sink 数 |
| `alert_dispatch_seconds` | histogram | 分发延迟 |

**Evictor（驱逐）**
| 指标 | 类型 | 说明 |
|------|------|------|
| `evictor_sweeps_total` | counter | 驱逐周期数 |
| `evictor_time_evicted_total` | counter | 时间驱逐数 |
| `evictor_memory_evicted_total` | counter | 内存驱逐数 |

**Window（窗口）** — per-window
| 指标 | 类型 | 说明 |
|------|------|------|
| `window_memory_bytes` | gauge | 内存占用 |
| `window_rows` | gauge | 存储行数 |
| `window_batches` | gauge | batch 数量 |

### 暴露方式

1. **Prometheus 端点** — 手写 TCP server 监听 `127.0.0.1:9901`，响应 `GET /metrics`，输出 Prometheus text format
2. **定期日志** — 按 `report_interval`（默认 2s）输出速率表格
3. **关闭摘要** — shutdown 时输出全生命周期统计

---

## 缺口分析

```
Receiver ──► Router ──► Window ──► StateMachine ──► Alert ──► Sink
   ✅          ✅         ⚠️           ❌            ⚠️        ✅
  解码/帧    路由/丢弃  仅总量      无延迟观测    仅总量    分发/耗时
```

### 缺口 1：延迟黑洞

事件从进入窗口到告警产出，中间的链路完全不可见：

```
event_time ──► ingress ──► [?????] ──► alert_emitted
                              ↑
                        这一段完全不知道花了多久
```

**需要**：

| 指标 | 说明 |
|------|------|
| `wf_window_append_seconds` | 事件追加到窗口 buffer 耗时 |
| `wf_sm_advance_seconds` | 状态机 advance（NFA 步进 + join）耗时 |
| `wf_sm_match_seconds` | 命中后 execute_match（entity/yield/conv）耗时 |
| `wf_event_e2e_latency_seconds` | 事件时间戳 → 告警产出，端到端延迟 |

### 缺口 2：通道背压

Rule task → Alert task 之间是 `mpsc::channel(64)`，只有失败计数 `alert_channel_send_failed_total`，不知道：

- 当前队列堆积了多少（决定是否要加大 channel）
- 发送方阻塞次数（影响 rule task 吞吐）

**需要**：

| 指标 | 类型 | 说明 |
|------|------|------|
| `wf_alert_channel_depth` | gauge | 当前队列深度 |
| `wf_alert_channel_full_total` | counter | 队列满次数 |

### 缺口 3：窗口流入/流出

Per-window 只有 gauges（总量），没有速率：

```
append_total ──► ┌──────────┐ ──► evict_total
                 │  Window   │
                 │  Buffer   │ ──► late_drop_total
                 └──────────┘
```

**需要**：

| 指标 | 类型 | 说明 |
|------|------|------|
| `wf_window_append_total` | counter (per-window) | 追加事件数 |
| `wf_window_evict_total` | counter (per-window) | 驱逐事件数 |
| `wf_window_late_total` | counter (per-window) | 迟到丢弃数 |

### 缺口 4：Prometheus 标准化

当前是手写 TCP 解析 + text format 渲染（~200 行），缺少：
- HTTP keep-alive / content negotiation
- OpenMetrics 格式
- `/health` 端点
- 标准 scrape 协议支持

**方案**：引入 `prometheus-client` + `axum`，20 行替代 200 行。

### 缺口 5：数据质量

| 指标 | 类型 | 说明 |
|------|------|------|
| `wf_schema_mismatch_total` | counter | schema 字段不匹配 |
| `wf_null_field_total` | counter | 必填字段为 null |
| `wf_event_order_violation_total` | counter | 乱序事件 |

### 缺口 6：TopN（config 已有，实现缺失）

`MetricsTopNConfig` 在 config 中已定义（`max`、`queue_capacity`），但无实现代码。用于追踪：
- 匹配最多的 Top-N 规则
- 内存占用最大的 Top-N 窗口
- 延迟最高的 Top-N 规则

---

## 实施计划

### P0（必须）

| 项目 | 文件 | 工作量 |
|------|------|--------|
| 通道背压 gauge | `metrics.rs` + `alert_task.rs` | 0.5d |
| 端到端延迟 histogram | `metrics.rs` + `rule_task.rs` | 1d |

### P1（重要）

| 项目 | 文件 | 工作量 |
|------|------|--------|
| 窗口流入/流出 counter | `metrics.rs` + `rule_task.rs` + `receiver.rs` | 1d |
| prometheus-client + axum 替换手写 | `metrics.rs` + `Cargo.toml` | 1d |

### P2（增强）

| 项目 | 文件 | 工作量 |
|------|------|--------|
| 分段延迟（append/advance/match） | `metrics.rs` + `rule_task.rs` | 1d |
| 数据质量指标 | `metrics.rs` + `receiver.rs` | 1d |
| TopN 实现 | `metrics.rs`（新 module） | 2d |

---

## Prometheus 替换示例

```rust
// 替换前：metrics.rs L519-731 手写渲染 + TCP 解析
// 替换后：

use prometheus_client::registry::Registry;
use axum::{Router, routing::get, extract::State};
use std::sync::Arc;

async fn metrics_handler(State(reg): State<Arc<Registry>>) -> String {
    let mut buf = String::new();
    prometheus_client::encoding::text::encode(&mut buf, &reg).unwrap();
    buf
}

async fn health_handler() -> &'static str { "ok" }

pub async fn serve_metrics(registry: Arc<Registry>, listen: &str) {
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
        .with_state(registry);
    let listener = tokio::net::TcpListener::bind(listen).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

## 告警规则建议

基于上述指标，推荐的关键告警：

```yaml
# Prometheus alert rules

- alert: HighLateDropRate
  expr: rate(router_dropped_late_total[5m]) / rate(receiver_rows_total[5m]) > 0.05
  annotations: summary: "迟到丢弃率 > 5%，检查 watermark 配置"

- alert: WindowMemoryNearLimit
  expr: window_memory_bytes / window_max_bytes > 0.85
  annotations: summary: "窗口内存接近上限"

- alert: AlertChannelBackpressure
  expr: rate(alert_channel_full_total[5m]) > 0
  annotations: summary: "告警通道满，可能丢失告警"

- alert: HighDecodeErrorRate
  expr: rate(receiver_decode_errors_total[5m]) / rate(receiver_frames_total[5m]) > 0.01
  annotations: summary: "解码错误率 > 1%，检查 schema 兼容性"

- alert: NoEventsIngested
  expr: rate(receiver_rows_total[5m]) == 0
  for: 5m
  annotations: summary: "5 分钟内无事件摄入"
```
