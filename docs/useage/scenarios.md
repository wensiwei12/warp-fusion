# `.wfg` 场景：为规则生成"带标签"的测试数据

`.wfg` 描述**一个场景**：一段背景流量 + 若干**定向构造**的实体。生成出来的数据自带
标签——`hit` 的实体必须报警、`near_miss` / `miss` 的实体必须不报警，`wfgen gen`
在写期望文件之前就把这件事断言掉（INJ1 / INJ2）。因此 `.wfg` 既是"造数据"，也是
"造验收标准"。

> 语法速查（能写什么、每个构造的约束、错误码全表）见 [`wfg-syntax.md`](wfg-syntax.md)。
> 语义与设计取舍的权威文档是 [`../design/wfg-design.md`](../design/wfg-design.md)（含完整
> EBNF、错误码表、旧语法迁移折算公式）。本文只讲怎么用。

## 最小场景

`wfadm init` 模板自带的 `models/scenarios/ssh_brute_force.wfg`，可直接跑：

```wfg
use "../schemas/auth.wfs"                          // 事件 schema
use "../rules/02-initial_access/ssh_brute_force.wfl"  // 要验证的规则

#[duration=10m]                                    // 场景虚拟时长
scenario brute_force_detect<seed=42> {             // seed 决定可复现的随机流
  background {                                     // 除定向构造外的随机流量
    stream auth_events gen 50/s
  }
  inject {
    hit<500> for ssh_brute_force auth_events {      // 500 个实体 × 12 条 = 6000 条
      use(result="failed", service="ssh") x 12
    }
    near_miss<1500> for ssh_brute_force auth_events {   // 1500 × 2 = 3000 条
      use(result="failed", service="ssh") x 2
    }
    miss<21000> for ssh_brute_force auth_events {       // 21000 × 1 = 21000 条
      use(result="success", service="ssh") x 1
    }
  }
}
```

读法：上面 500 个实体**必须都报警**；下面 22500 个实体**必须都不报警**。

生成 + 断言（下列输出为实测）：

```bash
wfgen lint crates/wfadm/templates/models/scenarios/ssh_brute_force.wfg
#   OK

wfgen gen --scenario crates/wfadm/templates/models/scenarios/ssh_brute_force.wfg --out out/gen
#   Inject assert: 23000 entities match their hit/near_miss/miss mode
#   Expected: 500 alerts -> out/gen/ssh_brute_force.except.jsonl
#   Expected meta -> out/gen/ssh_brute_force.except.meta.jsonl
#   Generated 60000 events -> out/gen/ssh_brute_force.jsonl
```

`out/gen/ssh_brute_force.except.jsonl`（+ `.except.meta.jsonl`）就是**期望告警**，可直接喂给
`wfgen verify --expected … --actual …` 与真实引擎输出对拍。

## 怎么写：四步

1. **读规则，拿三件事**
   - **实体字段**：`match<sip : 5m>` → `sip`；多 key → 元组；`on each` → `entity(<type>, <字段>)` 里的字段。
   - **事件步骤数**：规则里有几步（`seq { … }` 的步骤、`on event …` 的事件数），`use … x N` 最多就写几组（`VN24`）。
   - **阈值**：`count >= 10` 就要求每个 `hit` 实体至少 10 条匹配事件，否则 `INJ1`。
2. **定模式与数量**：“必报”用 `hit`、“必不报”用 `near_miss` / `miss`；数量 = 实体个数 × ΣN。
   模式**不改变**数量、也不改变字段值，所以数量要自己算对。
3. **选值来源**：字段少用 `use(a=1, b=2)`；手头有整份日志用 `use({ … })`；数据在文件里用 `use from`。
4. **`lint` → `gen`，看断言**：`VN*` 是加载期错误（写法 / 字段对不上），`INJ1` / `INJ2` 是生成期
   断言失败（会点名用例、实体、实际与期望告警数）。先确认“这个模式的实体真的该是这个行为”，
   再改数量或过滤条件——**不要靠放宽断言让它过**。

## 按任务找写法（配方）

