# `.wfg` 语法参考

`.wfg` 是 wfgen 的场景文件：描述**一段背景流量 + 若干定向构造的实体**，生成出来的数据自带
标签（`hit` 必须报警、`near_miss` / `miss` 必须不报警）。

本文只讲**语法**：能写什么、怎么组合、每个构造的硬约束与对应错误码，内容与实现
（`crates/wfgen/src/wfg_parser/`）逐条对齐。

| 想了解 | 看哪 |
|---|---|
| 语法（能写什么、约束、错误码） | **本文** |
| 怎么用、按任务找写法、常见坑 | [scenarios.md](./scenarios.md) |
| 为什么这样设计、语义推导、旧语法迁移折算 | [../design/wfg-design.md](../design/wfg-design.md) |

## 1. 词法

| 元素 | 写法 | 说明 |
|---|---|---|
| 注释 | `// …` 到行尾 | **只有行注释**。`#` 不是注释——`#[` 是场景注解 |
| 标识符 | `[A-Za-z_][A-Za-z0-9_]*` | 场景名 / stream / 窗口 / 字段 / 规则名 |
| 关键字 | `use` `scenario` `background` `stream` `gen` `entity` `zipf` `inject` `replay` `hit` `near_miss` `miss` `for` `then` `without` `within` `join` `as` `spread` `x` `from` | 按**词边界**匹配：`hitfoo` 是标识符，不会被当成 `hit` |
| 时长 | `500ms` `30s` `10m` `2h` `1d` | 单位 `ms` / `s` / `m` / `h` / `d`；`0` 可省略单位 |
| 速率 | `100/s` `600/m` `3600/h` | 速率单位**只有** `s` / `m` / `h`（没有 `/d`） |
| 数字 | `42`、`1.1` | 小数只出现在 `exponent` / `fresh` 这类参数上 |
| 字符串 | `"…"` | 双引号；路径、`use` 的值、注解值都用它 |
| 结构化值 | `{…}` / `[…]` | 内部是**纯 JSON**（不是 wfg 语法），可任意嵌套 |
| 分号 | `;` | 语句分隔**可选**，纯装饰 |

## 2. 文件结构

```wfg
use "schemas/auth.wfs"                                // 0..n 条：加载 schema / 规则
use "../rules/02-initial_access/ssh_brute_force.wfl"

#[duration=10m]                                       // 0..1 个场景注解块
scenario ssh_brute<seed=42> {                         // 必填：名字 + 0..n 个行内注解
  background {                                        // 必填：至少 1 个 stream（VN1）
    stream auth_events gen 50/s
    entity auth_events.sip zipf(pool=1000)            // 可选：实体分布，写在 background 里
  }
  inject { … }                                        // 可选：至多 1 个
  replay conn_events { use from "raw/monday.ndjson" } // 可选：可多条
}
```

- **`use` 的位置**：只能在 `scenario` **之前**。扩展名决定用途：`.wfs` → schema，
  `.wfl` → 规则；其它扩展名直接报错。
- **路径基准**：`use` 的路径、`use from` / `replay` 的文件路径，都相对 **`.wfg` 所在目录**
  解析（绝对路径按原样用）。
- **`background` 必填**，`inject` 至多一个，`replay` 可多条；三者在 `scenario` 体内
  **顺序无关**。
- 同名的 `background` / `inject` 只能有**一个**：写两次报 **VN32**（两个块不会合并，
  也不会静默丢掉前一个块）。`replay` 不受此限，可写多条。
- **注解白名单**（VN29）：`#[…]` 只认 `duration`（时长字面量，默认 `60s`）；
  `<…>` 只认 `seed`（非负数字，默认 `0`）。其余键——包括早期文档提过的 `tick` / `rows` /
  `emit`——一律报错，不会静默退回默认值。

## 3. EBNF

