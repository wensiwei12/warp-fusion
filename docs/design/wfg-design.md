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

background_block = "background" , "{" , { ( stream_stmt | entity_stmt ) } , "}" ;
stream_stmt      = "stream" , IDENT , "gen" , rate_expr ;
entity_stmt      = "entity" , IDENT , "." , IDENT , "zipf" , "(" , zipf_args , ")" ;
zipf_args        = "pool" , "=" , INTEGER , { "," , zipf_arg } ;
zipf_arg         = ( "exponent" | "fresh" ) , "=" , NUMBER ;
rate_expr        = rate_const | wave_expr | burst_expr | timeline_expr ;
rate_const       = NUMBER , "/" , ( "s" | "m" | "h" ) ;
wave_expr        = "wave(" , "base=" , rate_const , "," , "amp=" , rate_const , "," ,
                   "period=" , DURATION , [ "," , "shape=" , shape_kw ] , ")" ;
burst_expr       = "burst(" , "base=" , rate_const , "," , "peak=" , rate_const , "," ,
                   "every=" , DURATION , "," , "hold=" , DURATION , ")" ;
timeline_expr    = "timeline" , "{" , { DURATION , ".." , DURATION , "=" , rate_const } , "}" ;
shape_kw         = "sine" | "triangle" | "square" ;
// ⚠ `wave` / `burst` / `timeline` 三个随时间变化的形态**语法已定、语义未实现**：
// 生成器当前只取 `base=`（`timeline` 取第一段）当常量速率，故校验期直接以 VN28 拦下
// （§7.3 P2 落地后放开）。

inject_block     = "inject" , "{" , { inject_case } , "}" ;
replay_stmt      = "replay" , IDENT , "{" , "use" , "from" , STRING , "}" ;
inject_case      = mode_kw , "<" , [ IDENT , ":" ] , INTEGER , ">" ,
                   "for" , IDENT , IDENT , "{" , inject_body , "}" ;
mode_kw          = "hit" | "near_miss" | "miss" ;
inject_body      = { step } , [ "spread" , DURATION ] ;
step             = event_group | without_step | join_block ;
event_group      = [ "then" ] , "use" , value_source , "x" , INTEGER ;
join_block       = "join" , IDENT , "as" , IDENT , "{" , { event_group } , "}" ;
without_step     = [ "then" ] , "without" , "(" , predicate_list , ")" ,
                   [ "within" , DURATION ] ;
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
- 合法的注解键只有两个：`duration`（场景总时长，默认 `60s`）与 `seed`（写在
  `scenario name<seed=N>`，默认 `0`）。其余键——含文档早期提过的 `tick` / `rows` / `emit`
  ——与不合法的值类型都由 **VN29** 报出：注解列表是泛化解析的，此前它们被**静默忽略**，
  写错键名（`#[duratoin=10m]`）或值类型（`#[duration=10]`）会悄悄退回默认值（60s / seed 0）。
- `for RULE` **必填**，不再从期望反推。
- `x N` 是"每个实体在该步骤上的条数"，取代旧的 `with(N)`。
- 实体键可省：`hit<500>` 从规则推断；`hit<sip: 500>` 用于多 key / 消歧（见 §3.7）。
- `spread D` 可选，必须 ≤ `#[duration]`（VN25）。
- `use` 的值来源三种：`use(preds)`、`use({json})`、`use from "file"`。
- `without(preds) [within D]` 是**构造约束**（不是步骤）：声明“该实体的窗口内不得出现
  匹配 `preds` 的事件”。**不写条数**、不注入事件、不占步骤位（不参与 VN24）。
  `within` 省略时取目标规则 `match` 的窗口长度。见 §3.8。
- `replay <window> { use from "file" }` 是**照单发货**通道（§3.9）：不写条数、不做实体
  数学、不参与实体断言；可写多条，与 `inject` 并存。
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

把一份现成的数据原样灌进去（不做实体数学、不参与断言）：

```wfg
#[duration=10m]
scenario replay_only<seed=1> {
  background { stream conn_events gen 50/s }

  replay conn_events { use from "raw/monday.ndjson" }   // 文件有多少条就发多少条
}
```

带否定步骤的规则（`not has …`）要造“该触发”的数据，用 `without(...)` 声明窗口内不得
出现什么（§3.8）：

```wfg
#[duration=10m]
scenario no_login_then_xfer<seed=7> {
  background { stream conn_events gen 50/s }

  inject {
    // 20 个 IP × (scan + xfer)；声明这些 IP 的窗口内不得出现成功登录
    hit<sip: 20> for scan_then_xfer conn_events {
      use(action="scan") x 3
      then use(action="xfer") x 1
      without(action="login_ok") within 5m
    }
  }
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
- **`on each` 规则可注入**：该规则没有窗口与阈值——命中的**一条**事件即产出告警，故断言的
  步骤阈值取 1；`hit` = 注入事件满足 each 过滤条件，`near_miss` / `miss` = 一个都不许命中
  （两者在 `on each` 上**同义**，设计上没有"接近但未达阈值"可言）。实体字段可省：从
  `entity(...)` 的单一字段推断（§3.7 第三行）。

### 3.3 值与值模板（P2）

- `use(preds)` / `use({json})` / `use from "f.json"` 提供该步骤的字段值。
- 一条**记录** = 一组字段值；默认该步骤的 N 条事件都用同一份值。
- `use({...})` 的顶层键展开为字段；`_` 前缀的键（`_stream` / `_window` / `_timestamp`）**忽略**，
  方便把原始日志整份粘进来。
- `use from "file"` 在**加载期**（`loader::resolve_inject_files`）就地解析成与 `use({...})` 完全
  同构的内联 JSON，因此字段级校验（VN9/VN11/VN12）对文件内容同样生效：
  - 顶层 **object** → 一条记录；
  - 顶层 **object 数组** / **NDJSON**（每行一个 object，空行与 `//` 注释忽略）→ 多条记录，
    按事件序号**循环取用**（`N > 记录数` 时回绕）；
  - 路径相对 `.wfg` 所在目录（绝对路径原样用）；文件缺失、非 JSON、顶层不是 object /
    object 数组、数组为空都在加载期明确报错——不做"静默产出空字段"的兜底。
