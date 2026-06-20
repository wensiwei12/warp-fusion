# AI Agent 指南：WFS Window Schema 编写

> 供 AI agent 编写、修改 `.wfs` 文件时遵循。`.wfs` 是数据窗口的**类型契约**——规则文件只引用 schema 中声明的字段和类型。

## 文件结构

```wfs
// 注释用 //

// === 数据窗口（供规则消费） ===
window conn_events {
    stream = "netflow"       // 接收的流名
    time = event_time        // 事件时间字段
    over = 30m               // 窗口大小（数据保留时长）
    fields {
        sip: ip
        dip: ip
        dport: digit
        bytes_out: digit
        event_time: time
    }
}

// === 告警窗口（供规则输出） ===
window network_alerts {
    over = 0                 // 不进窗口，纯输出
    fields {
        sip: ip
        alert_type: chars
        detail: chars
    }
}
```

## 核心规则

### 1. 字段命名

- 小写 + 下划线：`sip`、`dport`、`bytes_out`、`event_time`
- 与 WPL/OML 产出的字段名**完全一致**
- 使用 `wparse batch` 输出确认字段名后，不能随意改

### 2. 字段类型

| 类型 | Arrow 映射 | 说明 | 示例 |
|------|-----------|------|------|
| `ip` | Utf8 | IP 地址字符串，**不能含端口** | `10.0.0.1` ✅ / `10.0.0.1:22` ❌ |
| `digit` | Int64 | 有符号 64 位整数，**不能浮点** | `800` ✅ / `3.5` ❌ |
| `float` | Float64 | 64 位浮点数 | `0.85` |
| `chars` | Utf8 | 任意字符串 | `"ssh"`、`"GET"` |
| `bool` | Boolean | true/false | `true` |
| `time` | Timestamp(Nanosecond) | Unix 纳秒时间戳 | `1718841139000000000` |

**类型匹配比命名更重要**：`digit` 字段必须解析为整数，`ip` 必须可解析为合法 IPv4/IPv6。

### 3. `stream` — 流名

```wfs
window auth_events {
    stream = "auth"          // 接收名为 "auth" 的流
    ...
}
```

**规则：**
- 流的"名称"由上游（wparse/sink）决定
- schema 的 `stream` 必须与上游发送的流名一致
- 一个流可以被多个 window 订阅（指向同一个数据源）

### 4. `over` — 窗口大小

| 值 | 含义 | 用途 |
|----|------|------|
| `> 0`（如 `5m`/`30m`/`1h`） | 数据窗口 | 供 `match<key:window>` 消费 |
| `0` | 不进窗口 | 告警输出 window / `on each` 规则 |

**规则：**
- 数据窗口：`over` ≥ 规则 `match` 中的时间窗
- 告警窗口：`over = 0`

### 5. `time` — 事件时间字段

```wfs
window conn_events {
    time = event_time
    ...
}
```

- 必须指向 `fields` 中声明的 `time` 类型字段
- 用于 watermark 推进、窗口到期判断
- 如果传入数据中 `event_time` 缺失或为 null，规则不触发

### 6. 窗口命名

- 小写 + 下划线：`conn_events`、`auth_events`、`network_alerts`
- 按数据域命名：
  - `conn_events` — 网络连接
  - `auth_events` — 认证
  - `http_events` — Web
  - `dns_events` — DNS
- 告警窗口：`<domain>_alerts`（`network_alerts`、`security_alerts`）

## 常见模式

### 数据窗口（供规则消费）

```wfs
window conn_events {
    stream = "netflow"
    time = event_time
    over = 30m
    fields {
        sip: ip
        dip: ip
        dport: digit
        bytes_in: digit
        bytes_out: digit
        protocol: chars
        action: chars
        event_time: time
    }
}
```

### 告警窗口（供规则输出）

```wfs
window network_alerts {
    over = 0
    fields {
        sip: ip
        dip: ip
        alert_type: chars
        detail: chars
        request_count: digit
    }
}
```

### 多流场景（规则需要两个数据源）

```wfs
window auth_events {
    stream = "auth"
    time = event_time
    over = 10m
    fields { sip: ip, user: chars, result: chars, event_time: time }
}

window conn_events {
    stream = "netflow"
    time = event_time
    over = 30m
    fields { sip: ip, dip: ip, dport: digit, bytes_out: digit, event_time: time }
}
```

规则可以用 `use "network.wfs"` 和 `use "auth.wfs"` 同时引用两个 schema，然后用多别名事件：

```wfl
events {
    scan  : conn_events && bytes_out < 1000
    login : auth_events && result == "success"
}
```

## 常见错误

### ❌ 错误 1：告警窗口没用 `over = 0`

```wfs
window security_alerts {
    over = 5m             // ❌ 告警输出不应该进窗口
}
```

### ❌ 错误 2：字段类型与数据不匹配

```wfs
fields {
    dport: digit          // 但上游产出是字符串 "22"，wparse 解析为 chars → 类型不匹配
}
```

**✅ 正确**：先确认上游数据格式，再写 schema。参考 [DATA_CONTRACT.md](../../wf-rules/DATA_CONTRACT.md)。

### ❌ 错误 3：`stream` 名与上游不一致

```wfs
window conn_events {
    stream = "netflow"    // wfs 写 "netflow"
}
```

但 wparse 发送的流名是 `"conn"` 或 `"netflow_events"` → 数据进不了窗口。

### ❌ 错误 4：`time` 字段类型不对

```wfs
fields {
    event_time: chars     // ❌ time 字段必须声明为 time 类型
}
```

**✅ 正确**：`event_time: time`。

## 验证

```bash
# 单文件检查
wfl lint rules/my_rule.wfl -s "schemas/*.wfs"

# 如果 wfl 编译报 "field not found" 或 "type mismatch"
# → 检查 schema 字段名和类型是否与 WPL 产出一致
```

## 参考

- 规则编写 → [wfl-guide.md](wfl-guide.md)
- 数据契约 → [wf-rules/DATA_CONTRACT.md](../../wf-rules/DATA_CONTRACT.md)
- 配置编写 → [wf-config-guide.md](wf-config-guide.md)
- 规则库 schema 示例 → [wf-rules/schemas/](../../wf-rules/schemas/)