```ebnf
scenario_file    = { use_decl } , [ scenario_attrs ] , scenario_decl ;
use_decl         = "use" , STRING ;

scenario_attrs   = "#[" , attr_list , "]" ;
attr_list        = [ attr , { "," , attr } ] ;
attr             = IDENT , "=" , value ;

scenario_decl    = "scenario" , IDENT , [ "<" , attr_list , ">" ] , "{" ,
                     { background_block | inject_block | replay_stmt } , "}" ;
(* 约束：background_block 恰有 1 个（其中至少 1 个 stream，VN1）；
        inject_block 至多 1 个；两者重复书写报 VN32；
        replay_stmt 可多条；顺序无关。 *)

background_block = "background" , "{" , { stream_stmt | entity_stmt } , "}" ;
stream_stmt      = "stream" , IDENT , "gen" , rate_expr , [ ";" ] ;
rate_expr        = rate_const | wave_expr | burst_expr | timeline_expr ;
rate_const       = NUMBER , "/" , ( "s" | "m" | "h" ) ;
wave_expr        = "wave" , "(" , "base=" , rate_const , "," , "amp=" , rate_const , "," ,
                   "period=" , DURATION , [ "," , "shape=" , shape_kw ] , ")" ;
burst_expr       = "burst" , "(" , "base=" , rate_const , "," , "peak=" , rate_const , "," ,
                   "every=" , DURATION , "," , "hold=" , DURATION , ")" ;
timeline_expr    = "timeline" , "{" ,
                     { DURATION , ".." , DURATION , "=" , rate_const , [ ";" ] } , "}" ;
shape_kw         = "sine" | "triangle" | "square" ;
entity_stmt      = "entity" , IDENT , "." , IDENT , "zipf" , "(" , zipf_args , ")" , [ ";" ] ;
zipf_args        = zipf_arg , { [ "," ] , zipf_arg } ;
zipf_arg         = ( "pool" | "exponent" | "fresh" ) , "=" , NUMBER ;
(* 约束：`pool` 必填，未知参数报错。 *)

inject_block     = "inject" , "{" , { inject_case } , "}" ;
inject_case      = mode_kw , "<" , [ IDENT , ":" ] , INTEGER , ">" ,
                   "for" , IDENT , IDENT , "{" , { case_stmt } , "}" ;
mode_kw          = "hit" | "near_miss" | "miss" ;
case_stmt        = [ "then" ] , use_group        (* 事件组：占步骤位 *)
                 | [ "then" ] , without_stmt     (* 构造约束：不占步骤位 *)
                 | join_stmt                     (* 跨流注入：不占步骤位 *)
                 | spread_stmt ;
use_group        = "use" , value_source , "x" , INTEGER , [ ";" ] ;
value_source     = "(" , predicate_list , ")" | "(" , json_object , ")" | "from" , STRING ;
without_stmt     = "without" , "(" , predicate_list , ")" ,
                   [ "within" , DURATION ] , [ ";" ] ;
join_stmt        = "join" , IDENT , "as" , IDENT , "{" , { [ "then" ] , use_group } , "}" ;
spread_stmt      = "spread" , DURATION , [ ";" ] ;
predicate_list   = predicate , { "," , predicate } ;
predicate        = IDENT , "=" , value ;

replay_stmt      = "replay" , IDENT , "{" , "use" , "from" , STRING , [ ";" ] , "}" ;

value            = STRING | NUMBER | DURATION | "true" | "false" | "null" | json ;
```

两点与设计文档 §2.1 的写法不同（**以本节为准**，因为这里逐条对齐了实现）：

- `spread` / `without` / `join` 在用例体内**位置无关**，不是「只能写在末尾」；`spread` 写多次
  时最后一个生效。
- `without(preds)` 与 `join … { … }` 在实现里是独立语句种类，不是 `use` 的变体。

## 4. 逐个构造

### 4.1 `use "…"`

| 项 | 规则 |
|---|---|
| 作用 | 加载 `.wfs`（字段类型）与 `.wfl`（被验证的规则） |
| 位置 | `scenario` 之前，可多条 |
| 路径 | 相对 `.wfg` 所在目录 |
| 扩展名 | `.wfs` / `.wfl`；其它扩展名报错 |
| `--no-wfl` | 跳过 `.wfl`（规则不编译、注入的 `use from` 值文件也不读） |

### 4.2 场景注解与场景头

```wfg
#[duration=10m]
scenario ssh_brute<seed=42> { … }
```

| 注解 | 位置 | 值 | 默认 |
|---|---|---|---|
| `duration` | `#[duration=…]` | 时长字面量 | `60s` |
| `seed` | `scenario NAME<seed=…>` | 非负数字 | `0` |