- 同一 `use` 内重复字段报 VN9（逐记录检查，不同记录重复同一字段是正常的）；与实体键重复报
  VN12；字段不在 schema 报 VN11。

### 3.4 背景与注入分离（P5）

`background` 只决定"除定向构造外的随机流量"。注入条数完全由用例头与 `x N` 决定，因此：

- 改背景速率**不会**改变注入条数；
- 注入不占背景的配额：`总条数 = 背景(rate × duration) + 注入(实体数 × ΣN)`——注入量大于
  背景配额时背景也不会被清零；
- 写错数量不再静默产出 0 条（由断言/VN21 暴露）。

### 3.5 时间铺开（P6）

- `spread D` 显式给出铺开窗口，覆盖默认的规则窗口长度；必须 ≤ `#[duration]`（VN25）。
- 实体的簇起点在 `[0, 跨度]` 上**等距**铺开（`uniform_cluster_start`，跨度 = `duration − 窗口`）：
  首簇贴 `0`、末簇贴 `跨度`（末簇窗口刚好收在场景末尾），整段 `duration` 被均匀覆盖、两端
  不留空档；只有一个簇时取中点。
  “末簇顶到场景末尾”本身不再有风险：引擎的收尾水位是 `final_wm` = **数据末尾**，oracle 已按
  同一口径扫收尾（§7.2 那条已落地，回归见 `crates/wfgen/tests/e2e_datagen.rs`）。
  窗口不短于 `duration` 时无法错开，退回起点 `0`（簇必然重叠，保持旧行为）。
- 簇内事件仍按步骤顺序在窗口内均匀落下（`per_step_window × i / N`）；`miss` 本来就按
  `duration × 事件序号 / 总条数` 均匀落下，不受影响。

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
| `on each s` + `entity(<type>, s.event_id)` | `event_id`（注入侧同口径推断，见 §3.2） |

- 显式给出时才用 `hit<sip: 500>`；显式值应与推断结果一致。
- 显式与推断不一致 → `VN23`；字段不在该 stream 的 schema → `VN22`（都是**校验期**错误，
  §4.1）。注意多 key 规则的实体是 key 元组，显式写字段属于**消歧**用法，不算不一致。

### 3.8 `without(...)`：否定步骤的构造约束

**问题**。带否定步骤的规则无法只靠注入正向事件来可靠触发：

```wfl
match<sip : 5m> { on event seq { has scan; not has login; has xfer; } }
```

`not has login` 由 `wf-cep` 的 `SeqRuntime` 在**该实体的窗口实例**上扫描（`scan_negations`
只看传到该实例的事件），所以只要窗口内出现一条**属于该实体、且命中 `login` 条件**的事件，
规则就不会触发。这类事件可能来自两处：

1. 背景噪声恰好撞上该实体的键值（随机 IP / 随机数字）；
2. **注入事件自己**命中否定条件（例如 `xfer` 步骤的某个随机字段恰好落在 `login` 的过滤条件里）。

**声明**。`without(preds) [within D]` 把“该实体窗口内不得出现匹配 `preds` 的事件”写成
构造约束（`preds` 与 `use(...)` 同形式）：

```wfg
hit<sip: 20> for scan_then_xfer conn_events {
  use(action="scan") x 3
  then use(action="xfer") x 1
  without(action="login_ok") within 5m
}
```

**口径**。

| 项 | 口径 |
|---|---|
| 条数 | **不写**。否定步骤没有“N 条”语义 |
| 判定窗 | `within D`；省略 = 目标规则 `match` 的窗口长度 |
| 窗口起点 | 该实体**首条注入事件**的时间 |
| 作用范围 | 该实体（`field = value`）在该 stream 上的窗口 |
| 与规则对位 | **不要求**对位。规则有没有 `not` 步骤都可以写（纯构造约束） |
| 位置 | 与 `use` 步骤**解耦**（写在前后无语义）；`[then]` 可省 |

**严格性**。窗口内“无匹配事件”对**注入 + 背景噪声**都要成立：

- 注入事件命中 `preds` → **报生成期错误**（排不掉：要改就得改 `use(...)` 的值）；
- 背景事件命中 `preds` → 直接剔除（`datagen::suppress_without_guards`）；
- 不命中 `preds` 的背景噪声**不剔**：它不会违反否定步骤，剔除只会无故偏离背景配额。

实体键与 `preds` 的交集为空是**前提**：VN12 禁止 `without(...)` 写实体字段，否则“该实体
窗口内不得出现该实体自己的事件”自相矛盾。`without(...)` 的谓词同样过 VN9 / VN11 / VN12 与
VN25（`within` ≤ `#[duration]`）。

**与旧 `not(...) within(...)` 的区别**：旧写法与 `use` 混在同一个 step 列表里、带条数，
容易被读成“否定上一步”；新写法是独立的构造约束声明。旧写法在**解析期**报 VN20（§5.1）。

**实现落点**。`inject_gen::WithoutGuard`（清单）+ `datagen::suppress_without_guards`
（背景剔除），两侧共用同一个 `WithoutGuard::is_violation` 判定（stream + 实体键 +
窗口闭区间 + 命中谓词）。实体标识定位不到（复合 `entity(...)`）时直接报生成期错误——
`without` 的保证以实体为单位，定位不到实体就无法把保证落到窗口上。

### 3.9 `replay`：照单发货

```wfg
replay conn_events { use from "raw/monday.ndjson" }
```

