# WFG 注入语义重设计 — 显式数量 + 硬断言

> 状态：**已评审确认**（§8 决策已定稿），实现中。
>
> 评审记录：决策 1 **取消**（实体键不强制写在用例头、从规则推断）；决策 2–7 按建议采纳。
> 见 §8 决策记录表。
>
> 范围：`injection` 块的语法与语义——数量、模式、断言、时间铺开、背景/注入分离。
>
> 不含：`faults` 块、`scenario` 属性、oracle 对拍机制（复用现有实现）。
>
> 相关文档：[wfadm.md](./wfadm.md) · [wfl_seq_design.md](./wfl_seq_design.md) ·
> [getting-started.md](../useage/getting-started.md) · [cli.md](../useage/cli/cli.md)

## 1. 背景

### 1.1 旧语义：数量是算出来的

```wfg
hit<20%> auth_events {
  sip seq { use(result="failed") with(12) }
}
```

```text
stream 配额 = 该 stream 速率 × duration                # derive_total, syntax/mod.rs:151
模式预算    = round(stream 配额 × 比例%)
实体数      = min over stream [ 模式预算 ÷ Σ每簇条数 ]   # helpers/plan.rs:76 / :132
总条数      = Σ每簇条数 × 实体数
背景条数    = stream 配额 − 注入条数                    # datagen/mod.rs:105
```

用户写下来的是**速率与比例**，条数是算出来的。上例 = `6000 ÷ 12 = 500` 个实体、6000 条。

### 1.2 缺口清单

| # | 缺口 | 证据 | 影响 |
|---|---|---|---|
| A1 | `hit<20%>` 的 `%` 不是实体比例也不是事件比例，而是 **stream 配额的百分比** | `%` 经 `compute_cluster_count*` 两次乘除后才落到实体数 | 读到 `20%` 无法知道会造多少 |
| A2 | 实体数 = 配额 × 比例 ÷ Σ每簇条数 | 同上 | 隐式除法；改任何一项都连带改总量 |
| A3 | `traffic { gen 50/s }` 参与注入量 | `compute_stream_totals`（`dispatch.rs:18`）用速率分摊总额度 | 改背景速率会改变定向构造的数量 |
| A4 | 配额不足 → 静默 0 条；`with(N)` 小于阈值 → 静默不触发 | `compute_cluster_count` 返回 0 时 `hit.rs:56` 直接返回空 | 写错没有任何反馈 |
| B1 | `with(N)` 既是"每簇条数"又是"分簇除数" | `compute_hit_counts` + `compute_cluster_count*` | 一个数字两个职责 |
| B2 | 未写 `use` 的步骤被**自动补到阈值**，且这些事件字段值随机 | `compute_hit_counts`（`helpers/plan.rs:141`）的 fill | 隐式补全，用户不知道生成了什么 |
| B3 | `with(N)` 的 N 条事件 predicates 完全相同 | `map_use_predicates_to_rule_steps`（`generate.rs:157`）克隆 N 份 | 想要 N 种取值做不到 |
| C1 | `near_miss` 把**最后一个** `use` 当边界，条数夹到 `min(N, T-1)`，前面步骤补到阈值、后面置 0 | `compute_near_miss_counts`（`helpers/plan.rs:29`） | 模式偷改用户写的数字 |
| C2 | `miss` 走"**允许** predicates 与规则 filter 冲突"的旁路 | `plan_use_steps_allowing_filter_conflicts`（`non_hit.rs:150`） | 同一份 `use` 换模式合法性就变 |
| C3 | `hit` 则**禁止**该冲突并报错 | `validate_use_step_predicates`（`plan.rs:238`） | 规则不可从语法本身读出 |
| D1 | `with(N)` 与阈值无固定关系：`count` 看条数，`sum/avg/conv` 看"值 × 条数" | `data_exfil.wfg`：`hit` 用 `with(4)`、`near_miss` 用 `with(5)`（条数更多却不命中主路径） | 无法靠直觉写对 |
| D2 | 规则可能有多条触发路径（`on event` / `on close` / `seq` / `conv`），靠哪条命中不可知 | 同上例注释：`near_miss` 是**故意**让 `on close` 触发的 | 命中与否不可预判 |
| E1 | **`expect { hit(rule) >= 90% }` 的度量与阈值从不被求值** | 全仓只有 `validate/syntax.rs:204` 读 `check.value`（范围校验）与 `injection_targets.rs` 读 `check.rule`（定位规则）；无任何消费 `metric`/`op` 的代码 | 用户以为写了断言，实际无人校验 |
| E2 | `expect` 里的 hit/near_miss/miss 与注入块的是两套独立词汇 | 同上 | 概念重复且不一致 |
| F1 | 时间铺开是隐式的（簇起始随机 + 窗口内铺开） | `hit.rs` `cluster_start_secs` / `compute_window_bounds` | 峰值速率不可预期，可能撞 `limits` throttle 或窗口驱逐 |
| G1 | `sip seq { … }` —— 实体键写在 `seq` 前，读起来像"名为 sip 的 seq" | `parse_seq_block`（`syntax/inject.rs:91`） | 语法像"块名"而非"实体键" |
| G2 | `for RULE` 可省，省略时靠 `expect` 反推目标规则；**25 个现有 `.wfg` 无一使用 `for`** | `validate/syntax.rs` VN13；`injection_targets.rs:11` | 两种写法并存 |
| G3 | `not(...) within(...)` 是另一类 step，没有 `with`，"不含这些条件"语义隐晦 | `syntax/inject.rs:135` | 需要读源码才能理解 |
| G4 | 多步骤 seq 下"命中"指走到第几步，没有显式表达 | `chain_attack.wfg` 的 `then use(...) with(3)` | 意图不可读 |
| H1 | 背景事件也参与判定 | 注入是**从 stream 配额里切出来**的（`datagen/mod.rs:105`），背景 = 总额 − 注入 | "命中率"的分母混了背景 |

