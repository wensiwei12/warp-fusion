# WFG 设计 — `.wfg` 场景 DSL

> 本文是 `.wfg` 的**唯一主设计文档**：语法、语义、校验错误码、迁移与落地状态。
>
> 归口：`.wfg` 的解析器 / AST / 生成器全部位于本仓库的 `wfgen` crate。
> 本文合并自两份前身文档：`wp-reactor/docs/design/wfg-design.md`（旧语法规范，已随旧语法废弃）
> 与本文档的前身 `wfg_injection_design.md`（注入语义重设计）；两者均已并入本文。
>
> 相关文档：[wfadm.md](./wfadm.md) · [wfl_seq_design.md](./wfl_seq_design.md) ·
> [getting-started.md](../useage/getting-started.md) · [cli.md](../useage/cli/cli.md)

## 1. 目标与边界

- **可读性优先**：场景编写者一眼看懂"生成什么、验证什么"。
- **规则验证显式**：不允许把规则逻辑藏在语法糖里。
- **stream-first**：场景只描述 stream 级数据，window 与字段约束由 `.wfs/.wfl` 推导。
- **数量显式**：写下来的数字 = 生成出来的数字，不做任何隐式除法。
- **模式即断言**：`hit` 必须报警、`near_miss`/`miss` 必须不报警，由系统判定。
- **一件事一个旋钮**：数量归数量、值归值、背景归背景、时间归时间。
- **不兼容旧语法**：旧形态直接报 VN20，附等价值改写建议。

不含：`faults` 块语义、oracle 对拍机制（复用现有实现）。

### 1.1 为什么这样设计（旧语义的缺口）

旧语法里用户写的是"速率 + 比例"，条数是算出来的：

```text
stream 配额 = stream 速率 × duration
模式预算    = round(配额 × 比例%)
实体数      = min over stream [ 模式预算 ÷ Σ每簇条数 ]
背景条数    = 配额 − 注入条数
```

`hit<20%>` 的 `%` 既不是实体比例也不是事件比例，而是**配额的百分比**；读到 `20%` 无法知道会造多少条，
改背景速率还会连带改变定向构造的数量。七条设计原则各自消解一类缺口：

| 原则 | 消解的问题 |
|---|---|
| P1 数量显式，不做隐式除法 | `%` 的含义不可读；实体数靠两次乘除推出 |
| P2 一个字段一个职责 | `with(N)` 同时是"每簇条数"和"分簇除数" |
| P3 模式是断言方向，不改任何数量与字段值 | `near_miss` 把条数夹到 `min(N, 阈值-1)`；`miss` 走"允许与 filter 冲突"旁路而 `hit` 禁止 |
| P4 断言可判定（复用 oracle，生成期报错） | `expect { hit(rule) >= 90% }` 的度量与阈值从不被求值；命中路径不可预判 |
| P5 背景与注入分离 | 背景事件也参与判定分母；改背景速率会改变注入量 |
| P6 时间显式 | 铺开策略隐式，峰值速率不可预期（可能撞 `limits` throttle） |
| P7 语法自解释 | `sip seq { … }` 像块名而不是实体键；`for RULE` 可省、靠 `expect` 反推 |

## 2. 语法

### 2.1 EBNF（当前实现）