把一份现成数据原样灌进去，取代 `wfgen send --input` 的多数用法。与 `inject` 的三点不同：
**不写条数**（文件有多少条就发多少条）、**不做实体数学**、**不参与实体断言**
（`hit` / `near_miss` / `miss` 那套与它无关）。可写多条，与 `inject` / `background` 并存。

| 项 | 口径 |
|---|---|
| 值来源 | 只有文件（`use from`）；写成内联值或 `x N` 报 VN20 |
| 记录形态 | object / object 数组 / NDJSON（与 `use from` 同构，脚本由 loader 解析） |
| 目标 stream | 必须是已加载 schema 里的窗口（VN3）；**不要求**写在 `background` 里（回放流可以不要背景噪声） |
| 事件字段 | 记录顶层键照抄（`_stream` 等 `_` 前缀内部键剔除）；时间列写 schema 的 `time_field` |
| 时间来源 | `_timestamp` 优先，其次 schema 的 `time_field`；按位宽归一化（秒 / 毫秒 / 微秒 / 纳秒） |
| 时间对齐 | 以文件**最早一条**为锚平移到场景起点（同文件内相对间隔保持），使 `#[duration]` 成为三类事件共同的时间窗 |
| 无时间字段 | 按序号在 `duration` 内均匀落下（与 `miss` 同策略） |
| 断言 | 不对任何实体承诺"必须 / 不得报警"，但它的事件**必须**进 oracle 的输入流（否则期望文件与引擎不一致） |
| `without(...)` | 对 replay 事件同样生效：命中 guard 谓词 → 生成期报错（replay 数据不能自动剔除） |
| `spread` | 与它无关（`spread` 是 inject 簇的铺开旋钮） |

校验：文件缺失 / 非 JSON / 形态非法在加载期报错；为空或时间字段口径不齐报 **VN26**；平移后
超出 `#[duration]` 报 **VN25**（不截断，不静默丢数据）。决策过程见 §8。

## 4. 校验与错误码

### 4.1 校验期（`wfgen lint` / `gen` 加载阶段）

码前缀按**校验域**分族（族内编号递增，删除的旧号不复用）：

| 族 | 管什么 |
|---|---|
| `VN` | `.wfg` 的语法与注入语义（本节的表） |
| `INJ` | **生成期**断言（§4.2，不属校验期） |

旧 `SC` / `SV` 两族**已退役**：它们校验的是 legacy 输入路径，而 `wfg_parser` 只实现
stream-first 语法、`WfgFile::syntax` 恒为 `Some`，那条路径已不存在（校验对象改由 `VN` 族覆盖：
stream / 规则绑定 → `VN3` / `VN10` / `VN14`；字段与 schema → `VN11` / `VN22` / `VN23`；
字段级 generator override 随 `stream ALIAS : WINDOW RATE { FIELD = gen_expr }` 语法一起删除）。

下表为 `VN` 族：

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
| VN17 | `use({...})` 顶层不是 object / object 数组（或记录不是 object、数组为空） | `… use({...}) 的顶层必须是 JSON object 或 object 数组` |
| VN20 | 使用旧语法 `hit<N%>` / `with(N)` | 见 §5.1（**解析期**报错） |
| VN21 | 实体个数为 0、`x 0`、或没有任何事件组 | `… 实体个数必须大于 0 / 第 k 个事件组 x 0 / 至少需要一个 use … x N 事件组` |
| VN22 | 显式实体字段不在该 stream 的 schema | `… 实体字段 '<f>' 不在 stream '<s>' 的 schema '<w>' 里` |
| VN23 | 显式实体字段与规则推断不一致（单 key `match` = 该 key；`on each` = `entity(...)` 的单字段） | `… 显式实体字段 '<f>' 与规则 '<r>' 推断的实体字段 '<g>' 不一致（去掉显式字段即用推断值；多 key 规则才需要显式消歧）` |
| VN24 | `use` 事件组数 > 规则的事件步骤数（每个 `use ... x N` 对应一个步骤） | `… use 事件组数 2 超过规则 '<r>' 的事件步骤数 1（每个 `use ... x N` 对应一个步骤）` |
| VN25 | `spread` / `without ... within` / `replay` 文件跨度 超过 `#[duration]` | `spread 20m` / `第 1 个 without 的 within 20m` / `replay 文件 \`x.ndjson\` 的时间跨度 300 超过场景 duration 60s` |
| VN26 | `replay` 文件为空，或文件里的时间字段口径不齐（部分记录有 / 没有） | `replay 文件 \`raw.ndjson\` 为空` / `… 时间字段 '_timestamp' 只在 1 / 3 条记录上出现（或不是合法时间戳）` |
| VN27 | 场景的实体 id 总数 ≥ 2^24（用例之间靠分段保证实体值不重叠，而实体值按 24 位地址映射） | `injection 实体 id 总数 16777216 达到上限 16777216（…超出后用例之间的实体会重叠：hit 与 near_miss 会指向同一实体）` |
| VN29 | 场景注解键不在白名单（`#[...]` 只认 `duration`，`<...>` 只认 `seed`），或值类型不合法 | `注解键 'tick' 不支持：\`#[...]\` 只认 'duration'（tick / rows / emit 从未实现）` / `注解 'duration' 的值必须是时长字面量（如 \`10m\`），实际是数字` |
| VN30 | `join <window> as <key>` 匹配不到规则的 join 子句（目标窗 / 右侧连接键 / 形态）或 `within` 区间不含左事件时间 | `… 的 \`join auction_events as wrong_key\` 匹配不到规则 'r' 的 join 子句（…）` / `… 的 \`join x\` 指向的规则 join 是 snapshot/asof/anti 形态，暂不支持（v1 只支持缺省 inner）` |
| VN31 | `entity <window>.<field> zipf(...)`：目标窗 / 字段不存在或类型不可承载、参数越界、重复声明、与注入实体的**值域预算**超出 24 位空间 | `entity 分布的字段 'ok' 类型不支持（只支持 ip / digit / float / chars / hex）` / `entity 'src_ip' 的值域超出 24 位地址空间：注入实体 100 + 池 8388609 + 新值带 8388609 > 16777216（…）` |
| VN28 | 背景速率用了未实现的随时间形态 `wave(...)` / `burst(...)` / `timeline { ... }`（会按 `base=` 常量生成，与写法不符） | `stream 'auth_events': \`gen burst(...)\` 的随时间变化尚未实现（当前会按 \`base=\` 的常量速率生成，与写法不符）；请先改用常量速率 \`gen 100/s\`` |