### 1.3 目标

1. **写下来的数字 = 生成出来的数字**：实体数与每实体条数都是用户写的，不做隐式除法。
2. **模式是硬断言**：`hit` 必须触发、`miss` 必须不触发，判定复用内置 oracle（生成期即可报错），不再需要用户写百分比。
3. **一件事一个旋钮**：数量归数量、模式归模式、背景归背景。

## 2. 设计原则

| 原则 | 对应消解的缺口 |
|---|---|
| P1 数量显式，不做隐式除法 | A1–A4 |
| P2 一个字段一个职责：数量、模式、值、时间各自独立 | B1–B3、C1–C3 |
| P3 模式是断言方向，不改任何数量与字段值 | C1–C3、D1 |
| P4 断言可判定：由系统（复用 oracle）判定并在生成期报错 | D1–D2、E1–E2 |
| P5 背景与注入分离：背景只表示"额外随机流量" | A3、H1 |
| P6 时间显式 | F1–F2 |
| P7 语法自解释：实体键、目标规则都在用例头部写清 | G1–G2、G4 |

## 3. 语法

### 3.1 旧语法（本次删除，不再兼容）

```ebnf
injection_block := "{" injection_case* "}"
injection_case  := mode "<" percent ">" [ "for" IDENT ] IDENT "{" seq_block "}"
mode            := "hit" | "near_miss" | "miss"
seq_block       := IDENT "seq" "{" seq_step* "}"
seq_step        := [ "then" ] "use" "(" predicates ")" "with" "(" INT ")" [ ";" ]
                 | [ "then" ] "not" "(" predicates ")" "within" "(" duration ")"
predicates      := predicate { "," predicate }
predicate       := IDENT "=" attr_value
attr_value      := STRING | number | duration | "true" | "false"
expect_block    := "{" expect_stmt* "}"
expect_stmt     := metric "(" IDENT ")" cmp_op expect_value [ ";" ]
```

### 3.2 新语法

