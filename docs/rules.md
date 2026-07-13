# 规则编写 (`.wfl`)

## 基本结构

```
use "network.wfs"                  # 引用 schema 文件

rule rule_name {
    events {                       # 事件声明（支持多别名）
        alias : window_name && filter_condition
    }
    match<group_key:window_duration> {   # 匹配窗口
        on event {                        # 匹配步
            step_label: alias.field | transform | measure cmp threshold;
        }
        and close { total: alias | count >= threshold; }  # 关闭条件（可选）
    } -> score(score_value)
    join window_name join_type on condition    # Join（可选）
    entity(entity_type, alias.field)           # 实体
    yield output_window (                      # 输出
        field1 = expr1,
        field2 = expr2
    )
}
```

## 事件声明

### 单别名

```
events { c : conn_events && action == "syn" }
```

### 多别名（多步匹配用）

```
events {
    scan  : conn_events && bytes_out < 1000
    login : auth_events && result == "success"
    xfer  : conn_events && bytes_out >= 10000
}
```

多别名之间用换行分隔，不加分号。每个别名绑定到一个 window，引擎按 stream 自动路由事件到正确的别名。

## 事件过滤器

```
// 等值比较
c.service == "ssh" && c.result == "failed"

// 数值比较
c.bytes_out < 1000 && c.dport >= 1024

// 端口集合
c.dport == 22 || c.dport == 445 || c.dport == 3389

// 正则匹配（多个模式用 || 组合）
(regex_match(h.uri, "'") || regex_match(h.uri, "union.*select"))
```

## 匹配步

### 聚合

| 聚合 | 说明 | 示例 |
|------|------|------|
| `count` | 事件数 | `c \| count >= 10` |
| `sum` | 求和 | `c.bytes_out \| sum >= 100000` |
| `avg` | 平均值 | `c.score \| avg >= 70` |
| `min` / `max` | 最小/最大值 | `c.score \| max >= 90` |
| `distinct` | 去重 | `c.dport \| distinct \| count >= 5` |

### 单步

```
// 5 分钟内同一 IP 产生 ≥ 10 次事件
match<sip:5m> {
    on event { c | count >= 10; }
}
```

### 多步（顺序匹配）

```
// scan → login → xfer 三步序列
match<sip,dip:30m> {
    on event {
        scan | count >= 1;
        login | count >= 1;
        xfer | count >= 1;
    }
}
```

关键：多步在同一个 `on event` 块内顺序执行。只有前一步满足后，状态机才推进到下一步。缺任一步都不命中。

### Close 条件

```
// 关闭条件：所有步满足 + 总数 ≥ 10 时才产出
and close { total: c | count >= 10; }
```

### 分组键

| 分组键 | 含义 |
|--------|------|
| `match<sip:5m>` | 按源 IP 分组，5 分钟窗口 |
| `match<sip,dip:30m>` | 按源 IP + 目标 IP 分组，30 分钟 |
| `match<:1h>` | 无分组键，全局 1 小时窗口 |

## Join

### Anti Join（白名单排除）

```
join scanner_whitelist anti on c.sip == scanner_whitelist.sip
```

匹配 `scanner_whitelist` 中相同 `sip` 的事件被排除。

### Snapshot Join（富化）

```
join internal_ips snapshot on c.sip == internal_ips.ip
```

匹配时从 `internal_ips` 获取 `department`、`owner` 等字段，可在 `yield` 中引用。

## Yield（输出）

```
yield security_alerts (
    sip = c.sip,
    dip = c.dip,
    alert_type = "port_scan",
    detail = "detected"
)
```

yield 中可直接引用事件字段、join 窗口字段、字符串常量和系统上下文变量。普通管道聚合（例如 `c | count`、`c.dport | distinct | count`）仍然只能写在匹配步中；如果需要在输出里说明“为什么触发”，应给匹配步加 label，然后用稳定统计上下文读取该步的结果。

### 时间变量

时间变量只能在 `yield` 中使用，用于把规则触发时的稳定时间语义写入输出窗口。