`without(...)` 的谓词与 `use(...)` 共用同一套字段检查：重名 VN9、不在 schema VN11、
重复实体键 VN12（VN12 在这条路径上尤其重要——见 §3.8）。

VN27 的计数口径与生成侧一致：`hit` / `near_miss` 每个实体占一个 id；`miss` 是「每个事件
一个独立键」，故每个用例占 `实体个数 × ΣN`（这是 `miss` 能不成簇的前提，§4.2）。

> 旧号（占比域、`expect` 规则存在性、`seq`/`not(...)` 步骤相关、`SC2/SC2a/SC3/SC4`、
> `SV2/SV3/SV4/SV6/SV7/SV8`）随旧语法一起删除，不复用。

### 4.2 生成期断言（已落地）

| 码 | 触发 | 消息形态 |
|---|---|---|
| INJ1 | `hit` 实体未产出告警 | `hit 用例第 %d 个实体（%s=%s）不会触发规则 <r>：条数/阈值 <bind> %d/%d（…）` |
| INJ2 | `near_miss`/`miss` 实体产出了告警 | `%s 用例第 %d 个实体（%s=%s）会触发规则 <r>：命中 %s 路径（emit_time=%s）` |

把"最后由 `wfgen verify` 红一行百分比"提前到**生成期 + 精确定位到实体**。

`without(...)` 的“排不掉”同样在生成期报（`WfgenReason::Generation`，不进 INJ 表）：
注入事件命中 `without` 谓词、或实体标识定位不到（复合 `entity(...)`）时报错并给出实体、
谓词与时间戳（§3.8）。

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
`use(...) with(N)`、`not(...) within(...)`、`expect { … }`、`traffic` 关键字（→ `background`）、
`injection` 关键字（→ `inject`）、`oracle { … }`。

本条（与其他 VN20 一样）在**解析期**被单独接住并给出改写方向：

| 旧写法 | 改写方向 |
|---|---|
| `hit<20%>` | 显式实体个数 + 每实体条数（见上方文案） |
| `<field> seq { … }` | 实体字段写在用例头（`hit<sip: 500>`），步骤直接列在体内 |
| `use(...) with(N)` | `use(...) x N` |
| `not(...) within(...)` | `without(...) [within D]`（**不写条数**）——它声明的是构造约束：该实体窗口内不得出现匹配的事件。要造“违反”的样本，把那条事件当普通 `use(...) x N` 步骤注入 |

`traffic` / `injection` 在**解析期**各自被单独接住并给出改写方向（各一条静态文案），
避免用户只看到笼统的"期望某个块"。

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
| `nginx_log_stats/nginx_access_quick` | 100/s × 2m = 12000 | hit | 10 | 4 | 300 | 1200 | `hit<300>` + `x 4` |
| | | miss | 90 | 1 | 10800 | 10800 | `miss<10800>` + `x 1` |
| `nginx_log_stats/live/nginx_access_live` | 100/s × 2h = 720000 | hit | 10 | 4 | 18000 | 72000 | `hit<18000>` + `x 4` |
| | | miss | 90 | 1 | 648000 | 648000 | `miss<648000>` + `x 1` |

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
- 背景速率**不折**：外部语料迁移统一按“保留原 `gen` 速率”处理（新总条数 = 背景配额 + 注入，
  故 ≈ 旧总量的两倍，如 `nginx_access_quick` 12000 → 24000）。需要贴回旧总量时再按上面的
  `rate_new` 折一次（对 nginx 两个用例会折成 `gen 0/s`）。
- 迁移脚本曾把单行 `traffic { stream … }` 压成空的 `background {`（块未闭合，加载即报
  `Expected('stream' in background block)`）：`wf-rules/…/ssh_brute_quick.wfg` 与
  `wf-examples/core/meta_disable/…/ssh_brute_quick.wfg` 已修复。迁移后**必须**用 `wfgen lint`
  + `wfgen gen` 逐文件过一遍（带 INJ 断言），不能只比对新旧关键字。

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
- 分段上限由校验期 VN27 拦下：实体值按 24 位地址映射（Ip 写 `10.a.b.c`），一个场景的
  实体 id 总数（`hit` / `near_miss` = 实体个数；`miss` = 实体个数 × ΣN）必须 < 2^24；
  超出后段会重叠、不同用例拿到同一个值。生成侧 `generate_key_values` 的 Ip 映射与该上限
  共用同一个常量（`ENTITY_ID_SPACE`）+ 一条 debug 断言，两处口径不会漂移。
- 校验期实体字段检查 VN22 / VN23：显式实体字段必须在该 stream 的 schema 里，且必须与
  规则推断的实体字段一致（单 key `match` = 该 key；`on each` = `entity(...)` 的单字段；
  多 key 规则的显式字段按消歧用法放行）。缺了它，注入会静默指向错实体——生成器对拿不到
  类型的字段只会用字符串兜底。
