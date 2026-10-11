# 监控配置 (`[metrics]`)

```toml
[metrics]
enabled = true
report_interval = "10s"       # 指标采集 & 输出周期
```

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | bool | `false` | 是否启用指标采集（含 monitor 输出） |
| `report_interval` | duration | `"100ms"` | 采集 & 输出周期 |
| `console_output` | bool | `true` | 是否把周期汇总打到日志（`[res]` 域）；关掉只静默 console，采集/输出不受影响 |
| `prometheus_listen` | string | `"127.0.0.1:9901"` | 预留项（当前仅解析/记录，不实际监听端口） |
| `topn` | table | 空 | 预留项（`enabled` / `max` / `queue_capacity`，当前仅校验取值范围） |
| `monitor` | table | 空 | monitor 输出筛选，见下节 |

## 指标输出

指标通过 monitor sink 输出，需配置 `sinks/infra.d/monitor.toml`：

```toml
[sink_group]
name = "monitor_infra"
windows = ["*"]

[[sink_group.sinks]]
connect = "file_json"
name = "monitor_out"
[sink_group.sinks.params]
file = "metrics.ndjson"
```

指标以 NDJSON 格式输出，每条为一行 JSON：`value` 一律为字符串（直方图分位为 `{:.6}` 浮点串）；不带 `label` 字段的是**全局指标**；`time` 为输出时的墙钟时间（RFC3339，UTC，恒为最后一个字段）：

```json
{"stage":"rule","name":"events_total","label":"nginx_5xx_surge","value":"24000","time":"2026-10-12T00:00:01.234567+00:00"}
{"stage":"window","name":"memory_bytes","label":"nginx_access","value":"1048576","time":"2026-10-12T00:00:01.234567+00:00"}
{"stage":"rule","name":"flush_seconds_p99","label":"nginx_ip_stats","value":"0.512000","time":"2026-10-12T00:00:01.234567+00:00"}
{"stage":"receiver","name":"decode_seconds_p50","value":"0.004000","time":"2026-10-12T00:00:01.234567+00:00"}
```

## 指标筛选 (`[metrics.monitor]`)

默认每个采集周期会把**全部**指标发往 monitor sink。高频部署下同秒可能产生数千行（例如按规则
展开的 `flush_seconds_p50/p99`），落盘/传输开销可观。`[metrics.monitor]` 允许按记录的
`stage` / `name` / `target`（即 `label`）筛选哪些指标发往 monitor sink：

```toml
[metrics]
enabled = true
report_interval = "30s"

[metrics.monitor]
default = "include"           # 未命中任何规则时的默认动作：include | exclude

[[metrics.monitor.exclude]]   # 排除优先于 include
stage = "rule"
name = "flush_seconds_*"      # 指标名通配（支持 * 与 ?）
target = "*"                  # 目标（label）通配；"*" = 任意目标

[[metrics.monitor.exclude]]
stage = "receiver"
name = "decode_seconds_*"     # 省略 target → 只匹配全局（无 label）指标
```

匹配语义：

| 字段 | 说明 |
|------|------|
| `stage` | 阶段通配（`receiver` / `router` / `rule` / `alert` / `window` / …）；省略 = `*` |
| `name` | 指标名通配（如 `flush_seconds_*`、`decode_seconds_*`）；省略 = `*` |
| `target` | 记录 `label` 的通配；省略 = 仅匹配全局记录（无 `label`）；`"*"` = 任意目标 |

优先级：**exclude > include > default**。

- 只裁剪 monitor 输出，不影响规则处理、告警输出及其它运行行为；
- 未配置任何规则时（`default = "include"`）行为与升级前完全一致；
- 被排除的记录不会写入文件 monitor sink，也不会发往 VM 等其它 monitor sink；
- **启动期即生效**：只被 monitor 读的指标族（如 `flush_seconds_*`、`scan_timeout_seconds_*`、`events_total`、窗口计量等）被排除的项**连计数器都不注册**，热路径更新与记录构造都直接省掉；console 汇总复用的指标（`matches_total` / `instances` / `emitted_total` / `memory_bytes`）仍在采集，只在输出侧剔除，console 输出不受影响；
- 改动 `[metrics.monitor]` 需重启（metrics 任务不可热重载，reload 会被阻断并提示；配置在启动期确定）。

### 不建议排除的指标（bench / 诊断脚本回读）

下面这些指标会被仓库自带的 bench / 诊断脚本从 `metrics.ndjson` 回读（或其派生的 `EMIT` 计数）。排除它们是允许的，但几乎总是误配，会让相关校验静默读不到数据。启动时若检测到被排除，引擎会打一条 `WARN [res]` 提示（不会拒绝配置）：