```ebnf
scenario        := { use_decl } [ attr ] "scenario" name [ "<" "seed" "=" INT ">" ] "{" background inject "}"
background      := "background" "{" stream_decl+ "}"
stream_decl     := "stream" IDENT "rate" rate_expr
rate_expr       := FLOAT "/" ("s" | "m" | "h") | "wave" "(" ... ")"

inject          := "inject" "{" inject_case+ "}"
inject_case     := mode "<" [ IDENT ":" ] INT ">" "for" IDENT IDENT "{" inject_body "}"
mode            := "hit" | "near_miss" | "miss"
inject_body     := { event_group } [ "spread" duration ]
event_group     := [ "then" ] "use" value_source "x" INT
value_source    := "(" predicates ")"                  (* 按字段覆盖 *)
                 | "{" json_object "}"                 (* 整份 JSON 内联 *)
                 | "from" STRING                       (* 整份 JSON 来自文件 *)
replay          := "replay" IDENT "{" "use" "from" STRING "}"   (* 照单发货，不做实体数学 *)
```

要点：

- `hit<500>` —— `500` 是**实体个数**。实体标识字段**默认从规则推断**（match 规则的 match key、
  `on each` 规则的 entity 表达式里的字段）；仅当推断不出（多 key 无法确定、entity 是复合表达式）或
  需要消歧时，才用显式形式 `hit<sip: 500>`。
- `for RULE` **必填**，不再从 `expect` 反推。
- `x N` 是**每个实体在该步骤上的条数**（取代 `with(N)`）。
- `expect` 块**删除**。
- `traffic` → `background`（改名以明示"只做背景"）。
- `not(...) within(...)` → 改为 `without` 步骤（§4.6，待决策）。

### 3.3 完整示例

```wfg
use "auth.wfs"
use "../rules/ssh_brute_force.wfl"

#[duration=10m]
scenario ssh_brute<seed=42> {
  // 背景噪声：只表示"除定向构造外还有多少随机流量"
  background {
    stream auth_events rate 50/s
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

多步骤：

```wfg
    hit<sip: 500> for chain_attack conn_events {
      use(action="syn")                       x 5
      then use(action="login_fail", dport=22)  x 6      // 500 × (5+6) = 5500 条
    }
```

文件形态：

```wfg
    hit<event_id: 200> for object_on_each sdm_event {
      use(from "raw/sxf_edr_sip_atk_alarm.json")             // 200 条，值在文件记录间循环
    }

    replay sdm_event { use(from "raw/big.ndjson") }          // 文件有多少条就发多少条
```

## 4. 语义

### 4.1 数量（P1）

```text
实体数      = 用例头的 INT
每实体条数  = 第 k 个 event_group 的 x N
注入条数    = 实体数 × Σ N
背景条数    = round(该 stream rate × duration)          # 与注入无关
该 stream 总条数 = 背景条数 + 注入条数
```

例：`hit<sip: 500> … { use(...) x 12 }` = **500 个实体、6000 条注入**；`background { stream s rate 50/s }` + `#[duration=10m]` = 30000 条背景；该 stream 合计 36000 条。

**没有任何隐式除法**：三个数字（实体数、每实体条数、背景速率）各自独立且可见。

### 4.2 模式 = 硬断言（P3、P4）

| 模式 | 数量 | 字段值 | 断言（生成期判定） |
|---|---|---|---|
| `hit` | `x N` 原样 | 由 `use` 决定 | **每个实体都必须产出 ≥1 条告警** |
| `near_miss` | `x N` 原样 | 满足规则 filter，但达不到阈值 | **每个实体都不得产出告警** |
| `miss` | `x N` 原样 | 可以违反 filter | **每个实体都不得产出告警** |

- 模式**不修改**任何数量、不修改任何字段值（消解 C1–C3）。
- `near_miss` 与 `miss` 的区别**只剩构造意图**（是否满足 filter）；断言强度相同。
- 断言以**实体**为单位，背景事件不参与。
- 判定复用内置 oracle（`oracle/mod.rs` 的 `RuleEngine` + `CepStateMachine`）：`gen` 本来就要跑它算期望告警写 `.except.jsonl`，因此断言是"读已有结果"，不是新实现规则语义。
- 多条触发路径（`on event` / `on close` / `seq` / `conv`）天然被覆盖：只要任一 路径产出告警即算"报警"。