- 校验期 `use` 组数检查 VN24：`use` 事件组数不得超过规则的事件步骤数，口径与编译产物一致
  （`on event seq` 链只数非 `neg` 步骤、`on each` = 1、stats = 0；由一条「以编译产物为
  oracle」的边界测试锁定）。数错组数会静默少注入某个步骤的事件；生成期 `plan_use_steps`
  仍保留同一检查（纵深防御）。
- `without(...)` 构造约束（§3.8）：`without(preds) [within D]` 解析进 `InjectCase::withouts`
  （与 `groups` 解耦，不参与 VN24），谓词过 VN9/VN11/VN12、`within` 过 VN25；生成期展开成
  `WithoutGuard` 清单——注入事件命中谓词即报错，背景噪声命中谓词则剔除。
- 旧 `not(...) within(...)` / `<field> seq { … }` / `use(...) with(N)` 在解析期各报一条
  带改写方向的 VN20 文案（§5.1）。
- `replay <window> { use from "file" }` 照单发货（§3.9 / §8）：文件记录照抄成事件（`_` 前缀
  内部键剔除、`_timestamp` 作为时间来源），时间以文件最早一条为锚**平移**到场景起点，
  使 `#[duration]` 成为三类事件共同的时间窗；文件没有时间字段时按序号在 `duration` 内均匀
  落下。文件缺失 / 为空 / 时间字段口径不齐 / 跨度超 `#[duration]` 都在加载与校验期报错；
  `without(...)` 的 guard 对 replay 事件同样生效（命中即报生成期错误——replay 的数据不能
  自动剔除）。
- 背景与注入完全分离：背景保留自己的配额（`rate × duration`），注入在其上叠加
  （旧口径 `背景 = 配额 − 注入` 及其 `inject_counts` 链路已删除）。
- 注入时间在场景 `duration` 内**等距铺开**：簇起点由 `uniform_cluster_start` 算出
  （`[0, duration − 窗口]` 上等距：首簇贴 `0`、末簇贴 `跨度`，覆盖整段），
  取代旧的“每簇随机起点”；窗口不短于 `duration` 时退回起点 `0`。
- `use from "file"` 的值文件解析（`loader::resolve_inject_files`）：相对 `.wfg` 目录解析路径，
  支持顶层 object / object 数组 / NDJSON，数组与 NDJSON 按事件序号循环取用；`gen` / `lint` /
  `bench` / `send` / `stream` 都经 `loader::load_from_uses` 走同一条解析。
- `on each` 规则作为注入目标：别名取自 `each_plan.alias`，注入步骤由该绑定合成（阈值 1、
  过滤条件取 bind filter + each filter 的等值约束），实体字段可从 `entity(...)` 推断。
- 结构化列与引擎契约对齐：wfgen 写 Arrow 时对 `object` / `array` / `array/<base>` 字段统一用
  **JSON 文本的 Utf8 列 + `wf.wfl.field_type` metadata**（常量取自 `wf-engine`，不复制字符串）。
  引擎只在带该 metadata 时把列值解析成 `Value::Object` / `Value::Array`，否则一律 `Value::Str`
  ——缺了它，读嵌套字段的规则会**静默不产出**，且与直读 `GenEvent` 的 oracle 不一致。
  有 schema 时按 schema 打标；`.arrow` 文件输出（无 schema）按列内实际值推断（整列同形才打标，
  混形保持当字符串）。
  同一条链的 oracle 侧同步收口：`GenEvent` → 引擎 `Value` 的转换**递归保留** object / array
  （旧实现 `_ => None` 把结构化字段整个丢掉，读嵌套字段的规则在 oracle 侧恒不命中）。
- 14 个仓内语料在断言下**全部通过**；其中 2 个按断言口径调整过（§5.3）。
- 外部语料迁移（P3）：`wf-rules`（4）· `wf-examples`（11）· `wf-conf-example`（1）共 **16 个**
  tracked `.wfg` 已迁到新语法并逐文件验证——`wfgen lint` 16/16 OK、`wfgen gen` 带 INJ1/INJ2
  断言 16/16 通过（其中 `performance/rule_scale_test` 是纯背景场景，验证覆盖“加载 + 生成”）。
  包含本仓外修复的 2 个迁移时写坏的文件（§5.3）。

### 7.2 未落地 / 未决

| 项 | 状态 |
|---|---|
| 外部语料迁移：`wf-rules` / `wf-examples` / `wf-conf-example` | **已落地**（§7.1；16/16 `lint` + `gen` 断言通过） |
| 文档：CHANGELOG | **已落地**（v0.7.0 随 release 提交写入 `CHANGELOG.md` / `CHANGELOG.en.md`，中英双语） |
| 尾部实例的 `close:flush` / `close:timeout` 时间口径 | **已落地**（oracle 收尾水位改用数据末尾 `final_wm`，与引擎 `close:flush` 对齐；回归见 `crates/wfgen/tests/e2e_datagen.rs`，`hop_oracle_closes_every_covered_window` / `batch_sweep_uses_data_end_not_scenario_end` 锁定口径） |
| 注解键白名单（VN29）+ `oracle { … }` 解析残留清理 | **已落地**（`tick` / `rows` / `emit` 与未知键、错值类型现由 VN29 拒绝；`OracleBlock` / `ParamAssign` / `ParamValue` / `extract_oracle_tolerances` / `validate/oracle.rs` 及 `ScenarioDecl.oracle` 已删除，容差固定 1s / 0.01） |

未决（不阻塞实现）：

- 生成期断言在**分片 / 多实例**下的口径（当前 oracle 是单机内存模型）。

### 7.3 扩展规划

**P1（优先）**

- 实体分布扩展：热点（Zipf）与新老实体比例 —— **已落地**（§10）。

**P2（增强真实性）**