```ebnf
scenario_file    = { use_decl } , [ scenario_attrs ] , scenario_decl ;

use_decl         = "use" , STRING ;

scenario_attrs   = "#[" , anno_list , "]" ;
anno_list        = anno_item , { "," , anno_item } ;
anno_item        = IDENT , "=" , value ;

scenario_decl    = "scenario" , IDENT , [ "<" , anno_list , ">" ] , "{" ,
                     background_block ,
                     [ inject_block ] ,
                   "}" ;

background_block = "background" , "{" , { stream_stmt } , "}" ;
stream_stmt      = "stream" , IDENT , "gen" , rate_expr ;
rate_expr        = rate_const | wave_expr | burst_expr | timeline_expr ;
rate_const       = NUMBER , "/" , ( "s" | "m" | "h" ) ;
wave_expr        = "wave(" , "base=" , rate_const , "," , "amp=" , rate_const , "," ,
                   "period=" , DURATION , [ "," , "shape=" , shape_kw ] , ")" ;
burst_expr       = "burst(" , "base=" , rate_const , "," , "peak=" , rate_const , "," ,
                   "every=" , DURATION , "," , "hold=" , DURATION , ")" ;
timeline_expr    = "timeline" , "{" , { DURATION , ".." , DURATION , "=" , rate_const } , "}" ;
shape_kw         = "sine" | "triangle" | "square" ;

inject_block     = "inject" , "{" , { inject_case } , "}" ;
inject_case      = mode_kw , "<" , [ IDENT , ":" ] , INTEGER , ">" ,
                   "for" , IDENT , IDENT , "{" , inject_body , "}" ;
mode_kw          = "hit" | "near_miss" | "miss" ;
inject_body      = { event_group } , [ "spread" , DURATION ] ;
event_group      = [ "then" ] , "use" , value_source , "x" , INTEGER ;
value_source     = "(" , predicate_list , ")"
                 | "{" , json_object , "}"
                 | "from" , STRING ;
predicate_list   = predicate , { "," , predicate } ;
predicate        = IDENT , "=" , value ;

value            = STRING | NUMBER | DURATION | "true" | "false"
                 | json_object | json_array | "null" ;
```

要点：

- 注释只支持 `//`；`#` 不是注释，`#[]` 是元信息注解。
- 当前被消费的注解键只有 `duration`（场景总时长，默认 `60s`）与 `seed`（写在
  `scenario name<seed=N>`，默认 `0`）。`tick` / `rows` / `emit` **未实现**。
- `for RULE` **必填**，不再从期望反推。
- `x N` 是"每个实体在该步骤上的条数"，取代旧的 `with(N)`。
- 实体键可省：`hit<500>` 从规则推断；`hit<sip: 500>` 用于多 key / 消歧（见 §3.7）。
- `spread D` 可选，必须 ≤ `#[duration]`（VN25）。
- `use` 的值来源三种：`use(preds)`、`use({json})`、`use from "file"`。
- 期望块 `expect {…}` 已删除，断言由模式承担（§3.2）。

### 2.2 完整示例

```wfg
use "auth.wfs"
use "../rules/ssh_brute_force.wfl"

#[duration=10m]
scenario ssh_brute<seed=42> {
  // 背景噪声：只表示"除定向构造外还有多少随机流量"
  background {
    stream auth_events gen 50/s
  }

  inject {
    // 500 个 IP × 每个 12 条 = 6000 条；断言：这 500 个都必须报警
    hit<sip: 500> for ssh_brute_force auth_events {
      use(result="failed", service="ssh") x 12
      spread 10m
    }

    // 200 个 IP × 每个 2 条 = 400 条；断言：这 200 个都不报警
    near_miss<sip: 200> for ssh_brute_force auth_events {
      use(result="failed", service="ssh") x 2
    }

    // 100 个 IP × 每个 1 条 = 100 条；断言：都不报警
    miss<sip: 100> for ssh_brute_force auth_events {
      use(result="success", service="ssh") x 1
    }
  }
}
```

多步骤、整份 JSON、文件来源：

```wfg
hit<sip: 500> for chain_attack conn_events {
  use(action="syn")                        x 5
  then use(action="login_fail", dport=22)  x 6      // 500 × (5+6) = 5500 条
}

hit<event_id: 200> for object_on_each sdm_event {
  use({                                            // 整份 JSON 内联：顶层键即字段
    "tenant_id": "tenant02",
    "source_finding_obj": { "rule": { "label": "账号攻击" } },
    "tags": ["a", "b"]
  }) x 1
}

hit<sip: 20> for sdm_rule sdm_event {
  use from "raw/big.ndjson" x 3                    // 值来自文件
  spread 5m
}
```

## 3. 语义

### 3.1 数量（P1）

```text
实体数      = 用例头的 INT
每实体条数  = 第 k 个 event_group 的 x N
注入条数    = 实体数 × Σ N
```

没有任何隐式除法。`hit<sip: 500> … { use(...) x 12 }` = 500 个实体、6000 条注入。