### 4.3 值与值模板（P2）

- `use(preds)` / `use({json})` / `use(from "f.json")` 提供该步骤的字段值。
- 默认：该步骤的 N 条事件使用**同一份**值（与旧行为一致）。
- 若 `use(from "f.json")` 的文件顶层是**数组**：N 条事件在记录间**循环取用**（`N > 记录数` 时回绕）。这解决 B3。
- `_stream` / `_window` / `_timestamp` 出现在 JSON 里时**忽略并给提示**（不报错）。

### 4.4 `replay`：照单发货

```wfg
replay sdm_event { use(from "raw/big.ndjson") }
```

- 文件有多少条就发多少条；不做实体数学、不参与断言（它是一条"灌数据"通道，取代 `wfgen send --input` 的多数用法）。
- 与 `inject` 可并存：`replay` 的数据 + `inject` 的定向构造 + `background` 的噪声。

### 4.5 背景与注入分离（P5）

- `background` 只决定"除定向构造外的随机流量"，其速率**不再影响**注入条数（消解 A3）。
- 注入条数完全由用例头与 `x N` 决定（消解 A4 的静默 0）。
- 总条数 = 背景 + 注入（相加，不再"从配额里切"）。

### 4.6 `without` 步骤（取代 `not(...) within(...)`）

```wfg
hit<sip: 200> for chain_attack conn_events {
  use(action="syn")                x 5
  then without(action="login_fail") x 1     // "期间不含失败登录"
}
```

原语义（`SeqStep::Not`）是"不含这些条件的事件"，但旧语法里它**没有条数**、且与 `use` 混在同一个 step 列表里，容易被读成"否定上一步"。新语法把它显式化为一个 `without` 步骤并给条数。

### 4.7 时间铺开（P6）

- 默认：注入事件在该标注的 `duration` 内**均匀铺开**（不再用"随机簇起点 + 窗口内铺开"这种内部策略）。
- 显式旋钮：`spread <duration>`（必须 ≤ `#[duration]`）。
- 可选校验：注入后的**峰值速率**若超过该窗口 `limits` 的 throttle 阈值，给 WARN（消解 F2）。

### 4.8 校验规则

| 类别 | 规则 |
|---|---|
| 旧语法 | 一律报错，并给出等价值建议（`hit<20%>` → `hit<sip: 500>`），见 §6 |
| 数量 | `<field: 0>` 或 `x 0` → 错误 |
| 实体字段 | 默认从规则推断（match key / entity 表达式字段）。显式给出时必须存在于该 stream 的 schema，且与推断结果一致（不一致 → 错误）；推断不出时**要求显式**（否则错误）|
| 步骤对应 | `event_group` 个数 > 规则步骤数 → 错误（保留旧校验文案口径）|
| `spread` | 必须 ≤ `#[duration]` |
| `background` | 至少一个 stream；速率 > 0 |
| 断言 | 生成期逐个实体判定，不一致 → 错误（见 §5）|

## 5. 错误信息规格

### 5.1 校验期（`wfgen lint` / `gen` 加载阶段）

| 码 | 触发 | 消息模板 |
|---|---|---|
| VN20 | 使用旧语法 `hit<20%>` | `hit<%> 已于 vX.Y 移除：`%` 表示 stream 配额的百分比、且实际数量由「配额 × 比例 ÷ 每实体条数」推出。请改写为实体个数，例如 `hit<sip: 500>`（按旧语义等价值见下方提示）` |
| VN21 | `<field: 0>` 或 `x 0` | `injection 数量必须大于 0：%s` |
| VN22 | 显式实体字段不在 schema | `实体字段 `%s` 不在 stream `%s` 的窗口 `%s` 中` |
| VN23 | 无法推断实体字段 / 与推断结果不一致 | `无法从规则 `%s` 推断实体字段（match key 为 [%s]），请显式写成 `hit<字段: %d>`；或与推断结果不一致（当前：%s）` |
| VN24 | `event_group` 数 > 步骤数 | `injection 声明了 %d 组 use，但规则 `%s` 只有 %d 个步骤` |
| VN25 | `spread` > duration | ``spread %s` 超过场景 duration %s` |
| VN26 | `replay` 缺文件/文件无记录 | `replay 文件 `%s` 不存在或为空` |