- 速率模型扩展：`spike`、`jitter`、`diurnal`（昼夜曲线）。**现状：语法侧已有 `wave` / `burst` /
  `timeline`（§2 grammar），但生成器只取 `base=` 当常量速率**——降级时静默塌成平坦流量。
  已先用 **VN28** 把这类写法拦下（`lint` / `gen` 报错并给出改写方向），实现后再放开；
  另外 `spike` ≈ 已有的 `burst`、`diurnal` ≈ `wave(period=24h)`，真正需要新增的是 `jitter`。
- 跨流注入：同一实体在多 stream 的联动序列。
- 场景矩阵：同一场景的多参数批量运行。

**P3（工程效率）**

- 模板化：`template/param` 复用场景片段。
- 基线对比：与历史结果自动比对回归漂移。
- 报告输出：自动生成 markdown/html 对比报告。

## 8. `replay` 的时间对齐：方案与决定（**已落地**）

`replay STREAM { use from "f" }` 是 §7.2 里剩下的大块：一条"照单发货"的通道（文件有多少条发
多少条、不做实体数学、不参与实体断言），取代 `wfgen send --input` 的多数用法。它与
`inject` / `background` **可并存**——问题就出在这里：三类事件必须落在同一条时间轴上。

### 8.1 事实（代码依据，不是推测）

| 事实 | 依据 |
|---|---|
| 输出是**单条时间序**流 | `datagen::merge_sorted_chunks` 按时间戳归并所有 chunk |
| oracle 的收口水位固定在 `场景起点 + #[duration]` | `oracle/mod.rs`：`eos_time = scenario_start + duration`，随后推进水位并（batch 时）`close_all` |
| `--send` 只发事件，引擎**纯事件时间**驱动 watermark | `cmd_gen` 的发送路径只送 `_stream` / `_window` / 字段，不传场景起止 |
| `inject` / `background` 的时间**全部由 `#[duration]` 决定** | 簇起点 `uniform_cluster_start`（§3.5）、背景 `rate × duration` |

推论：**文件里的时间戳一旦落在 `[场景起点, 场景起点 + duration]` 之外，oracle 与引擎的口径
就对不上**——oracle 会在文件时间戳之后把水位退回 EOS，引擎侧的窗口收口则是跟着数据走的。

### 8.2 方案对比

| 方案 | 做法 | 优点 | 问题 |
|---|---|---|---|
| **A 原样照发** | 直接并入文件时间戳，`#[duration]` 只管 background / inject | 最"回放"，不改文件语义 | 单条时间轴被打破：超出窗口的事件会让 oracle 水位与引擎错位，期望文件与实际输出不一致 |
| **B 重新基准（推荐）** | 取文件内最早时间戳为锚，整体平移使锚点落在场景起点；`#[duration]` 成为三类事件**共同**的时间窗 | 一条时间轴、水位唯一、oracle 的 EOS 口径不变、batch 与 `--send` 行为一致 | 文件里的"真实时刻"被改写（回放语义有损），需在文档里写明 |
| **C 场景时长让给 replay** | 场景时长取 `max(#[duration], replay 跨度)`，或给 `replay` 自带 `within D` | 保留文件时间戳的相对关系 | 多 replay 块 + inject + background 需要一个统一的"时间轴合并"规则，复杂度高、校验码也要新增 |
| **D replay 不进 oracle** | 断言只看 background + inject，replay 只写文件 | 断言口径最干净 | 引擎看到的是**合并后的流**，replay 事件照样可能触发规则 → 期望文件与引擎必然不一致。除非能保证 replay 事件不触发任何规则（做不到） |

### 8.3 已定口径

**① 时间基准 = 方案 B（重新基准）；② 锚点在场景起点；③ 无时间字段则按序号均匀落下；
④ 允许与 `inject` 指同一 stream**（均已拍板）。据此写死五条：

1. **单条时间轴**：`#[duration]` 是 background / inject / replay 三类事件共同的时间窗；
   `replay` 事件以文件内最早时间戳为锚平移进该窗（同文件内相对间隔保持），锚点即**场景起点**。
2. **事件时间优先**：文件记录里若有时间字段（`_timestamp` 或 schema 的 `time_field`），
   **用字段值**算锚点与间隔；同一个文件里“部分记录有时间字段” ⇒ **报错**（口径必须唯一）；
   全都没有时间字段时，按序号在 `duration` 内均匀落下（与 `miss` 同策略，见 §3.5）。
3. **断言豁免、但参与流**：`replay` 不对任何实体承诺"必须 / 不得报警"，但它的事件**必须**
   进 oracle 的输入流——否则期望文件与引擎不一致（这正是 D 的问题）。
4. **与 `inject` 可指同一 stream**：叠加是预期语义；叠加后若 replay 数据破坏了 hit /
   near_miss 的口径，INJ1/INJ2 会如实报出——不静默。
5. **文件跨度不得超过 `#[duration]`**：平移后落在窗内是硬前提（否则违反第 1 条），超出
   直接报错，不截断（不静默丢数据）；多 replay 块各自独立平移、不额外错开。

另外三条附注：

- `replay` 不受 `spread` 影响（`spread` 是 inject 簇的铺开旋钮）。
- VN26（文件缺失 / 为空即报错）随 `replay` 一起落地，口径沿用旧设计。
- **`without(...)` 对 replay 是“排不掉”的来源**：guard 现在只剔背景噪声、对注入冲突报错；
  replay 事件既不能默默删（那是用户给的数据）、也不能默默无视（否则窗口保证失效）。
  故 replay 事件命中某个 guard 的谓词 → **生成期报错**，与注入侧冲突同口径。

### 8.4 决定记录

| # | 问题 | 结论 |
|---|---|---|
| ① | 时间基准 | **B（重新基准）** |
| ② | `replay` 锚点 | **场景起点**（除平移外不改写文件；多 replay 块各自锚到起点） |
| ③ | 无时间字段怎么落时间 | **按序号在 `duration` 内均匀落下**；同文件内部分有时间字段 ⇒ 报错 |
| ④ | 是否允许与 `inject` 同 stream | **允许**（叠加语义；断言如实反映） |