背景条数由 `background` 速率决定（各 stream 的 `rate × duration` 之和），注入量**不从配额推导**；
两者**完全分离**：`总条数 = 背景 + 注入`，背景不被注入挤压、注入也不被配额截断
（消解"改背景速率会改变注入量"）。

### 3.2 模式 = 硬断言（P3、P4）

| 模式 | 数量 | 字段值 | 断言 |
|---|---|---|---|
| `hit` | `x N` 原样 | 由 `use` 决定 | 每个实体都必须产出 ≥1 条告警 |
| `near_miss` | `x N` 原样 | 满足规则 filter，但达不到阈值 | 每个实体都不得产出告警 |
| `miss` | `x N` 原样 | 可以违反 filter | 每个实体都不得产出告警 |

- 模式**不修改**任何数量、不修改任何字段值。
- `near_miss` 与 `miss` 的区别只剩构造意图（是否满足 filter），断言强度相同。
- 断言以**实体**为单位，背景事件不参与。
- 判定复用内置 oracle（`oracle/mod.rs` 的 `RuleEngine` + `CepStateMachine`）：`gen` 本来就要跑它算期望告警，因此断言是"读已有结果"。
- 多条触发路径（`on event` / `on close` / `seq` / `conv`）天然被覆盖：只要任一路径产出告警即算"报警"。
- **`conv` + `top(N)` 的例外**：`top(N)` 每个窗口最多输出 N 条，所以只有"能进入某窗口
  top-N"的实体才可能报警——`hit` 实体数取 ≤ N 最直观（`hit<14400>` + `top(2)` 必然 INJ1，
  这是约束而不是断言误报）。
- **实体键空间按用例分段**：每个用例独占一段实体 id（`hit`/`near_miss`/`miss` 之间不重叠），
  否则同一实体既被声明为 `hit` 又被声明为 `near_miss`，两个模式的口径互相矛盾。

### 3.3 值与值模板（P2）

- `use(preds)` / `use({json})` / `use from "f.json"` 提供该步骤的字段值。
- 默认：该步骤的 N 条事件使用**同一份**值。
- `use({...})` 的顶层键展开为字段；`_` 前缀的键（`_stream` / `_window` / `_timestamp`）**忽略**，方便把原始日志整份粘进来。
- `use from` 的文件顶层若是**数组**：N 条事件应在记录间循环取用（`N > 记录数` 时回绕）——**未实现**（见 §7.2）。
- 同一 `use` 内重复字段报 VN9；与实体键重复报 VN12；字段不在 schema 报 VN11。

### 3.4 背景与注入分离（P5）

`background` 只决定"除定向构造外的随机流量"。注入条数完全由用例头与 `x N` 决定，因此：

- 改背景速率**不会**改变注入条数；
- 注入不占背景的配额：`总条数 = 背景(rate × duration) + 注入(实体数 × ΣN)`——注入量大于
  背景配额时背景也不会被清零；
- 写错数量不再静默产出 0 条（由断言/VN21 暴露）。

### 3.5 时间铺开（P6）

- `spread D` 显式给出铺开窗口，覆盖默认的规则窗口长度；必须 ≤ `#[duration]`（VN25）。
- 当前实现仍是"随机簇起点 + 窗口内铺开"，**均匀铺开未实现**（§7.2）。

### 3.6 字段覆盖优先级

从高到低（`inject_gen/helpers/generate.rs::build_event_fields_with_predicates`）：

1. 实体键覆盖（保证同实体聚合）
2. 当前 step 的 `use(...)` 谓词
3. 规则 bind filter 推导出的字段约束
4. 时间字段（自动填 `timestamp`）
5. 随机默认生成

### 3.7 实体字段推断（决策 1）

实体键省略时按规则形态推断：

| 规则形态 | 实体字段 |
|---|---|
| `match<sip:5m>`（单 key） | `sip` |
| `match<sip,dport:5m>`（多 key） | 全部 key 各生成唯一值（实体 = key 元组） |
| `on each s` + `entity(<type>, s.event_id)` | `event_id` |

