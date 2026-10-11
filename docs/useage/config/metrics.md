# 监控配置 (`[metrics]`)

```toml
[metrics]
enabled = true
report_interval = "10s"       # 指标采集 & 输出周期
```

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | bool | `false` | 是否启用指标采集 |
| `report_interval` | duration | `"2s"` | 采集周期 |
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

指标以 NDJSON 格式输出，每条为一行 JSON：

```json
{"stage":"match","name":"rule_hits","label":"port_scan","value":"12345"}
{"stage":"window","name":"memory_bytes","label":"conn_events","value":"1048576"}
{"stage":"rule","name":"match_latency_p50","label":"port_scan","value":"0.512"}
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

## 指标列表

更多运行和排障场景见 [Admin API / CLI 文档](../cli/admin_api.md) 和 [运行时配置](runtime.md)。

| 类别 | 指标（部分） |
|------|-------------|
| Receiver | `connections_total`, `frames_total`, `rows_total`, `decode_errors_total` |
| Router | `route_calls_total`, `delivered_total`, `dropped_late_total` |
| Rule | `events_total`, `matches_total`, `instances` |
| Alert | `emitted_total`, `channel_depth`, `dispatch_total` |
| Evictor | `sweeps_total`, `time_evicted_total` |
| Window | `memory_bytes`, `rows`, `batches`, `append_total`, `evict_total` |
| Latency | `decode_seconds_p50/p99`, `dispatch_seconds_p50/p99`, `e2e_latency_seconds_p50/p99` |