落地清单（**全部完成**）：语法与 AST（`replay <window> { use from … }`，可写多条）、
loader 解析（`--no-wfl` 也解析，因为 replay 不依赖规则）、生成路径（平移 / 无时间字段时均匀
落下 / 写 `time_field` 时间列）、VN26 与跨度检查（VN25）、`without` × replay 冲突检查、文档。

## 9. 跨流注入：`join <window> as <key>`（**已落地**）

§7.3 P2 的「跨流注入」：让 `.wfg` 能造出**跨流配对**的数据。驱动场景是 join 家族
（nexmark q8/q9 这类 `join … within … on … emit at …` 规则）——此前它们完全不走 `.wfg`：
规则由 daemon 配置加载、数据由独立生成器 `gen-nexmark` 产出，`.wfg` 只能给背景流量。

### 9.1 事实（代码依据）

| 事实 | 依据 |
|---|---|
| 一个用例只覆盖**一个窗口**：窗口与本用例 `stream` 不同的 bind 直接不参与 | `inject_gen/dispatch.rs` `build_alias_map_for_syntax_case` 的 `if bind.window != stream_block.window { return; }` |
| 注入器**不读 join**：只遍历 `match_plan.event_steps` + `each_plan` | `inject_gen/extract.rs` `extract_rule_structure` |
| 管道已经是「每步一个窗口」的形状 | `StepInfo` 带 per-step `window_name` / `scenario_alias` |
| 配对所需信息在计划里齐备 | `JoinPlan { right_window, mode, conds, within, reduce, emit_at }`；`JoinCondPlan::right_field_name()` |
| oracle 侧 join 已实现 | `run_oracle_events_full` 带 schemas → 右窗 lookup（否则 `EmptyLookup`，join 恒 miss） |
| **`gen` 的 oracle 调用没带 schemas** | `cmd_gen.rs` 用 `run_oracle`（无 schemas 形态）→ 任何 join 规则的右窗恒空、oracle 一条告警都出不来，INJ1 必然失败。这就是 join 负载从来不经过 `.wfg` 的直接原因；本次一并修掉 |
| 区间求值器**拿不到** | 引擎的 `eval_interval_bound` 是 `pub(crate)` |

### 9.2 语法

```wfg
hit<id: 200> for q8_monitor_new_user person_events {
  use(name="n") x 1                 // 左（驱动）侧：仍是用例头的 stream
  join auction_events as seller {   // 目标窗 + 右行连接键字段
    use(price=7) x 1                // 每**条左事件**在目标窗造几条
  }
}
```

`join` 块**不占事件步骤位**（不参与 VN24 的组数口径），可写多个（对应规则里多个 join）。

### 9.3 语义（推导而非手写）

右事件由生成器推导三件事，用户不写键值也不写时间：

1. **连接键**：右行的连接键字段（`as <key>`）写成**左实体键值**——与规则 `on <left> ==
   <right>` 的右侧字段名对齐（VN30 校验能唯一匹配到该 join 子句）。
2. **时间**：由规则 join 的**形态**决定（§9.4 的表），用户不写时间。
3. **其余字段**：按目标窗 schema 随机生成，再由 `use(...)` 的谓词覆盖（复用
   `build_event_fields_with_predicates`，时间字段口径因此与左事件天然一致）。

断言口径不变：仍以**驱动侧实体**为单位（`hit` 必报 / `near_miss`·`miss` 必不报）；右事件
不产生独立实体。

### 9.4 支持的两种形态（右事件放在哪，由形态决定）

| 规则 join 形态 | 判据 | 右事件时间 | 理由 |
|---|---|---|---|
| **deferred** | inner + `within` + `emit at` | 与左事件**同刻** | 到期评估时右行必已在窗内（q4/q8/q9） |
| **snapshot** | `snapshot` 且**无** `within` | 左事件**提前 1ms** | snapshot 在驱动事件被处理时查右窗，同刻右行还没进去（q3/q20）；取 1ms 而非 1ns 是为了让**毫秒精度**的下游（JSONL 的 `_timestamp`）也看得出先后 |

其余形态都报 VN30：`asof` / `anti`、**没有 `emit at` 的即时 inner join**（它要求右行更早、而
`within` 下界常就是左事件时间，两者冲突会时好时坏）、`snapshot` + `within`（WFL 语法本身也不接受）。

其余边界：

- **`within` 区间不做静态校验**：引擎要求 deferred join 的上界是**绝对时间表达式**
  （`bucket_end(...)` / `a.expires`），注入器侧没有求值器（引擎的 `eval_interval_bound` 是
  `pub(crate)`），算不了。若用户把下界写成晚于左事件时间，右事件会落在区间外——后果是生成期
  INJ1 报「hit 实体不会触发」，属于**可见的失败**（不是静默产出）。
- `spread` 只管左簇；右事件跟随所属左事件的时间，不单独铺开。
- 目标窗必须已在 schema 里、`use` 的字段必须属于目标窗（VN11）、在 `use` 里重复连接键报 VN12。

### 9.5 join-then-key（nexmark q6 形态）

q6 的 `match<seller:10m>` 里，**键 `seller` 不在驱动事件（bid）上，而在 join 侧（auction）**：
引擎先 snapshot join（`b.auction == auction_events.id`）拿到 `seller`，再按该键分组。注意它的
**实体仍是驱动侧的 `b.auction`**（`entity(digit, b.auction)`）——只有分组键取自 join 侧。

判据与引擎一致（`JoinKeyPlan`）：单个 key 不在任何 bind 的窗口 schema 上、且恰有一个
`snapshot` join 的目标窗提供它。生成侧据此做三件事（**语法无需新增**）：

