# AI Agent 指南：WFL 规则编写

> 供 AI agent 编写、修改、检查 `.wfl` 检测规则时遵循。
>
> 适用版本：`wf-lang` v2.1+。配套 schema 编写见 [`wfs-guide.md`](wfs-guide.md)。

## 规则文件结构

```wfl
use "network.wfs"              // 声明依赖的 schema 文件

rule port_scan {               // 规则名：小写 + 下划线
    events {                   // 1. 事件声明
        c : conn_events && c.action == "syn"
    }
    match<sip:5m> {            // 2. 匹配窗口
        on event { c.dport | distinct | count >= 10; }
        and close { c | count >= 10; }
    } -> score(80.0)           // 3. 分数
    entity(ip, c.sip)          // 4. 实体
    yield network_alerts (     // 5. 输出
        sip = c.sip,
        alert_type = "port_scan",
        detail = ">=10 ports in 5min"
    )
    limits {                   // 6. 治理（v2.1 必填）
        max_memory = "64MB";
        max_instances = 10000;
        on_exceed = throttle;
    }
}
```

## 核心规则

### 1. `events` — 事件过滤

```
events { alias : window && filter }
```

| 写法 | 含义 |
|------|------|
| `e : auth_events` | 所有 auth_events |
| `e : auth_events && e.result == "failed"` | 过滤失败事件 |
| `e : conn_events && e.dport == 22` | 过滤 SSH 连接 |
| `e : conn_events && (dport == 22 \|\| dport == 445)` | 多端口 OR |

**重要**：只能用 `==`、`!=`、`&&`、`||`，不支持 `>`、`<`（这些放 match 阶段）。

### 2. `match` — 匹配窗口

```
match<key:window> { on event { ... } and close { ... } }
```

- **`key`**：分组字段（`sip`、`sip,dip`），每个分组独立窗口
- **`window`**：时间窗（`5m`/`30m`/`1h`），不超过 schema `over`
- **`on event`**：触发条件，每次事件都评估
- **`and close`**：关闭条件（可选），窗口结束时评估

**常见模式**：

| 模式 | 写法 | 场景 |
|------|------|------|
| 频率阈值 | `e \| count >= 10` | brute force |
| 去重计数 | `e.dport \| distinct \| count >= 5` | port scan |
| 求和 | `e.bytes_out \| sum >= 10000000` | data exfil |
| 全部命中 | `a\|count>=1; b\|count>=1; c\|count>=1` | 多步攻击链 |

**多别名事件**（非同一数据源时）：

```wfl
events {
    scan  : conn_events && bytes_out < 1000
    login : auth_events && result == "success"
    xfer  : conn_events && bytes_out >= 10000
}
match<sip,dip:30m> {
    on event {
        scan | count >= 1;
        login | count >= 1;
        xfer | count >= 1;
    }
}
```

### 3. `on each` — 逐条外部查询

```wfl
on each e where external("password_check", e.password_hash) -> score(75.0)
```

- 每条事件调一次 `external()`
- 返回值参与 `where` 判断（true 则命中）
- 依赖 `knowdb.toml` 中的 `[fun.<name>]` 配置
- **限制**：不能用于 `match` 内（`match<key>` 只能用聚合）

### 4. `yield` — 输出声明

```wfl
yield security_alerts (
    sip = e.sip,           // 字段赋值
    alert_type = "name",   // 字符串字面量
    detail = "text"
)
```

- 目标 window 必须在 `.wfs` 中声明，`over = 0`
- 字段名必须存在于目标 window schema 中
- `__wfu_*` 是系统保留前缀，不能作为业务字段

### 5. `entity` — 实体标识

```wfl
entity(ip, c.sip)         // entity_type, entity_id_expr
entity(user, e.user)
```

- 第一参数：实体类型（`ip`/`user`/`host`/`domain`）
- 第二参数：从哪个字段取值

### 6. `limits` — 治理（v2.1 必填）

```wfl
limits {
    max_memory = "64MB";
    max_instances = 10000;
    on_exceed = throttle;
}
```

| 字段 | 说明 |
|------|------|
| `max_memory` | 单规则最大内存 |
| `max_instances` | 最大状态机实例数 |
| `on_exceed` | 超限策略：`throttle`（唯一支持值） |

### 7. `score()` — 风险评分

```wfl
-> score(75.0)                           // 固定分数
-> score(if e.count > 10 then 90.0 else 50.0)  // 条件分数
```

**不允许**：`score()` 作为 `yield` 字段——score 是规则级元数据，不是输出字段。

## 常见错误

### ❌ 错误 1：`external()` 放 match 内

```wfl
match<sip:5m> {
    on event { e && external("check", e.hash) | count >= 1; }  // ❌ external 不能放在聚合里
}
```

**✅ 正确**：用 `on each`：
```wfl
on each e where external("check", e.hash) -> score(75.0)
```

### ❌ 错误 2：`yield` 窗口用了 `over > 0`

```wfl
window security_alerts {    // ❌ 告警窗口 should be over = 0
    over = 5m
    ...
}
```

**✅ 正确**：`over = 0`。

### ❌ 错误 3：缺少 `limits` 块

v2.1 要求每条规则必须含 `limits`。`wfl lint` 会拒绝。

### ❌ 错误 4：字段名与 schema 不一致

`.wfs` 里声明 `alert_type: chars`，但 `.wfl` 里写 `alert_type = e.type` ——如果 `e.type` 在 events 中不存在，编译时报错。

## 验证

```bash
# 单规则检查
wfl lint rules/my_rule.wfl -s "schemas/*.wfs"

# 全量检查
wfl lint rules/*.wfl -s "schemas/*.wfs"

# 编译计划查看
wfl explain rules/my_rule.wfl -s "schemas/*.wfs"

# 单元测试
wfl test rules/my_rule.wfl -s "schemas/*.wfs"
```

## 参考

- Schema 编写 → [wfs-guide.md](wfs-guide.md)
- 配置编写 → [wf-config-guide.md](wf-config-guide.md)
- 规则库 → [wf-rules/](../../wf-rules/)
- 数据契约 → [wf-rules/DATA_CONTRACT.md](../../wf-rules/DATA_CONTRACT.md)
