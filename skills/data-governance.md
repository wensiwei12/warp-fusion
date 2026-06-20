# AI Agent 指南：数据治理（WPL/OML 适配与归一化）

> 供 AI agent 编写 WPL/OML 适配器、诊断数据治理问题、确保上游数据符合 window schema 契约时遵循。

## 数据流全景

```
原始日志 → wpl 解析 → oml 包声明 → wparse 发送 → wfusion window (wfs) → WFL 规则
               ↑              ↑                                ↑
            命名映射       包归属                          类型契约
           (ip:sip等)   (/nginx/→http_events)         (sip:ip等)
```

**规则成功与否，90% 取决于前三步的数据治理质量。**

## 治理原则

1. **先看契约，再写适配**：对照 `DATA_CONTRACT.md` 的字段表，确认每个必填字段都有来源
2. **类型先于内容**：`digit` 必须是整数、`ip` 必须是合法 IP——类型错了，再对的规则也匹配不上
3. **枚举归一、不可省略**：`result` 只能是 `success`/`failed`（小写），`action` 只能是 `syn`/`established`/`fin`/`reset`
4. **流名一致**：wpl 产出的流名 = wfs schema 的 `stream` = wfusion source 配置的 `stream`

## WPL 适配器写法

### 基本语法

```
package /name/ {             // 包名：对应数据源类型
    rule rule_name {        // 规则名
        (field_spec, ...)   // 字段抽取序列
    }
}
```

### 字段声明

| 声明 | 含义 | 示例输入 | 产出 |
|------|------|---------|------|
| `ip:name` | IP 地址 | `10.0.0.1` | `sip = "10.0.0.1"` |
| `digit:name` | 有符号整数 | `"22"` 或 `22` | `dport = 22` |
| `chars:name` | 字符串 | `"ssh"` | `protocol = "ssh"` |
| `time/clf:name` | CLF 时间 | `[06/Aug/2019:12:12:19 +0800]` | `event_time = 1565068339000000000` |
| `time/iso8601:name` | ISO8601 时间 | `2025-01-01T00:00:00Z` | `event_time = 1735689600000000000` |
| `unix:name` | Unix 时间戳 | `1718841139` | `event_time = 1718841139000000000` |
| `http/request` | HTTP 请求行 | `GET /path HTTP/1.1` | 内置解析为 `method`+`uri` |
| `http/status` | HTTP 状态码 | `200` | `status = 200` |
| `http/agent` | User-Agent | `Mozilla/5.0...` | `user_agent = "Mozilla/5.0..."` |
| `_:name` | 跳过字段 | — | 不产出 |
| `2*_` | 跳过 N 个字段 | — | 不产出 |
| `"literal"` | 匹配字面量 | `"sshd["` | 不产出，仅匹配 |

### 样例：nginx → http_events

```
// nginx 日志: 222.133.52.20 - - [06/Aug/2019:12:12:19 +0800]
//   "GET /nginx-logo.png HTTP/1.1" 200 368 "http://ref/" "Mozilla/5.0..." "-"

package /nginx/ {
    rule example {
        (ip:sip, _, _,
         time/clf:event_time,
         http/request,        // → method="GET", uri="/nginx-logo.png"
         http/status,         // → status=200
         digit:bytes_out,     // → bytes_out=368
         chars,               // referer（跳过）
         http/agent:user_agent, // → user_agent
         _)                   // "-"（跳过）
    }
}
```

产出字段对照 `http_events` schema：
- `sip` ✅ `222.133.52.20`
- `event_time` ✅ `1565068339000000000`（纳秒）
- `method` ✅ `GET`（http/request 内置）
- `uri` ✅ `/nginx-logo.png`
- `status` ✅ `200`
- `bytes_out` ✅ `368`
- `user_agent` ✅ `Mozilla/5.0...`
- `dip` ⚠️ 未产出

**`dip` 该从哪来？** nginx 日志不记录目标 IP。治理选项：
1. 从 Web 服务器配置注入（静态值）
2. 从 syslog/Fluentd 的 `hostname` 字段 DNS 解析
3. 在网络层（netflow）补 dip 字段
4. 接受 null——规则用 `dip` 需判空

### 样例：SSH auth.log → auth_events

```
// /var/log/auth.log:
//   Jun 20 10:15:30 host sshd[12345]: Failed password for root from 10.0.0.1 port 22 ssh2

package /auth/ {
    rule ssh {
        (_:month, _:day, time:event_time, _:host,
         "sshd[", digit:_pid, "]: ",
         chars:result_raw,         // "Failed" / "Accepted"
         " password for ",
         chars:user,
         " from ",
         ip:sip,
         " port ",
         digit:dport,
         " ", chars:protocol)
    }
}
```