| 我想造 | 写法 | 详见 |
|---|---|---|
| 同一实体反复触发规则 | `hit<sip: N> for … { use(…) x M }`，`M` 要踩到规则阈值 | 数量口径 |
| “接近但未达阈值”的负样本 | `near_miss<…>`，条数刚好差一两条 | 三种模式 |
| 与规则无关的随机实体 | `miss<…>` | 三种模式 |
| 背景里出现热点 IP / 热点账号 | `entity <窗口>.<字段> zipf(pool=…, exponent=…)` | 背景实体分布 |
| 跨流配对（join 规则） | `join <目标窗> as <右键字段> { use(…) x N }` | 跨流注入 |
| 带 `not has …` 的规则要能触发 | `without(<谓词>) [within D]` | 带否定步骤的规则 |
| 把现成的一份数据原样灌进去 | `replay <窗口> { use from "文件" }` | 照单发货 |
| 多步骤序列（`seq`） | 多个 `use … x N`；`then` 只影响可读性 | 值与值来源 |
| `on each` 规则 | `hit<…> for … { use(…) x 1 }`（一条即触发）；`near_miss` 与 `miss` 在此同义 | 三种模式 |

语法层面（写法是否合法、每个构造的硬约束）见 [`wfg-syntax.md`](wfg-syntax.md)。

## 文件结构

| 元素 | 说明 |
|---|---|
| `use "x.wfs"` / `use "x.wfl"` | 相对 `.wfg` 所在目录解析；`.wfs` 给字段类型，`.wfl` 给规则 |
| `#[duration=10m]` | 场景虚拟时长（`30s` / `2m` / `2h` …）。注解键只有 `duration` 与 `seed` 两个，其他键报 `VN29` |
| `scenario NAME<seed=N>` | 场景名 + 随机种子 |
| `background { stream S gen R/s }` | 背景流量：只决定"除定向构造外还有多少随机事件"。速率**只支持常量**（`/s` `/m` `/h`） |
| `entity W.f zipf(pool=N, exponent=S, fresh=R)` | 背景实体分布：该字段从 N 个实体的池里按 Zipf 权重抽（热点重复出现），`fresh` 比例取池外新值 |
| `inject { … }` | 定向构造：每个用例声明**模式 + 实体个数 + 事件组** |
| `join <window> as <key> { … }` | 跨流注入：为规则的 join 目标窗造配对事件（键与时间自动推导） |

## 三种模式 = 硬断言

| 模式 | 语义 | 断言（生成期，INJ1/INJ2） |
|---|---|---|
| `hit` | 这个实体**应该**报警 | 每个 `hit` 实体在其窗口内必须产出告警，否则 FAIL |
| `near_miss` | 差一点，但**不该**报警 | 每个 `near_miss` 实体必须不产出告警，否则 FAIL |
| `miss` | 与规则无关，**不该**报警 | 每个 `miss` 实体必须不产出告警，否则 FAIL |

- 断言复用期望结果，失败在**写期望文件之前**报出，不会留下半成品侧车文件。
- 用例之间**实体键空间分段**（各自独占一段 `10.a.b.c` 地址），因此 `hit` 与
  `near_miss` 不会指向同一个实体、两个口径互相污染。
- 目标规则是 `on each` 形态时：命中的**一条**事件即产出告警，故 `near_miss` 与
  `miss` 在这里**同义**（都是"一条都不许命中"）。

## 数量口径

```text
新写法： mode<[field:]实体数> for RULE STREAM { use(...) x N }
注入条数 = 实体数 × ΣN（同一用例内多个事件组的 N 相加）
总条数   = 背景配额(rate × duration) + 注入条数
```

- **背景与注入分离**：改背景速率不会改变注入条数，注入也不占背景配额。
- `%` 写法的旧形态（`hit<20%> … with(N)`）**已删除**，解析期报 `VN20` 并给出改写方向。
- 实体字段可省（从规则推断）；规则是**多 key**、或推断有歧义时显式写 `hit<sip: 500>`。
  推断口径见设计文档 §3.7。

## 值与值来源

每个事件组用 `use(…)` 指定这次事件要覆盖哪些字段，三种来源：