### 5.2 生成期断言（新能力）

| 码 | 触发 | 消息模板 |
|---|---|---|
| INJ1 | `hit` 实体未产出告警 | `hit 用例第 %d 个实体（%s=%s）不会触发规则 `%s`：%s` —— 第二段给可定位原因（如"12 条 ≥ 阈值 10，但 events filter 要求 service=="ssh"，本用例未设置"）|
| INJ2 | `near_miss`/`miss` 实体产出了告警 | `%s 用例第 %d 个实体（%s=%s）会触发规则 `%s`：命中 %s 路径（%s）` —— 例如"命中 on close 路径（sum 60MB ≥ 50MB）" |

这两条把原来"最后由 `wfgen verify` 红一行百分比"的问题提前到**生成期 + 精确定位到实体**。

## 6. 示例：改造前后对照

### 6.1 `templates/ssh_brute_force.wfg`（`count >= 10`）

改造前：

```wfg
#[duration=10m]
scenario brute_force_detect<seed=42> {
  traffic { stream auth_events gen 50/s }
  injection {
    hit<20%>       auth_events { sip seq { use(result="failed", service="ssh") with(12) } }
    near_miss<10%> auth_events { sip seq { use(result="failed", service="ssh") with(2)  } }
    miss<70%>      auth_events { sip seq { use(result="success", service="ssh") with(1) } }
  }
  expect {
    hit(ssh_brute_force) >= 90%
    near_miss(ssh_brute_force) <= 5%
    miss(ssh_brute_force) <= 0.5%
  }
}
```

等价换算（旧语义，`50/s × 600s = 30000`）：
`hit`：`30000 × 20% = 6000`，`÷12` → **500 实体**；`near_miss`：`3000 ÷ 2` → **1500 实体**；`miss`：`21000 ÷ 1` → **21000 实体**。

改造后：

```wfg
#[duration=10m]
scenario brute_force_detect<seed=42> {
  background { stream auth_events rate 50/s }

  inject {
    hit<sip: 500>       for ssh_brute_force auth_events { use(result="failed", service="ssh") x 12 }
    near_miss<sip: 1500> for ssh_brute_force auth_events { use(result="failed", service="ssh") x 2  }
    miss<sip: 21000>     for ssh_brute_force auth_events { use(result="success", service="ssh") x 1 }
  }
}
```

（`expect` 删除，断言由 §5.2 承担。）

### 6.2 `examples/sum/data_exfil.wfg`（`on event sum >= 100MB` + `on close sum >= 50MB`）

改造前的 `near_miss<20%>` 是**故意靠 `on close` 触发**的（文件注释明说）。在旧语义下用户必须心算"5 × 12MB = 60MB，介于 50MB 与 100MB 之间"；新语义下：

- 若想让 `near_miss` **不报警**：把值调到 `on close` 阈值以下（例如 `bytes=5000000 x 5` = 25MB），生成期断言会确认。
- 若想验证"`on event` 不命中但 `on close` 命中"：那**不能**用 `near_miss`（它断言"不得报警"），应改用 `hit`（断言"必须报警"）并在注释里说明命中路径。这消除了旧语法里"模式名与实际意图不符"的歧义。

## 7. 影响面清单

### 7.1 代码

