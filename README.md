# warp-fusion

[![CI: build & test](https://github.com/wp-labs/warp-fusion/actions/workflows/build-and-test.yml/badge.svg?branch=alpha)](https://github.com/wp-labs/warp-fusion/actions/workflows/build-and-test.yml)
[![release](https://img.shields.io/github/v/tag/wp-labs/warp-fusion?include_prereleases&label=release&color=orange)](https://github.com/wp-labs/warp-fusion/releases)
![license: ELv2](https://img.shields.io/badge/license-ELv2-blue.svg)
![lang: Rust](https://img.shields.io/badge/lang-Rust-000000.svg)
![status: active](https://img.shields.io/badge/status-active-brightgreen.svg)

**WarpFusion  是高性能Ai Native 的实时计算引擎

## 目录

- [快速开始](#快速开始)
- [一个最小的规则](#一个最小的规则)
- [Workspace 组件](#workspace-组件)
- [示例导览](#示例导览)
- [文档](#文档)
- [能力定位与性能参照](#能力定位与性能参照)
- [架构亮点](#架构亮点)
- [边界声明](#边界声明)
- [License](#license)

## 快速开始

### 安装（推荐）

一行命令安装 `warp-fusion` 套件（`wfusion` 引擎及配套 CLI，默认装到 `~/bin`）：

```bash
# stable（默认通道，推荐生产）
curl -sSf https://get.warpparse.ai/inst-x.sh | bash -s -- wfusion
 
# 预发布通道：alpha / beta（新语法验证、与引擎开发线对齐时使用）
curl -sSf https://get.warpparse.ai/inst-x.sh | bash -s -- wfusion alpha
curl -sSf https://get.warpparse.ai/inst-x.sh | bash -s -- wfusion beta
```

安装后确保 `~/bin` 在 PATH 中（脚本会提示）：

```bash
export PATH="$HOME/bin:$PATH"
```

### 快速体验（示例项目集合）

示例项目在独立仓库 [wf-examples](https://github.com/wp-labs/wf-examples)。
`nginx_log_stats/` 为最小业务示例——对 Nginx access 日志做**持续流式统计**
（状态码 / 来源 IP，5s 固定桶）+ **5xx 突发检测**，看板实时展示
（直读引擎输出 `data/alerts/nginx.ndjson`，每 3s 自动刷新）：

```bash
# 确保 wfadm / wfusion / wfgen 在 PATH（安装见上方；前置细节以
# wf-examples/nginx_log_stats 的 README 为准），然后：
git clone https://github.com/wp-labs/wf-examples
cd wf-examples/nginx_log_stats
./run.sh     # ① 持续运行：wfusion daemon + wfgen stream 实时注入（Ctrl-C 停止）
./view.sh    #   另开终端：实时看板 → http://localhost:8123/view/
```

看板展示累计请求 / 独立 IP（Top 10 + 总数）/ 状态码分布 / 请求时间线，以及 5xx
突发明细（时间 / IP / URI）。

## Workspace 组件

| 二进制 | 作用 |
| --- | --- |
| `wfusion` | 引擎主二进制（本地文件/网络源回放 → 规则执行 → alert/错误输出） |
| `wfl` | 规则开发工具：`lint` / `test`（规则内联用例）/ `replay` / `verify` |
| `wfgen` | 数据生成与 oracle 验证；含 `nexmark_pk` 基准工具链 |
| `wfadm` | 管理 CLI（Admin API 状态查询、在线 reload、发布流程） |

各 crate 变更见 [CHANGELOG.md](./CHANGELOG.md) / [CHANGELOG.en.md](./CHANGELOG.en.md)。

## 文档

> **用 AI / Agent 辅助开发？建议优先使用 [wf-skills](https://github.com/wp-labs/wf-skills)**——
> 产品级技能集（Claude Code / Codex / 各类 agent 通用），把本仓库经验沉淀为
> 「何时用 → 怎么做 → 已踩过的坑 → 检查清单」，覆盖 schema / 规则 / 配置 /
> **系统集成（5 步接入）** / 基准与正确性验证。一条命令安装：
>
> ```bash
> curl -sSf https://get.warpparse.ai/inst-x.sh | bash -s -- wf-skills
> ```

- **快速上手 / 概念**：[getting-started.md](docs/useage/getting-started.md) · [warp-fusion-intro.md](docs/warp-fusion-intro.md)
- **开发者集成**：[integration.md](docs/useage/integration.md)（把引擎接入自有系统：来源 → 窗口 → 输出路由 → 规则）
- **WFL 语言**：[rules.md](docs/useage/rules.md)
- **运行与配置**：[config](docs/useage/config/) · [cli](docs/useage/cli/cli.md)
- **Admin API / 在线 reload / 发布**：[admin_api.md](docs/useage/cli/admin_api.md)
- **设计与能力**：[design](docs/design/) · [warp-fusion-competitiveness.md](docs/warp-fusion-competitiveness.md)

## 能力定位与性能参照

`warp-fusion` 定位为**通用流处理引擎**，以 WFL 高层语义 DSL 表达规则，轻量化运行。

### NEXMark 性能参照

与 Flink 系**同方法论**对照（100M 事件、in-memory 源 + blackhole 汇、同型号云服务器）：

| 对照基线                         | 几何平均领先    | 算术平均领先 |
| ---------------------------- | --------- | ------ |
| Flink OSS（3×12 vCPU / 48GiB） | **24.3×** | 44.7×  |
| 阿里 VVR（8 CU / 32GiB 托管集群）    | **6.8×**  | 10.1×  |

完整口径与逐查询数据见 [NEXMark PK 报告](https://github.com/wp-labs/wf-examples/blob/main/performance/nexmark_pk/NEXMARK_PK_REPORT.md)。

![WarpFusion vs Flink NEXMark 对照](images/vs-flink.jpg)

### WFL 表达能力

![WFL 五原语 Core IR](images/wfl-five-primitives.svg)

- **五原语内核（Bind / Match / Stats / Join / Yield）**：既写逐事件流式检测，也写声明式窗口统计（`stats<dur> [group by] { 聚合 }`）。
- **检测表达力为核心差异化**：时序链 + OR 分支 + 双阶段匹配（实时/窗口关闭），缺失检测（A→NOT B）；一等实体声明 `entity()` 驱动跨规则评分。
- **覆盖范围**：哈希族、网络 `cidr_match`、多精度时间、对象 `merge`、HOP 跳窗、`anti`/延迟触发 join、规则级 `let`、表达式派生分组 key 等；与 SPL Top50 高频函数对齐率 100%（50/50）。
- **诚实边界**：三角函数、行保留聚合（eventstats 类）等通用计算不在主战场；分项可解释评分为规划项。

## 架构亮点

| 杠杆               | 砍掉了什么                                                 |
| ------------------ | ---------------------------------------------------------- |
| **列批式向量化**  | 逐事件对象分配 + 解释器分发                                |
| **数据零拷贝**     | 消灭 Event→Record→DataRecord 多层拷贝                      |
| **内存精确控制**   | 窗口数据仅过期且被下游全部消费后才释放、数据预读总量设上限 |
| **Rust vs Java**   | 免去 Java 系引擎（Flink 等）的 JVM GC 停顿                 |
| **规则即规划**     | 运行期逐事件解释（Stats/Match 编译期定型为执行计划）       |

## 边界声明

上述领先在**「引擎纯算力 / 单机内存」隔离维度**测得：当前为**单机、无 exactly-once / checkpoint(规划) / 分布式协调开销**。NEXMark 为合成基准，结论作**能力参照**而非生产 SLA 承诺；生产级容错、分布式与有状态一致性补齐后方可对等比较。

## License

`warp-fusion` 及核心运行时采用 **Elastic License 2.0 (ELv2)**。

- **允许**：个人、研究、教学、非营利组织，以及企业**内部自用**。
- **禁止**：将本软件作为**托管服务 / 产品对外提供**、销售本软件本身、或绕过授权限制。
- 任何超出上述免费范围的商业用途，需与版权人另行签署商业授权协议。

完整条款见 [LICENSE](./LICENSE)；版权归属 `Copyright (c) 2026 zuowenjian`。

> 注：ELv2 不属于 OSI 认证的开源协议（source-available），但允许企业内部使用。