```wfg
hit<sip: 20> for sdm_rule sdm_event {
  use(action="syn", dport=22)                x 5   // ① 谓词（标量）
  then use({ "tenant_id": "t02",                   // ② 整份 JSON 内联（顶层键即字段）
             "source_finding_obj": { "rule": { "label": "账号攻击" } },
             "tags": ["a", "b"] })             x 1
  spread 5m
}

hit<sip: 20> for sdm_rule sdm_event {
  use from "raw/big.ndjson" x 3                    // ③ 值来自文件
}
```

- `use from "file"`：相对 `.wfg` 目录解析；支持顶层 object、object 数组、NDJSON
  （按**事件序号**循环取用，`N` 大于记录数时回绕）。文件缺失 / 非 JSON / 形态非法在
  `lint`、`gen` 阶段都会直接报错，不做静默兜底。
- 字段覆盖优先级（高 → 低）：实体键（保证同实体聚合）> 当前事件组 `use(...)` >
  规则 bind filter 推导出的约束。
- `spread D`：把该用例的事件铺开到 `D`，必须 ≤ `#[duration]`（`VN25`）；不给时按
  规则窗口长度铺开。各实体的窗口在场景时长内**等距**排开（首簇贴 `0`、末簇贴到场景末尾，
  覆盖整段），簇内事件按步骤顺序在窗口内均匀落下。

## 跨流注入：`join(...)`

规则的 `join` 是跨流的：驱动侧与目标窗在两条流上。`join` 块声明「为本用例的每条左事件，
在目标窗造几条」——右行的**连接键**与**时间**由生成器推导，你不用手写：

```wfg
hit<id: 200> for q8_monitor_new_user person_events {   // 左（驱动）侧
  use(name="n") x 1
  join auction_events as seller {                      // 目标窗 + 右行连接键字段
    use(price=7) x 1
  }
}
```

- **连接键**：`as seller` 的 `seller` 会被写成**左实体键值**——名字要和规则
  `on p.id == auction_events.seller` 的**右侧字段**一致（VN30 校验能唯一匹配到该 join 子句）。
- **时间**：由规则 join 的形态决定 —— `deferred`（inner + `within` + `emit at`）放**同刻**；
  `snapshot`（无 `within`）放**提前 1ms**（点查在驱动事件处理时就要能看见右行）。
- 断言口径不变：仍以**驱动侧实体**为单位（`hit` 必报、`near_miss` / `miss` 必不报）。
- 支持两种形态：**deferred**（`inner` + `within` + `emit at`）与 **snapshot**（无 `within`）。
  其余（`asof` / `anti`、没有 `emit at` 的即时 inner）明确报错。
- 若规则的 `within` 下界晚于左事件时间，右事件会落在区间外——生成期 INJ1 会报
  「hit 实体不会触发」（可见的失败，不会静默出数据）。
- **join-then-key**（如 `match<seller:…>` 而 `seller` 在 `auction` 上）**已支持**：连接键取规则
  `on <left> == <right>` 的左侧字段值（两侧同源），join 侧键（`seller`）自动写到右行上，且值域
  与背景噪声分开；实体仍是驱动侧字段（如 `entity(digit, b.auction)` 的 `auction`）。

## 背景实体分布：`entity(...)`

背景事件默认**每个字段每条现随机**——同一条流里 `sip` 每次都是新 IP，没有任何值会重复，
所以「热点 IP 被反复打」这种现实流量表达不出来。给字段声明实体分布即可：

```wfg
background {
  stream conn_events gen 100/s
  entity conn_events.sip zipf(pool=1000, exponent=1.1, fresh=0.2)
}
```

| 参数 | 含义 | 默认 |
|---|---|---|
| `pool=N` | 实体值池：**N 个不同实体** | 必填 |
| `exponent=s` | Zipf 指数：`0` = 均匀，越大越集中（热点越热） | `1.0` |
| `fresh=r` | 比例 `r` 的事件取**池外新值**（新实体不断出现） | `0.0` |

- 只作用于**背景**事件；`inject` / `replay` 的实体值不受影响。
- 值域**与注入实体分区**（池占 24 位空间顶部、注入占底部），校验期会检查预算——所以背景
  噪声不会撞上注入实体、把 INJ1/INJ2 的口径搅乱。
- 只支持 `ip` / `digit` / `float` / `chars` / `hex` 字段；其它类型报 `VN31`。

## 带否定步骤的规则：`without(...)`