| 指标 | 回读者 |
|------|--------|
| `alert.emitted_total` | `wfgen verify-nexmark`（EMIT 对拍）、nexmark/alert_output bench 正确性摘要 |
| `window.append_total` | nexmark bench（EPS 口径 + “数据已全部进窗”完成条件） |
| `window.acked_lag` | nexmark/alert_output bench（完成/排空判定） |
| `window.evict_total` | bench / 诊断 |
| `window.memory_bytes` | 内存分账（`MEMORY_BISECTION_METHOD`） |
| `alert.dispatch_total` | bench 告警数 |
| `rule.matches_total`、`rule.events_total` | bench `rules/s` / 诊断 |
| `receiver.rows_total` | bench `row/s` |
| `receiver.decode_errors_total` | 健康校验 |

把这些名字从 `exclude` 里拿掉（或改用更精确的 `name`/`target`）即可消除告警。

## 指标目录（`stage` × `name`）

写 `[metrics.monitor]` 的 `stage` / `name` 时按此对照。`label` 即记录里被 `target` 匹配的字段；`—` 表示该指标无 `label`（全局记录，仅被「省略 `target`」或 `target="*"` 命中）：

| `stage` | `name` | `label` | 说明 |
|---------|--------|---------|------|
| `receiver` | `connections_total` · `frames_total` | — | 连接数 · 收帧总数 |
| `receiver` | `rows_total` | 源（+`source_type`；有按机明细时 `machine`） | 按源收行数 |
| `receiver` | `decode_errors_total` · `read_errors_total` | 源 | 解码 / 读取错误 |
| `receiver` | `window_miss_total` | 源（+`reason`） | 未命中窗口 |
| `receiver` | `decode_seconds_p50` · `decode_seconds_p99` | — | 解码耗时 |
| `router` | `route_calls_total` · `delivered_total` · `dropped_late_total` · `skipped_non_local_total` | — | 路由调用 / 投递 / 迟到丢弃 / 非本地下发 |
| `router` | `route_errors_total` | 源 | 路由错误 |
| `evictor` | `sweeps_total` · `time_evicted_total` · `memory_evicted_total` | — | 驱逐扫描 / 时间驱逐 / 内存驱逐 |
| `rule` | `events_total` · `matches_total` · `instances` · `stats_over_limit_total` | 规则 | 事件数 / 命中数 / 实例数 / 状态超限拒收 |
| `rule` | `memory_bytes` | 规则 | 规则实例估算内存 |
| `rule` | `cursor_gap_total` | 规则 + 窗口（无 `label`） | 消费滞后（字段 `rule`/`window`） |
| `rule` | `scan_timeout_seconds_p50/p99` · `flush_seconds_p50/p99` | 规则 | 扫描超时 / flush 耗时 |
| `alert` | `emitted_total` | 规则 | 每规则告警产出数（权威口径） |
| `alert` | `emitted_detail` | 规则（+`machine`、`scope_key`） | 1/64 采样明细 |
| `alert` | `dispatch_total` · `channel_depth` · `channel_full_total` · `channel_send_failed_total` · `sink_dispatch_failed_total` · `append_failed_total` · `no_sink_records_total` · `drain_dropped_records_total` · `escalate_failed_total` · `append_nanos` | — | 告警输出链计数 |
| `alert` | `dispatch_seconds_p50` · `dispatch_seconds_p99` | — | 告警派发耗时 |
| `window` | `memory_bytes` · `allocated_bytes` · `window_capacity_bytes` | 窗口 | 内容 / 分配 / 容量字节 |
| `window` | `rows` · `batches` · `acked_lag` · `append_total` · `evict_total` · `late_total` | 窗口 | 行 / 批 / 未消费滞后 / 入窗 / 驱逐 / 迟到 |
| `window` | `fanout_queued_batches` · `fanout_capacity_batches` · `mailbox_inflight_bytes` · `mailbox_budget_bytes` | 窗口 | 输出链 / mailbox 在途量 |
| `event` | `e2e_latency_seconds_p50` · `e2e_latency_seconds_p99` | — | 端到端延迟 |
| `alloc` | `current_rss_bytes` · `peak_rss_bytes` · `current_commit_bytes` · `peak_commit_bytes` · `page_faults_total` | — | 进程内存分账（装入 provider 时才有） |

> `flush_seconds` / `scan_timeout_seconds` / `decode_seconds` / `dispatch_seconds` / `e2e_latency_seconds` 是直方图，每个分位一条记录（`name` 以 `_p50` / `_p99` 结尾），可按 `name = "flush_seconds_p99"` 只留一个。

更多运行和排障场景见 [Admin API / CLI 文档](../cli/admin_api.md) 和 [运行时配置](runtime.md)。