- 显式给出时才用 `hit<sip: 500>`；显式值应与推断结果一致。
- "显式与推断不一致"（VN23）与"字段不在 schema"（VN22）**未实现**（§7.2）。

## 4. 校验与错误码

### 4.1 校验期（`wfgen lint` / `gen` 加载阶段）

| 码 | 触发 | 消息要点 |
|---|---|---|
| VN1 | `background` 无 stream | `background block must contain at least one stream` |
| VN2 | 速率 ≤ 0 | `stream '<s>': rate must be greater than 0` |
| VN3 | stream 不在已加载 schema | `stream '<s>' not found in loaded schemas (.wfs windows)` |
| VN9 | 同一 `use` 内重复字段 | `… has duplicate field '<f>' in use` |
| VN10 | 注入用例的 stream 不在 `background` 中声明 | `injection case stream '<s>' is not declared in background` |
| VN11 | `use` 字段不在该 stream 的 schema | `… field '<f>' not found in schema '<w>'` |
| VN12 | `use` 重复了实体字段 | `… repeats entity field '<f>'` |
| VN14 | `for RULE` 指向的规则不在 `.wfl` | `… targets rule '<r>' not found in WFL files` |
| VN17 | `use({...})` 顶层不是 object | `… use({...}) 的顶层必须是 JSON object` |
| VN20 | 使用旧语法 `hit<N%>` / `with(N)` | 见 §5.1（**解析期**报错） |
| VN21 | 实体个数为 0、`x 0`、或没有任何事件组 | `… 实体个数必须大于 0 / 第 k 个事件组 x 0 / 至少需要一个 use … x N 事件组` |
| VN25 | `spread` 超过 `#[duration]` | `spread 20m` 超过场景 duration `10m` |

其他层级的校验：`SC2/SC2a/SC3/SC4`（stream 与规则的绑定关系）、`SV2/SV3/SV4/SV6/SV7/SV8`
（场景基础值、字段类型、oracle 参数）。

> 旧号（占比域、`expect` 规则存在性、`seq`/`not(...)` 步骤相关）随旧语法一起删除。

### 4.2 生成期断言（已落地）

| 码 | 触发 | 消息形态 |
|---|---|---|
| INJ1 | `hit` 实体未产出告警 | `hit 用例第 %d 个实体（%s=%s）不会触发规则 <r>：条数/阈值 <bind> %d/%d（…）` |
| INJ2 | `near_miss`/`miss` 实体产出了告警 | `%s 用例第 %d 个实体（%s=%s）会触发规则 <r>：命中 %s 路径（emit_time=%s）` |

把"最后由 `wfgen verify` 红一行百分比"提前到**生成期 + 精确定位到实体**。

实现（`wfgen/src/inject_assert/`，由 `cmd_gen` 在 `run_oracle` 之后、写
`.except.jsonl` 之前调用，与期望文件同一门控）：

- 判定**复用** `gen` 本来就要跑的 oracle 结果（`OracleAlert`），不额外评估。
- 按 `(rule_name, entity_id)` 建索引；实体值按 `entity_id` 同口径渲染
  （Str 透传 / 数字整数不带 `.0` / 容器退化为 `[array]`、`[object]`）。
- 失败**一次性汇总**报出（`WfgenReason::Generation`），每类最多列 5 条明细 + 同码总数，
  避免语料级失败（上万个实体）刷屏。
- 断言覆盖不到的实体（`entity(...)` 不是单一字段、实体字段不在本次键覆盖里）会计入
  `GenResult::unasserted_inject_entities`，由 `gen` 打一条 Warning 说明——不静默跳过。
- `--no-oracle` / `--no-wfl` / 无 `--out`（即不生成期望文件的场景）不做断言。
- `miss` 的 `x N` 是"N 条各自独立键"，因此**实体数 = 用例头实体数 × Σ N**，比
  `hit`/`near_miss` 的口径大 N 倍（同一实体成簇就会报警，独立键是 `miss` 能构造出来的前提）。

## 5. 从旧语法迁移

### 5.1 VN20（旧语法一律报错）