规则里有 `not has …` 步骤时，光注入正向事件不够——该实体的窗口里只要出现一条
**属于它、且命中否定条件**的事件，规则就不会触发。用 `without(...)` 声明这条约束：

```wfg
#[duration=10m]
scenario no_login_then_xfer<seed=7> {
  background { stream conn_events gen 50/s }
  inject {
    // 20 个 IP：scan → xfer；声明这些 IP 的窗口内不得出现成功登录
    hit<sip: 20> for scan_then_xfer conn_events {
      use(action="scan") x 3
      then use(action="xfer") x 1
      without(action="login_ok") within 5m
    }
  }
}
```

- **不写条数**（否定步骤没有“N 条”语义），也不占 `use` 步骤位（不影响 `VN24`）。
- `within D` 可省，默认取目标规则 `match` 的窗口长度；必须 ≤ `#[duration]`（`VN25`）。
- 窗口起点 = 该实体**首条注入事件**的时间。
- 与规则的 `not` 步骤**不要求对位**：它是纯构造约束，规则有没有 `not` 都能写。
- 严格性：注入事件命中谓词 → 生成期直接报错（要就得改 `use(...)` 的值）；背景噪声
  命中谓词 → 直接剔除。要造“违反”的样本，把那条事件当普通 `use(...) x N` 步骤注入。
- 谓词与 `use(...)` 同形式，同样过 `VN9` / `VN11` / `VN12`。

## 照单发货：`replay`

有一份现成的数据要原样灌进去（不做实体数学、不参与断言），用 `replay`：

```wfg
#[duration=10m]
scenario replay_only<seed=1> {
  background { stream conn_events gen 50/s }

  replay conn_events { use from "raw/monday.ndjson" }   // 文件有多少条就发多少条
}
```

- **不写条数**（写了 `x N` 报 `VN20`）；值来源只能是文件（`use from`）。
- 记录形态与 `use from` 一致：object / object 数组 / NDJSON。
- 时间：记录里的 `_timestamp`（或 schema 的时间字段）以**最早一条为锚平移到场景起点**，
  同文件内的相对间隔保持；文件没有时间字段时按序号在 `#[duration]` 内均匀落下。
  平移后超出 `#[duration]` 报 `VN25`（不截断）。
- 目标窗口要在 schema 里（`VN3`），但**不必**写进 `background`。
- 不对任何实体做"必须 / 不得报警"的断言；但它的事件会进期望的输入流，因此
  `verify` 的口径仍与实际一致。若 `inject` 的 `without(...)` 窗口里落进了 replay 事件，
  生成期直接报错（replay 的数据不能自动剔除）。

## 常见错误码

码前缀按校验域分族：`VN`（`.wfg` 语法与注入语义）、`INJ`（**生成期**断言）；族内编号递增，
删除的旧号不复用。旧 `SC` / `SV` 两族已随旧语法退役（stream / 规则绑定 → `VN3` / `VN10` /
`VN14`，字段与 schema → `VN11` / `VN22` / `VN23`）。下表是常见的 `VN` 与 `INJ`：

| 码 | 含义 |
|---|---|
| `VN20` | 用了旧注入语法（`hit<N%>` / `with(N)` / `not(...) within(...)` / `injection` / `traffic` / `expect` / `<field> seq`），文案里给出改写方向 |
| `VN10` | 用例的 stream 没在 `background` 里声明 |
| `VN17` | `use({...})` / `use from` 的形态非法（非 object / object 数组，或空数组） |
| `VN11` | `use(...)` / `without(...)` 的字段不在 schema 里 |
| `VN22` | 显式实体字段不在该 stream 的 schema 里 |
| `VN23` | 显式实体字段与规则推断的实体字段不一致（多 key 规则的显式字段属消歧用法，放行） |
| `VN24` | `use` 事件组数超过规则的事件步骤数（每个 `use ... x N` 对应一个步骤） |
| `VN25` | `spread` / `without ... within` / `replay` 文件跨度 超过 `#[duration]` |
| `VN26` | `replay` 文件为空，或文件里的时间字段口径不齐 |
| `VN27` | 场景的实体 id 总数达到 2^24 上限（`miss` 按「每个事件一个独立键」计入） |
| `VN28` | 背景速率用了 `wave(...)` / `burst(...)` / `timeline { ... }`——这三个形态**语法已定但语义未实现**（会按 `base=` 的常量速率生成），先改用常量速率 |
| `VN29` | 场景注解键不在白名单（`#[...]` 只认 `duration`、`<...>` 只认 `seed`），或值类型不合法（如 `#[duration=10]`、`<seed="abc">`） |
| `VN30` | `join <window> as <key>` 匹配不到规则的 join 子句（目标窗 / 右侧连接键），形态不是缺省 inner，或规则的 `within` 区间不含左事件时间 |
| `VN31` | `entity <window>.<field> zipf(...)` 的窗口 / 字段 / 类型 / 参数不合法，或与注入实体的值域预算超限 |
| `VN32` | 重复书写 `background` / `inject` 块（**解析期**报错）：两者都是单例，重复书写不会合并 |
| INJ1 / INJ2 | 生成期断言失败：`hit` 实体没报警（INJ1），或 `near_miss` / `miss` 实体报了警（INJ2） |