**OML 后处理**（在 wparse pipeline 中）:
```
result = result_raw.lower()
    .replace("accepted", "success")
    .replace("failed", "failed")
service = "ssh"
dip = host_ip  // 从 syslog hostname DNS 解析或配置注入
```

## OML 包声明

OML 将 WPL 解析出的字段集合声明为一个"包"，并指定流名：

```
package /nginx/ {
    rule example { (ip:sip, time/clf:event_time, ...) }
}
```

产出流的字段名按 WPL 的命名决定。流名由 wparse 的 sink 配置决定——WPL 包名与流名是独立的。

## 字段归一化清单

写 wpl 时逐项检查：

### conn_events

- [ ] `sip`: ip 类型，不含端口 ✅
- [ ] `dip`: ip 类型，不含端口 ✅
- [ ] `dport`: digit 类型，1–65535 ✅
- [ ] `bytes_in` / `bytes_out`：digit 类型，≥0。如果原始数据只给总字节（`bytes`），需拆方向或留空
- [ ] `protocol`: chars，归一为 `tcp`/`udp`/`icmp`（小写）
- [ ] `action`: chars，从 TCP flags 映射为 `syn`/`established`/`fin`/`reset`
- [ ] `event_time`: time 类型，ISO8601 或 Unix 秒→纳秒
- [ ] 所有必填字段非空

### auth_events

- [ ] `sip`: ip 类型 ✅
- [ ] `result`: chars 类型，**必须归一为 `success`/`failed`（小写）** ⚠️
- [ ] `service`: chars，归一为 `ssh`/`smb`/`rdp`/`winlogon`/`vpn`
- [ ] `user`: chars，去域前缀、小写
- [ ] `password_hash`：chars，统一小写十六进制（如果有这个字段）⚠️
- [ ] `event_time`: time 类型 ✅

### http_events

- [ ] `sip`: ip 类型 ✅
- [ ] `method`: chars，`GET`/`POST`/`PUT`/`DELETE`（大写）
- [ ] `uri`: chars，含 query string，URL 解码后
- [ ] `status`: digit，100–599
- [ ] `bytes_out`: digit，≥0

### dns_events

- [ ] `sip`: ip 类型 ✅
- [ ] `query`: chars，小写 FQDN，去尾点
- [ ] `qtype`: chars，`A`/`AAAA`/`TXT`/`MX`/`CNAME`
- [ ] `rcode`: chars，`NOERROR`/`NXDOMAIN`/`SERVFAIL`

## 常见错误

### ❌ 错误 1：result 没归一

```
wpl 产出 result = "Accepted"  → 规则写 `e.result == "success"` → 0 命中
```

**✅ 正确**：wpl 后处理 `result_raw → result`（Accepted→success, Failed→failed）。

### ❌ 错误 2：IP 含端口

```
wpl 产出 sip = "10.0.0.1:22"  → schema `sip: ip` → 类型不匹配
```

**✅ 正确**：ip 类型字段只取 IP 部分，端口独立为 `dport`。

### ❌ 错误 3：流名不一致

```
wpl 产出流名 "conn_events"
wfs schema stream = "netflow"      → 数据进不了窗口
```

**✅ 正确**：wpl 流名、schema `stream`、wfusion source `stream` 三者一致。

### ❌ 错误 4：时间格式不兼容

```
wpl 产出 event_time = "2025-01-01 12:00:00"（字符串，非 ISO8601）
schema event_time: time             → 类型不匹配
```

**✅ 正确**：用 `time/iso8601:event_time` 或 `time/clf:event_time` 产出为 time 类型。

## 验证

```bash
# 确认 WPL 产出的字段名和类型
wparse batch -p -n 100 -S 1 2>&1 | grep "parse_stat"

# 确认字段名与 schema 一致
grep "fields" schemas/*.wfs

# 用 wfl lint 验证 schema-规则一致性
wfl lint rules/*.wfl -s "schemas/*.wfs"
```

## 参考

- 字段契约 → [wf-rules/DATA_CONTRACT.md](../../wf-rules/DATA_CONTRACT.md)
- WPL 样例 → [wf-rules/wpl-samples/](../../wf-rules/wpl-samples/)
- Schema 编写 → [wfs-guide.md](wfs-guide.md)
- 规则编写 → [wfl-guide.md](wfl-guide.md)