| 层 | 文件 | 改动 |
|---|---|---|
| AST | `wfg_ast.rs` | `AttrValue` 增结构化值；`ValueSource`（preds / json / file）；`ReplayCase`；删 `ExpectBlock`/`ExpectCheck` |
| 语法 | `wfg_parser/syntax/inject.rs` | `hit<field:N>`；`use … x N`；三种 `value_source`；`use(` 后补 `ws_skip` |
| | `wfg_parser/syntax/attrs.rs` | `parse_attr_value` 增 `{…}` / `[…]` / `null` |
| | `wfg_parser/syntax/{traffic,mod}.rs` | `traffic` → `background`；删 `expect` 分支 |
| 校验 | `validate/syntax.rs` | 新增 VN20–VN26；调整 VN9/VN11/VN12（seq 实体 → 用例头字段） |
| | `validate/{scenario,stream_schema,gen_compat}.rs` | background 改名与数量校验 |
| | `injection_targets.rs` | `for RULE` 必填，删除"从 expect 反推"分支 |
| 生成 | `datagen/inject_gen/{dispatch,extract,structures}.rs` | 数量显式；`RuleStructure` 增实体数；支持 `replay` |
| | `datagen/inject_gen/{hit,near_miss,non_hit}.rs` | 去掉簇数推导与 `near_miss` 夹取；删除 filter-conflict 旁路 |
| | `inject_gen/helpers/{plan,generate}.rs` | 删 `compute_cluster_count*`、`compute_hit_counts` 的隐式 fill、`compute_near_miss_counts` 的夹取 |
| | `datagen/mod.rs` | 背景 = 速率推导（不再 `总 − 注入`） |
| 断言 | 新增（复用 `oracle/mod.rs`） | 逐实体判定 + INJ1/INJ2 |
| 输出 | `output/{jsonl,arrow_ipc}.rs` | 结构化值序列化；object/array 列补 `wf.wfl.field_type` metadata |
| 命令 | `cmd_gen.rs` / `cmd_lint.rs` | `expect` 删除后"是否生成期望文件"改为：有 `--out` 且未 `--no-oracle` 即生成 |

### 7.2 语料

| 类别 | 数量 | 说明 |
|---|---|---|
| tracked `.wfg` | **约 25 个** | `crates/wfgen/examples`(6) · `crates/wfadm/templates`(4) · `docker/default_setting`(4) · `wf-rules/models`(4) · `wf-examples/core/meta_disable`(4) · `wf-examples/nginx_log_stats`(2) · `wf-conf-example`(1) |
| 含 WFG 文本的 Rust 测试 | **4 个文件 / 46 处** | `wfg_parser/tests.rs`(5) · `validate/tests/syntax.rs`(19) · `datagen/tests/inject/correctness.rs`(12) · `datagen/tests/inject/syntax.rs`(10) |

### 7.3 机械迁移表（旧语义等价值）

单 stream、单步骤场景下 `实体数 = floor(round(rate × duration × pct/100) ÷ ΣN)`：

| 场景 | rate × duration | 模式 | pct | ΣN | 旧实体数 | 旧注入条数 | 新写法 |
|---|---|---|---|---|---|---|---|
| `templates/ssh_brute_force` | 50/s × 10m = 30000 | hit | 20 | 12 | 500 | 6000 | `hit<sip: 500>` + `x 12` |
| | | near_miss | 10 | 2 | 1500 | 3000 | `near_miss<sip: 1500>` + `x 2` |
| | | miss | 70 | 1 | 21000 | 21000 | `miss<sip: 21000>` + `x 1` |
| `examples/distinct/port_scan` | 100/s × 10m = 60000 | hit | 40 | 12 | 2000 | 24000 | `hit<sip: 2000>` + `x 12` |
| `examples/avg/dns_tunnel` | 100/s × 10m = 60000 | hit | 30 | 3 | 6000 | 18000 | `hit<sip: 6000>` + `x 3` |
| `examples/sum/data_exfil` | 100/s × 15m = 90000 | hit | 25 | 4 | 5625 | 22500 | `hit<sip: 5625>` + `x 4` |
| `examples/conv/top_scanners` | 40/s × 2h = 288000 | hit | 25 | 5 | 14400 | 72000 | `hit<sip: 14400>` + `x 5` |