```
VN20 旧注入语法已移除：`hit<N%>` 里的 N 是 stream 配额的百分比，实际数量由
「配额 × 比例 ÷ 每实体条数」推出，与「数量写在用例里」不能共存。请改写为显式的
实体个数与每实体条数，例如 `hit<sip: 500> for RULE STREAM { use(...) x 12 }`；
实体个数 ≈ round(配额 × N%) ÷ 每实体条数。
```

旧语法清单：`hit<20%>` / `near_miss<10%>` / `miss<60%>`、`<field> seq { … }`、
`use(...) with(N)`、`not(...) within(...)`、`expect { … }`、`traffic` 关键字、`injection` 关键字、
`oracle { … }`。

### 5.2 折算公式

```text
实体数   = floor( round(rate × duration × pct / 100) ÷ ΣN )
新写法   = mode<[field:]实体数> for RULE STREAM { use(...) x N … }
```

迁移示例（旧语义的等价值）：

| 场景 | rate × duration | 模式 | pct | ΣN | 实体数 | 注入条数 | 新写法 |
|---|---|---|---|---|---|---|---|
| `templates/ssh_brute_force` | 50/s × 10m = 30000 | hit | 20 | 12 | 500 | 6000 | `hit<sip: 500>` + `x 12` |
| | | near_miss | 10 | 2 | 1500 | 3000 | `near_miss<sip: 1500>` + `x 2` |
| | | miss | 70 | 1 | 21000 | 21000 | `miss<sip: 21000>` + `x 1` |
| `examples/distinct/port_scan` | 100/s × 10m = 60000 | hit | 40 | 12 | 2000 | 24000 | `hit<sip: 2000>` + `x 12` |
| `examples/avg/dns_tunnel` | 100/s × 10m = 60000 | hit | 30 | 3 | 6000 | 18000 | `hit<sip: 6000>` + `x 3` |
| `examples/sum/data_exfil` | 100/s × 15m = 90000 | hit | 25 | 4 | 5625 | 22500 | `hit<sip: 5625>` + `x 4` |
| `examples/conv/top_scanners` | 40/s × 2h = 288000 | hit | 25 | 5 | 14400 | 72000 | `hit<sip: 14400>` + `x 5` |

**背景速率的折算**（迁移时可选，用于让新总条数贴近旧总条数）：

```text
旧总条数 ≈ 配额 = round(rate_old × duration)        （旧写法：背景 = 配额 − 注入）
新总条数 = 背景配额 + 注入 = round(rate_new × duration) + 注入条数
令新 = 旧 ⇒ rate_new = (旧总条数 − 注入条数) / duration
```

新写法下背景与注入不再互相影响，因此这个折算只在"想保留旧文件的总条数"时需要做一次。

### 5.3 迁移注意

- 旧 `near_miss` 会把条数夹到 `min(N, 阈值−1)`：若新写法照抄旧 `with(N)`，可能**真的报警**
  （`near_miss` 现在是硬断言，不夹取）。
- 旧语法里"某模式实际生成 0 条"的用例（阈值夹取后为 0）迁移后会产生真实事件，总量不变但背景相应减少。
- 多步骤、同一 alias 声明多条 stream（含 `wave(...)`）的场景需人工复核。
- 迁移后有两处语料按"模式 = 硬断言"调整过（§5.2 表里给的是旧语法等价值）：
  - `examples/conv/top_scanners`：`hit<14400>` 与 conv `top(2)` 冲突（每窗口只有 2 条输出）
    → `hit<2>`；`near_miss` 由 `x 3`（达到 `on close` 的 `distinct >= 3`）→ `x 2`（达不到）。
  - `examples/sum/data_exfil`：`near_miss` 的 `bytes` 由 `12000000` → `9000000`
    （45MB < 50MB 关闭阈值；原值 60MB 靠 `on close` 触发，与 `near_miss` 不得报警冲突）。

## 6. 运行闭环

```text
wfg + wfs + wfl
   -> wfgen gen --scenario ... --out ... [--send]
   -> wfusion batch
   -> actual alerts
   -> wfgen verify / wfl verify
   -> 断言判定 + 报告
```

