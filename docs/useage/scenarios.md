# `.wfg` 场景：为规则生成"带标签"的测试数据

`.wfg` 描述**一个场景**：一段背景流量 + 若干**定向构造**的实体。生成出来的数据自带
标签——`hit` 的实体必须报警、`near_miss` / `miss` 的实体必须不报警，`wfgen gen`
在写期望文件之前就把这件事断言掉（INJ1 / INJ2）。因此 `.wfg` 既是"造数据"，也是
"造验收标准"。

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

## 文件结构

| 元素 | 说明 |
|---|---|
| `use "x.wfs"` / `use "x.wfl"` | 相对 `.wfg` 所在目录解析；`.wfs` 给字段类型，`.wfl` 给规则 |
| `#[duration=10m]` | 场景虚拟时长（`30s` / `2m` / `2h` …） |
| `scenario NAME<seed=N>` | 场景名 + 随机种子 |
| `background { stream S gen R/s }` | 背景流量：只决定"除定向构造外还有多少随机事件" |
| `inject { … }` | 定向构造：每个用例声明**模式 + 实体个数 + 事件组** |

## 三种模式 = 硬断言

| 模式 | 语义 | 断言（生成期，INJ1/INJ2） |
|---|---|---|
| `hit` | 这个实体**应该**报警 | 每个 `hit` 实体在其窗口内必须产出告警，否则 FAIL |
| `near_miss` | 差一点，但**不该**报警 | 每个 `near_miss` 实体必须不产出告警，否则 FAIL |
| `miss` | 与规则无关，**不该**报警 | 每个 `miss` 实体必须不产出告警，否则 FAIL |

- 断言复用 oracle 结果，失败在**写期望文件之前**报出，不会留下半成品侧车文件。
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
  规则窗口长度铺开。各实体的窗口在场景时长内**等距**排开（首尾分别贴着起点与终点），
  簇内事件按步骤顺序在窗口内均匀落下。

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

## 常见错误码

码前缀按校验域分族：`VN`（`.wfg` 语法与注入语义）、`SC`（stream 与规则 / schema 的
绑定）、`SV`（场景基础值、字段类型、oracle 参数）、`INJ`（**生成期**断言）；族内编号递增，
删除的旧号不复用。下表是常见的 `VN` 与 `INJ`：

| 码 | 含义 |
|---|---|
| `VN20` | 用了旧注入语法（`hit<N%>` / `with(N)` / `not(...) within(...)` / `injection` / `traffic` / `expect` / `<field> seq`），文案里给出改写方向 |
| `VN10` | 用例的 stream 没在 `background` 里声明 |
| `VN17` | `use({...})` / `use from` 的形态非法（非 object / object 数组，或空数组） |
| `VN11` | `use(...)` / `without(...)` 的字段不在 schema 里 |
| `VN22` | 显式实体字段不在该 stream 的 schema 里 |
| `VN23` | 显式实体字段与规则推断的实体字段不一致（多 key 规则的显式字段属消歧用法，放行） |
| `VN24` | `use` 事件组数超过规则的事件步骤数（每个 `use ... x N` 对应一个步骤） |
| `VN25` | `spread` 或 `without ... within` 超过 `#[duration]` |
| INJ1 / INJ2 | 生成期断言失败：`hit` 实体没报警（INJ1），或 `near_miss` / `miss` 实体报了警（INJ2） |

排查手法：断言失败时输出会点名是哪个用例、哪个实体、实际与期望的告警数；先确认
"模式是否真的是这个实体应有的行为"，再改数量或过滤条件——不要靠放宽断言让它过。

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

`wfgen lint` / `gen` / `verify` / `send` / `bench` / `stream` 的参数与 `--no-oracle` /
`--no-wfl` 语义见 [`cli/cli.md`](cli/cli.md)；把生成的数据送到引擎联调见
[`integration.md`](integration.md)。