| 变量 | 含义 | 常用输出字段 |
|------|------|--------------|
| `@event_first_time` | 本次输出证据中最早的事件时间 | `first_seen` |
| `@event_last_time` | 本次输出证据中最晚的事件时间 | `last_seen` |
| `@evidence_start_time` | 本次输出证据范围起点；通常等同最早证据事件时间 | `evidence_start_time` |
| `@evidence_end_time` | 本次输出证据范围终点；通常等同最晚证据事件时间 | `evidence_end_time` |
| `@window_start_time` | 当前规则匹配窗口的起点 | `rule_window_start` |
| `@window_end_time` | 当前规则匹配窗口的终点 | `rule_window_end` |
| `@emit_time` | 引擎生成本条输出记录的稳定时间 | `latest_analysis_time` |

示例：

```wfl
yield security_alerts (
    sip = c.sip,
    first_seen = @event_first_time,
    last_seen = @event_last_time,
    evidence_start_time = @evidence_start_time,
    evidence_end_time = @evidence_end_time,
    rule_window_start = @window_start_time,
    rule_window_end = @window_end_time,
    latest_analysis_time = @emit_time
)
```

输出窗口中这些字段通常声明为 `time`。

### 稳定统计上下文

在 `on event` / `and close` 中给匹配步加 label 后，可以在 `yield` 中使用 `stat.count(...)` 或 `stat.value(...)` 输出稳定统计值。

```wfl
match<sip:5m> {
    on event {
        failures: c | count >= 10;
        target_spread: c.dip | distinct | count >= 3;
    }
    and close {
        final_failures: c | count >= 20;
    }
} -> score(80.0)

yield security_alerts (
    sip = c.sip,
    window_events = stat.count(window_event(c)),
    matched_events = stat.count(match_event(failures)),
    distinct_targets = stat.count(match_distinct(target_spread)),
    trigger_count = stat.value(trigger(failures)),
    final_count = stat.value(final(final_failures))
)
```

| 表达式 | 含义 |
|--------|------|
| `stat.count(window_event(alias))` | 当前 rule instance / window 内，source alias 进入窗口的候选事件数 |
| `stat.count(match_event(label))` | 指定 `on event` step label 接受为证据的命中事件数 |
| `stat.count(match_distinct(label))` | 指定 `distinct | count` step label 的精确 distinct 数量 |
| `stat.value(trigger(label))` | 指定 `on event` label 第一次满足阈值时的聚合值 |
| `stat.value(final(label))` | 指定 `and close` label 在 close / flush 输出时的最终聚合值 |

selector 参数是静态符号，不加引号。`window_event(c)` 中的 `c` 是 `events` 里声明的 alias；`match_event(failures)`、`trigger(failures)` 引用 `on event` label；`final(final_failures)` 引用 `and close` label。label 不存在、阶段不匹配或 selector 用错位置会在编译期报错。

## 测试用例

```
test test_name for rule_name {
  input {
    row(alias, field1 = "value1", field2 = "value2", event_time = "2026-01-01T00:00:00Z");
    row(alias, field1 = "value3", event_time = "2026-01-01T00:00:10Z");
  }
  expect {
    hits == 1;                        // 期望命中数
    hit[0].entity_id == "10.0.0.99";  // 期望的实体 ID
  }
}
```

- `row(alias, ...)` — alias 对应 `events` 中声明的别名
- `input` — 事件按时间顺序注入，支持 `Tick(duration)` 推进时间
- `expect` — `hits` 期望命中数；`hit[i].field` 检查特定命中的字段值
- 一个 `.wfl` 文件只能有一条规则，测试用例必须在该规则内

## 完整示例

```
use "network.wfs"

rule port_scan {
    events { c : conn_events && action == "syn" }
    match<sip:5m> {
        on event { c.dport | distinct | count >= 10; }
    } -> score(80.0)
    join scanner_whitelist anti on c.sip == scanner_whitelist.sip
    entity(ip, c.sip)
    yield alerts (sip = c.sip, alert_type = "port_scan", detail = ">= 10 distinct ports")
}

test scan_detected for port_scan {
  input {
    row(c, sip = "10.0.0.99", dip = "192.168.1.1", dport = "80", action = "syn", event_time = "2026-01-01T00:00:00Z");
    // ... 9 more events to reach threshold
  }
  expect { hits == 1; }
}
```