| 环节 | 做法 |
|---|---|
| 连接键 | 取规则 `on <left> == <right>` 的 **left（驱动侧）字段**值，写进驱动事件与右行的 `right` 字段——两侧因此指向同一实体（q6 的 `b.auction` ↔ `auction_events.id`） |
| join 侧键 | 驱动 schema 里没有的 match key（`seller`）**写到右行**上，按目标窗字段类型生成 |
| 实体推断 | VN23 在 join-then-key 时不再拿 match 键当实体（那会误报不一致），改取规则 `entity(...)` |

join 侧键的值落在**与背景噪声分开的值带**（`1 << 22` 起）：背景 digit 是 `0..100_000`、ip 是
随机 24 位——不分开的话，注入实例会和背景事件并到同一条窗口实例上，`avg >= 200` 之类的阈值
被背景稀释，断言随背景波动。**已知边界**：驱动实体值域超过 `1<<22`（> 420 万实体）时可能与
实体段重叠（VN27 只守 `< 2^24`），此时该边界靠 INJ1 的可见失败暴露。

### 9.6 落地清单

`wfg_ast.rs`（`JoinStmt` + `InjectCase.joins`）、`wfg_parser/syntax/inject.rs`（`join` 块 +
抽出共用的 `parse_use_group`）、`validate/syntax.rs`（VN30）、`extract.rs`
（`InjectOverrides.joins`）、`helpers/generate.rs`（`push_join_events`）、`hit.rs` /
`near_miss.rs` / `non_hit.rs`（在左事件之后补发右事件）、**`cmd_gen.rs`（oracle 调用改为带
schemas，否则 join 规则永远出不了期望）**。

`extract.rs` 额外登记规则侧 join 口径（`RuleJoinInfo`：目标窗 + 右侧连接键 + **驱动侧连接键** +
放置偏移），生成时按 `(目标窗, 连接键)` 与用例的 `join` 块配对后决定右事件时间与键的来源；
`generate_key_values` 的字段类型查找扩到所有窗口，并给 join 侧键用分离值带；VN23 的实体推断
在 join-then-key 时改取 `entity(...)`。

测试：解析 1、VN30 4、生成 + oracle 复核 6（deferred / snapshot / join-then-key 各 2；配对后
**真能触发规则**，配对错、时间放错或值带选错就掉到 0 条）；另有 CLI 端到端实测（q8、q20 与
q6 三种形态：`lint` OK、`gen` 断言全过、右事件键与时间符合 §9.4/§9.5、期望告警数正确）。

## 10. 实体分布：`entity <window>.<field> zipf(...)`（**已落地**）

§7.3 P1。背景事件默认**每个字段每条现随机**（`stream_gen.rs` 的字段循环直接
`generate_field_value`）——同一 stream 里没有任何值会重复，于是「热点实体」根本表达不出来：
想造"某 IP 被反复打"的现实流量，此前只能靠 `inject` 定向构造。

### 10.1 语法

```wfg
background {
  stream conn_events gen 100/s
  entity conn_events.sip zipf(pool=1000, exponent=1.1, fresh=0.2)
}
```

可写多条（每 `(窗口, 字段)` 至多一条）；`exponent` / `fresh` 可省（默认 `1.0` / `0.0`）。

### 10.2 语义

| 参数 | 含义 |
|---|---|
| `pool=N` | 实体值池大小：**N 个不同实体**（由索引直接导出，不消耗 RNG，跨运行确定） |
| `exponent=s` | Zipf 指数：rank `k` 权重 `1/(k+1)^s`；`s=0` 退化为均匀，越大越集中 |
| `fresh=r` | 比例 `r` 的事件取**池外新值**（真实流量里总有新实体） |

**值域与注入实体分区**（关键正确性点）：

| 占用方 | 值域（24 位地址空间，与注入共用 `entity_value_for_index` 映射） |
|---|---|
| 注入实体 | 底部 `[0, total_entity_ids)`（VN27 已守总长 < 2^24） |
| 池 | 顶部 `[2^24 − pool, 2^24)` |
| `fresh` 新值带 | 再往下的 `pool` 个：`[2^24 − 2·pool, 2^24 − pool)` |

VN31 校验 `total_entity_ids + 2·pool ≤ 2^24`。少了这一步，背景噪声会撞上注入实体，
直接污染 INJ1/INJ2 的口径（`hit` 实体窗口里混进不该有的匹配事件）。

### 10.3 边界

- 只支持 `ip` / `digit` / `float` / `chars` / `hex` 五类标量字段（布尔 / 时间 / 结构化字段
  做实体没有意义）——其它类型报 VN31，不静默忽略。
- 只作用于**背景**事件：`inject` / `replay` 的实体值仍由它们各自的机制决定。
- 同一 `(窗口, 字段)` 重复声明报 VN31（避免"哪条生效"的歧义）。

### 10.4 落地清单

`wfg_ast.rs`（`EntityDistStmt` + `BackgroundBlock.entities`）、`wfg_parser/syntax/background.rs`
（`entity … zipf(…)` 语句）、`validate/syntax.rs`（VN31 + 值域预算）、`stream_gen.rs`
（`EntityPool`：池 + 累计权重二分采样 + 新值带）、`datagen/mod.rs`（按窗口建池并传入）、
`inject_gen/helpers/generate.rs`（把 24 位索引 → 值 的映射抽成 `entity_value_for_index`，
注入与背景池共用）。

测试：解析 1（含缺 `pool` 报错）、VN31 8 例（窗口 / 字段 / 类型 / `pool=0` / `fresh` 越界 /
重复 / 值域预算 / 无 background stream）、生成 5 例（池限定值域且 8 个都出现、`exponent` 造出
热点、`exponent=0` 均匀、`fresh=1.0` 取不重叠新值带、未声明字段保持随机）。