排查手法：断言失败时输出会点名是哪个用例、哪个实体、实际与期望的告警数；先确认
"模式是否真的是这个实体应有的行为"，再改数量或过滤条件——不要靠放宽断言让它过。

## 常见坑（口径陷阱）

这些坑的共同特点是：**不报错，但断言或下游对不上**。

| 坑 | 现象 | 正确做法 |
|---|---|---|
| 数值字段的**整值形态** | 早期 `use(bytes=30000000)` 会落成 `30000000.0`，经 Arrow 写 `digit` 列时被 `as_i64()` 静默丢成 null（引擎侧 `sum` 恒为 0，而期望说“必报”） | 已修：整值保持整数；断言 `as_i64()` 即可（`use from` 的文件里 `3e7` 这类也已被 Arrow 侧兼容） |
| JSONL 的 `_timestamp` 只有**毫秒**精度 | 纳秒级断言拿不到值 | 读 schema 时间字段列（纳秒）；只在需要“看得出先后”时用 `_timestamp` |
| `_stream` 是 **tag 不是窗口名** | 按 `_stream` 过滤窗口，过滤错 | 用 `_window` / `window_name` |
| schema 里**没有的字段不落位** | 自定义字段（含 join 侧键）静默丢弃 | 先把它加进对应窗口的 schema |
| `conv` + `top(N)` 限制产出 | `hit` 实体数 > N 必然 `INJ1` | `hit` 实体数取 ≤ N（这是约束，不是断言误报） |
| `on each` 上 `near_miss` 与 `miss` **同义** | 以为能造“差一点”的样本 | 该形态没有窗口与阈值，两者都是“一条都不许命中” |
| 改了背景速率，注入条数没变 | 以为漏算 | 设计如此：背景与注入**完全分离**，注入不从配额推导 |
| 注解拼错（`#[duratoin=10m]`） | 以前静默退回 60s | 现在报 `VN29`，不会默默用默认值 |
| `use from` 的文件路径写绝对路径 | 换机器就挂 | 相对 `.wfg` 目录写（与 `use "x.wfs"` 同一基准） |

## 从旧语法迁移

```text
实体数 = floor( round(rate × duration × pct / 100) ÷ ΣN )
```

逐项改写：`traffic` → `background`、`injection` → `inject`、`hit<N%>` → `hit<实体数>`、
`use(...) with(N)` → `use(...) x N`、`<field> seq { … }` 去掉（实体字段改由规则推断）、
`not(...) within(...)` → `without(...) [within D]`、
`expect { … }` 删除（改由 INJ1/INJ2 承担）。背景速率是否折算可选（不折则总条数 ≈ 旧总量
两倍）。完整对照表见设计文档 §5.2 / §5.3。

`wfadm init` 生成的项目模板（`models/scenarios/*.wfg`）已是新语法，可直接照抄。

## 相关命令

`wfgen lint` / `gen` / `verify` / `send` / `bench` / `stream` 的参数与 `--no-expect` /
`--no-wfl` 语义见 [`cli/cli.md`](cli/cli.md)；把生成的数据送到引擎联调见
[`integration.md`](integration.md)。