`duration` 是场景的虚拟时长，也是背景条数（`rate × duration`）与 `spread` / `within` 的上界；
`seed` 决定随机流可复现。两者写错键名或值类型报 **VN29**（不再静默忽略）。

### 4.3 `background { … }`

```wfg
background {
  stream conn_events gen 100/s
  stream dns_events  gen 20/s
  entity conn_events.sip zipf(pool=1000, exponent=1.1, fresh=0.2)
}
```

**`stream S gen <速率>`**

- 速率常量形如 `100/s`：速率单位 `s` / `m` / `h`。速率 ≤ 0 报 **VN2**。
- `wave(...)` / `burst(...)` / `timeline { … }` **语法已定、语义未实现**（会按 `base=`
  的常量生成，与写法不符）→ 校验期直接报 **VN28**，请先写常量速率。
- stream 名必须能在已加载的 schema 里找到（**VN3**）。

**`entity <窗口>.<字段> zipf(…)`**

| 参数 | 必填 | 含义 |
|---|---|---|
| `pool=N` | 是 | 实体值池：N 个不同实体（由索引直接导出，不消耗 RNG，跨运行确定） |
| `exponent=s` | 否（默认 `1.0`） | Zipf 指数：rank `k` 权重 `1/(k+1)^s`；`0` 退化为均匀，越大越集中 |
| `fresh=r` | 否（默认 `0.0`） | 比例 `r` 的事件取池外新值 |

- 参数顺序无关，`pool` 必填，未知参数报错。
- 只作用于**背景**事件；`inject` / `replay` 的实体值不受影响。
- 只支持 `ip` / `digit` / `float` / `chars` / `hex` 五类标量字段。
- 同一 `(窗口, 字段)` 重复声明、参数越界、与注入实体的值域预算超 24 位 → **VN31**。

### 4.4 `inject { … }`

每个**注入用例**是一个 `mode<实体> for 规则 流 { … }`：

```wfg
inject {
  hit<sip: 500> for ssh_brute_force auth_events {
    use(result="failed", service="ssh") x 12
  }
}
```

**用例头：`mode<[字段:]实体数> for RULE STREAM`**

| 元素 | 说明 |
|---|---|
| `mode` | `hit` / `near_miss` / `miss` 三者之一 |
| `[字段:]` | 实体字段，可省（从规则推断）。多 key 规则或需要消歧时显式写 |
| `实体数` | 该用例的实体个数，必须 > 0（**VN21**） |
| `for RULE` | **必填**，规则名必须在已加载 `.wfl` 里（**VN14**） |
| `STREAM` | 用例作用的流，必须在 `background` 里声明过（**VN10**） |

实体字段的静态检查：不在该 stream 的 schema → **VN22**；与规则推断的实体字段不一致 →
**VN23**（多 key 规则的显式字段属消歧用法，放行）。

注意：生成器会**无条件把自己算出来的值写进事件**，覆盖 `use(...)` 给的同名字段
（`key_overrides` 优先级高于 `use` 的 `predicate_overrides`）。被覆盖的是**实体键字段**：
`match<...>` 的键（`on each` 无 match key，则为 `entity(...)` 的单字段 `实体字段`）∪ 用例头
显式写的字段。这些字段在 `use` / `without` 里再写一律报 **VN12** —— 不是多此一举：写进去的值
会被静默丢掉，数据里是实体 id 派生的（首个用例从 `0` 起、用例间分段）。

要命中以**键取值**为条件的规则（如 `b.auction % 123 == 0`），靠的是这个分配规律，而不是在
`use` 里指定值。注意 `match<seller>` + `entity(…, b.auction)` 这类 join-then-key 规则：被覆盖的是
`seller`，`auction` 不在覆盖之列，写它**不报错**（但实体标识可能对不上，`gen` 会打 `unasserted` 警告）。

**用例体：四类语句（顺序无关）**

| 语句 | 写法 | 占「步骤位」？ | 参与断言？ |
|---|---|---|---|
| 事件组 | `[then] use <值来源> x N` | 是（对齐规则的事件步骤，**VN24**） | 是（注入这些事件） |
| 否定约束 | `[then] without(preds) [within D]` | **否** | 否（只剔除背景噪声，自己不发事件） |
| 跨流注入 | `join <目标窗> as <右键字段> { <事件组>+ }` | **否** | 否（右事件不产生独立实体） |
| 时间铺开 | `spread D` | 否 | 否 |