> 需人工复核的例外：`examples/multi_step/chain_attack.wfg`（多步骤 + 部分步骤可能未覆盖注入 stream，`extract.rs` 的 SC6 会跳过）与 `examples/count/brute_force.wfg`（同一 alias 声明了两条 stream，含 `wave(...)`）。

**背景速率的折算**（迁移时可选，用于保持总条数不变）：

```text
旧总条数 = round(rate_old × duration)
旧背景   = 旧总条数 − 旧注入条数
新背景   = round(rate_new × duration)
令两者相等 ⇒ rate_new = (旧总条数 − 旧注入条数) / duration
```

### 7.4 文档

`docs/useage/getting-started.md`、`docs/useage/cli/cli.md`、`docs/design/wfadm.md`（模板说明）、CHANGELOG。`docs/design/wfl_seq_design.md` §2.3 提到"wfgen 注入 seq"之处需同步。

## 8. 决策记录

| # | 决策 | 结论 |
|---|---|---|
| 1 | 实体键与数量的写法 | **取消该决策项** —— 不强制写实体键，默认从规则推断；主写法 `hit<500>`，`hit<sip: 500>` 仅用于显式消歧 / 多 key |
| 2 | 每实体条数的关键字 | 采纳 `x N`（取代 `with(N)`）|
| 3 | `traffic` 是否改名 | 采纳 `background` |
| 4 | `for RULE` 是否必填 | 采纳**必填**，删除"从 expect 反推目标规则"这条隐式路径 |
| 5 | `near_miss` / `miss` 是否合并 | 采纳**保留两态**：区别仅"构造意图"（是否满足 filter），断言强度相同 |
| 6 | 文件为数组时 N 条事件 | 采纳**循环取用**（`N > 记录数` 时回绕）|
| 7 | 旧语法删除方式 | 采纳**直接报错 + 等价值建议**（VN20），并附一次性迁移脚本（按 §7.3 公式）|

### 决策 1 在实现中的落实

实体字段的推断来源，按规则形态分三种：

| 规则形态 | 实体字段 |
|---|---|
| `match<sip:5m>`（单 key） | `sip` |
| `match<sip,dport:5m>`（多 key） | 全部 key 各生成唯一值（实体 = key 元组）|
| `on each s` + `entity(<type>, s.event_id)` | `event_id` |

- 显式给出时必须与推断结果一致，否则 VN23。
- 推断不出（无 key 的 match、entity 表达式是复合式如 `concat(a,b)`）→ **要求显式**，否则 VN23。

### 未决（不阻塞实现，另出文档）

- 生成期断言在**分片 / 多实例**下的口径（当前 oracle 是单机内存模型）。
- `replay` 与 `inject` 并存时的**时间对齐**（谁决定 watermark 推进）。

## 9. 落地清单

| 阶段 | 内容 | 产出 / 验收 |
|---|---|---|
| 1 ✅ | 本文档评审定稿（§8） | 已完成：决策记录见 §8，状态行已更新 |
| 2 | 语法与 AST 改造；旧语法给 VN20（含等价值建议）；`expect` 删除 | `wfgen lint` 对旧语料报 VN20 且给出建议；新语法可解析 |
| 3 | 生成器改造：数量显式、去隐式除法/补全/夹取、背景与注入分离 | 单测：`500 × 12 = 6000` 精确断言；`near_miss` 不再改数字 |
| 4 | 生成期硬断言（INJ1/INJ2，复用 oracle） | 单测：故意写错的 hit/miss 用例在生成期报错且定位到实体 |
| 5 | 语料迁移（25 个 `.wfg` + 46 处测试）+ 文档 + CHANGELOG | 全量 `cargo test` 绿；`wfgen gen` 对 25 个语料生成量与迁移前一致（按 §7.3 折算） |
| 6 | 叠加 issue #72 的三件事：结构化值、`use(from …)`、`on each` 注入 | 端到端：`wfgen gen --send` + daemon 产出告警 |

阶段 2–3 是"大改"的主体；阶段 5 的验收基准是**迁移前后生成量一致**（因为旧语义可机械换算），这使大改的风险可控。
