# MoJu Draft Review — Warp-Fusion

从 `wfgen.facts.json` (71 types) 和 `wfl.facts.json` (10 types) 通过 AI 语义合并生成。

`moju verify moju-draft` ✅ 全部 5 个 verify case 通过。

---

## 高置信度（代码事实，建议直接接受）

| Domain | 变更 | 依据 |
|--------|------|------|
| cli | `Commands` 合并 wfgen (Gen/Lint/Verify/Send/Bench) + wfl (Explain/Fmt/Replay/ReplayVerify/Test) | 两个 crate 的 CLI 子命令合并 |
| wfg | WFG AST 39 个类型完整建模 | `wfgen.facts.json` wfg_ast 模块 |
| wfg | Data generation 12 个类型 | `wfgen.facts.json` datagen 模块 |
| wfg | Oracle + Verify 10 个类型 | `wfgen.facts.json` oracle + verify 模块 |
| replay | Replay 6 个类型 | `wfl.facts.json` cmd_replay 模块 |

---

## 推断性变更（需要人工审查）

| 变更 | 推断依据 | 风险 |
|------|---------|------|
| `Generate` flow 步骤划分: ParseScenario → LoadDependencies → GenerateStreams | 对应 wfgen pipeline 实际流程 | 中 — 步骤可能过细或遗漏 |
| `Verify` flow 步骤划分: RunOracle → MatchAndCompare → Report | 对应 verify pipeline | 中 — MatchResult 放到 Report step 还是独立 step 待定 |
| 域聚类: cli / wfg / replay 三个域 | 基于 module 边界和 struct_relations 密度 | 低 — wfgen+wfl 两个 crate 自然分离 |
| `ReplayEngine` struct 包含 `CepStateMachine` + `RuleExecutor` | 字段关系来自 facts | 低 — 代码事实 |

---

## 模型中可能遗漏

| 类型 | 说明 |
|------|------|
| `error::WfgenStructExt`, `error::WflStructExt` | 错误扩展 trait，基础设施，跳过 |
| `Measure` 类型 | `StepInfo.measure` 引用，但未在 facts 中独立定义（可能来自 wf-core） |
| `Duration` 类型 | 多处引用 (`window_dur`, `within`, `start`, `end`)，未在 facts 中独立定义 |
| `CepStateMachine`, `RuleExecutor`, `ConvPlan` | 来自 wf-engine/wf-core，warp-fusion 作为外部依赖使用 |

---

## 建议的下一步

1. **人工审查** wfg domain 的 flow 设计（Generate, Verify）
2. **确认** Commands 合并是否包含所有实际子命令
3. **补充** 缺失的外部类型声明（Duration, Measure, CepStateMachine）
4. **确认** review 通过后执行: `cp -r moju-draft/domain/* moju/model/domain/ && cp moju-draft/architecture.mju moju-draft/dataflow.mju moju/model/`