- `x N` 是「**每个实体**在这条步骤上的条数」，`N = 0` 报 **VN21**。
- `then` 是可选前缀，只影响可读性（「接着」），语义上与不写等价。
- `without(...)` 不写条数；`within D` 省略时取目标规则 `match` 的窗口长度；超过
  `#[duration]` 报 **VN25**。它的谓词同样过 VN9 / VN11 / VN12。
- `join` 块里每个事件组也写 `x N`；`join` 的形态（右事件放同刻还是提前 1ms）由**规则**
  决定，不由 `.wfg` 语法决定；匹配不到规则 join 子句报 **VN30**。

**`use` 的值来源（三种）**

| 写法 | 形态 | 取值规则 |
|---|---|---|
| `use(f1=v1, f2=v2)` | 谓词列表 | 该步骤每条事件都用同一份值 |
| `use({ … })` | 内联 JSON，顶层必须是 **object** | 顶层键展开为字段；`_` 前缀的键（`_stream` / `_window` / `_timestamp`）忽略 |
| `use from "path"` | 文件 | 顶层 object → 一条记录；object 数组 / NDJSON → 多条记录，按**事件序号循环取用**（`N` 大于记录数时回绕） |

值字面量（`v`）可以是字符串、数字、时长、`true` / `false` / `null`，或 `{…}` / `[…]` 结构化值。

字段级检查：同一 `use` 内重复字段 → **VN9**；字段不在该 stream 的 schema → **VN11**；
重复了生成器会覆盖的字段（规则 key / 实体字段，见 §4.4）→ **VN12**；`use({…})` / 文件形态非法 → **VN17**。

### 4.5 `replay <窗口> { use from "…" }`

把一份现成数据原样灌进去（不写条数、不做实体数学、不参与实体断言），可写多条：

```wfg
replay conn_events { use from "raw/monday.ndjson" }
```

- 值来源**只能是文件**（`use(...)` / `use({...})` 报 **VN20**）。
- **不写 `x N`**，写了报 **VN20**（条数由文件决定）。
- 目标窗口要在 schema 里（**VN3**），但**不必**写进 `background`。
- 时间：`_timestamp` 优先，其次 schema 的时间字段；按位宽归一化（秒/毫秒/微秒/纳秒），
  以文件**最早一条**为锚平移到场景起点。无时间字段时按序号在 `#[duration]` 内均匀落下。
- 文件为空、时间字段口径不齐 → **VN26**；平移后超出 `#[duration]` → **VN25**。

## 5. 数量与时间口径

```text
注入条数 = Σ 用例(实体数 × Σ 事件组 x N)
背景条数 = Σ stream(rate × duration)
总条数   = 背景条数 + 注入条数          // 两者完全分离，互不挤压
```

- **没有任何隐式除法**：不写百分比、不从配额反推。
- `hit` / `near_miss` 的实体是「一个实体占一个 id」；`miss` 是「每个事件一个独立键」，
  因此 `miss` 用例占用的 id 数 = 用例头实体数 × ΣN（这是 `miss` 不成簇、能构造负样本的前提）。
- 用例之间**实体键空间分段**，所以 `hit` 与 `near_miss` 不会指向同一个实体。
- `spread D` 覆盖默认铺开跨度（默认 = 规则窗口长度）：各实体的簇在
  `[0, duration − 窗口]` 上**等距**排开，首簇贴 `0`、末簇贴到场景末尾；`D` 超过
  `#[duration]` 报 **VN25**。
- 断言以**实体**为单位，背景事件不参与。

## 6. 校验码

码族按校验域划分，族内编号递增、删除的旧号不复用：`VN` = `.wfg` 语法与注入语义
（校验期，`lint` / `gen` 加载阶段），`INJ` = 生成期断言。旧 `SC` / `SV` 两族已随旧语法退役。