`wfgen gen` 在未 `--no-oracle` / 未 `--no-wfl` 时会生成 `.except.jsonl` / `.except.meta.jsonl`
（期望告警），`wfgen verify` 据此对拍。

## 7. 落地状态

### 7.1 已落地

- 语法与 AST：`background` / `inject`、`hit<[field:]N> for RULE STREAM { use … x N }`、
  三种 `value_source`（含整份 JSON 内联）、`spread`。
- 数量显式：删除 `compute_cluster_count*` 与从 stream 配额推导的整条链路。
- 模式不改数字：`near_miss` 不再夹取，`hit`/`near_miss` 共用同一套条数口径。
- 旧语法在**解析期**报 VN20。
- `expect` 块删除；期望文件改为"未 `--no-oracle` / 未 `--no-wfl` 即生成"。
- 语料迁移：本仓库内 14 个 tracked `.wfg`（`crates/wfgen/examples` 6 · `crates/wfadm/templates` 4 ·
  `docker/default_setting` 4）与相关 Rust 测试用例。
- `.wfg` 只有一份解析器实现（本仓库 `wfgen`）；`wp-reactor/wf-lang` 里的旧副本已删除，
  `wfadm` 改为用 `wfgen` 的解析器校验场景。
- 生成期硬断言 INJ1/INJ2（§4.2）：`hit` 每个实体必须报警、`near_miss`/`miss` 每个实体
  必须不报警；复用 oracle 结果，失败在写期望文件之前报出。
- 实体键空间按用例分段（`InjectEntities::next_entity_base`）：用例之间实体值不重叠，
  否则 `hit` 与 `near_miss` 会指向同一实体、两个口径互相污染。
- 背景与注入完全分离：背景保留自己的配额（`rate × duration`），注入在其上叠加
  （旧口径 `背景 = 配额 − 注入` 及其 `inject_counts` 链路已删除）。
- 14 个仓内语料在断言下**全部通过**；其中 2 个按断言口径调整过（§5.3）。

### 7.2 未落地 / 未决

| 项 | 状态 |
|---|---|
| `replay STREAM { use from "f" }` 照单发货 | 未实现 |
| `without(...)` 步骤（取代旧 `not(...) within(...)`） | 未实现 |
| `use from "path"` 的路径解析（相对 `.wfg` 所在目录） | 未实现；残留 `File` 会明确报错而非静默空字段 |
| 文件为数组时 N 条循环取用 | 未实现 |
| 时间**均匀**铺开 | 部分：`spread` 已可写并覆盖窗口长度，铺开策略仍是"随机簇起点 + 窗口内铺开" |
| VN22 / VN23（实体字段存在性与推断一致性）、VN24（事件组数 > 步骤数）、VN26（replay 文件） | 未实现；事件组数超限目前是生成期的 `exceeds rule step count` |
| 外部语料迁移：`wf-rules` / `wf-examples` / `wf-conf-example` | 未迁移 |
| 文档（getting-started / cli / wfadm 模板说明 / CHANGELOG） | 未同步 |

未决（不阻塞实现）：

- 生成期断言在**分片 / 多实例**下的口径（当前 oracle 是单机内存模型）。
- `replay` 与 `inject` 并存时的**时间对齐**（谁决定 watermark 推进）。
- 实体分段的**上限**：分段使实体值不重叠依赖 Ip 映射的 24 位地址空间（`10.a.b.c`），
  因此一个场景的实体 id 总数需 < 2^24（约 1670 万，实际够用）。超出后段会重叠，
  目前未做校验，建议改成明确报错。

### 7.3 扩展规划

**P1（优先）**

- `without(...)` 的生成支持与严格约束定义。
- 实体分布扩展：热点（Zipf）与新老实体比例。

**P2（增强真实性）**

- 速率模型扩展：`spike`、`jitter`、`diurnal`（昼夜曲线）。
- 跨流注入：同一实体在多 stream 的联动序列。
- 场景矩阵：同一场景的多参数批量运行。

**P3（工程效率）**

- 模板化：`template/param` 复用场景片段。
- 基线对比：与历史结果自动比对回归漂移。
- 报告输出：自动生成 markdown/html 对比报告。