| 码 | 触发 |
|---|---|
| `VN1` | `background` 里一个 stream 都没有 |
| `VN2` | 速率 ≤ 0 |
| `VN3` | stream 不在已加载 schema 里 |
| `VN9` | 同一 `use` 内重复字段 |
| `VN10` | 用例的 stream 没在 `background` 里声明 |
| `VN11` | `use` / `without` 的字段不在该 stream 的 schema 里 |
| `VN12` | `use` / `without` 里写了**实体键字段**（`match<...>` 的键；`on each` 规则为 `entity(...)` 的单字段；join 块为连接键）——这些字段的值由实体 id 分配，写了会被静默丢掉，所以拦下来 |
| `VN14` | `for RULE` 指向的规则不在已加载 `.wfl` 里 |
| `VN17` | `use({…})` / `use from` 的记录形态非法（非 object、数组元素不是 object、数组为空） |
| `VN20` | 用了旧语法（`hit<N%>` / `with(N)` / `not(...) within(...)` / `traffic` / `injection` / `expect` / `<field> seq` / `replay` 写条数），文案给出改写方向 |
| `VN21` | 实体个数为 0、某事件组 `x 0`、或没有任何事件组 |
| `VN22` | 显式实体字段不在该 stream 的 schema 里 |
| `VN23` | 显式实体字段与规则推断的实体字段不一致 |
| `VN24` | `use` 事件组数超过规则的事件步骤数 |
| `VN25` | `spread` / `without ... within` / `replay` 文件跨度 超过 `#[duration]` |
| `VN26` | `replay` 文件为空，或文件里的时间字段口径不齐 |
| `VN27` | 场景的实体 id 总数达到 2²⁴ 上限 |
| `VN28` | 背景速率用了未实现的 `wave(...)` / `burst(...)` / `timeline { … }` |
| `VN29` | 注解键不在白名单（`#[…]` 只认 `duration`、`<…>` 只认 `seed`），或值类型不合法 |
| `VN30` | `join <窗口> as <键>` 匹配不到规则的 join 子句，形态不支持，或规则的 `within` 区间不含左事件时间 |
| `VN31` | `entity … zipf(…)` 的窗口 / 字段 / 类型 / 参数不合法，或与注入实体的值域预算超限 |
| `VN32` | 重复书写 `background` / `inject` 块（**解析期**报错）。两者都是单例，不会合并 |
| `INJ1` | **生成期**：`hit` 实体没产出告警 |
| `INJ2` | **生成期**：`near_miss` / `miss` 实体产出了告警 |

## 7. 已删除 / 未实现的语法

**旧语法一律报 `VN20`**，文案里给出改写方向：

| 旧写法 | 改成 |
|---|---|
| `traffic { … }` | `background { … }` |
| `injection { … }` | `inject { … }` |
| `hit<N%>` | `hit<实体数>`（`实体数 ≈ round(配额 × N%) ÷ 每实体条数`） |
| `use(...) with(N)` | `use(...) x N` |
| `not(...) within(...)` | `without(...) [within D]` |
| `<field> seq { … }` | 删掉，实体字段写在用例头或由规则推断 |
| `expect { … }` | 删掉，断言由 `hit` / `near_miss` / `miss` + INJ1/INJ2 承担 |

**语法可解析但语义未实现**：`wave(...)` / `burst(...)` / `timeline { … }`（速率波形）——
校验期报 `VN28`。

## 8. 完整示例

```wfg
use "../schemas/conn.wfs"
use "../rules/scan_then_xfer.wfl"

#[duration=10m]
scenario scan_then_xfer_ok<seed=7> {
  background {
    stream conn_events gen 100/s
    entity conn_events.sip zipf(pool=1000, exponent=1.1, fresh=0.2)
  }

  inject {
    // 20 个实体 × (3 + 1) = 80 条；断言：这 20 个都必须报警
    hit<sip: 20> for scan_then_xfer conn_events {
      use(action="scan") x 3
      then use(action="xfer") x 1
      without(action="login_ok") within 5m   // 窗口内不得出现成功登录
      spread 8m
    }

    // 跨流注入：为每条左事件在目标窗造 1 条右事件（键与时间自动推导）
    hit<id: 200> for q8_monitor_new_user person_events {
      use(name="n") x 1
      join auction_events as seller {
        use(price=7) x 1
      }
    }
  }

  replay conn_events { use from "raw/monday.ndjson" }
}
```

## 相关文档

- 怎么用（编写流程、按任务的配方、常见坑）：[`scenarios.md`](./scenarios.md)
- 设计与语义推导、旧语法迁移折算、后续规划：[`../design/wfg-design.md`](../design/wfg-design.md)
- 命令参数（`lint` / `gen` / `verify` / `send`）：[`cli/cli.md`](./cli/cli.md)
