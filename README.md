# warp-fusion

[![CI: build & test](https://github.com/wp-labs/warp-fusion/actions/workflows/build-and-test.yml/badge.svg?branch=alpha)](https://github.com/wp-labs/warp-fusion/actions/workflows/build-and-test.yml)
[![release](https://img.shields.io/github/v/tag/wp-labs/warp-fusion?include_prereleases&label=release&color=orange)](https://github.com/wp-labs/warp-fusion/releases)
![license: ELv2](https://img.shields.io/badge/license-ELv2-blue.svg)
![lang: Rust](https://img.shields.io/badge/lang-Rust-000000.svg)
![status: active](https://img.shields.io/badge/status-active-brightgreen.svg)

**WarpFusion  是高性能Ai Native 的实时计算引擎**

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
`nginx_log_stats/` 对 Nginx access 日志做**持续流式统计**

```bash
# 确保 wfadm / wfusion / wfgen 在 PATH（安装见上方；前置细节以
# wf-examples/nginx_log_stats 的 README 为准），然后：
git clone https://github.com/wp-labs/wf-examples
cd wf-examples/nginx_log_stats
./view.sh &
./run.sh 
```

## Workspace 组件

| 二进制 | 作用 |
| --- | --- |
| `wfusion` | 引擎主二进制 |
| `wfl` | 规则开发工具 |
| `wfgen` | 数据生成与 oracle 验证|
| `wfadm` | 管理 CLI|

## 文档

> **用 AI / Agent 辅助开发 [wf-skills](https://github.com/wp-labs/wf-skills)**
> ```bash
> curl -sSf https://get.warpparse.ai/inst-x.sh | bash -s -- wf-skills
> ```

- **快速上手 / 概念**：[getting-started.md](docs/useage/getting-started.md) · [warp-fusion-intro.md](docs/warp-fusion-intro.md)
- **开发者集成**：[integration.md](docs/useage/integration.md)（把引擎接入自有系统：来源 → 窗口 → 输出路由 → 规则）
- **WFL 语言**：[rules.md](docs/useage/rules.md)
- **运行与配置**：[config](docs/useage/config/) · [cli](docs/useage/cli/cli.md)

## 能力定位与性能参照

`warp-fusion` 定位为**通用流处理引擎**，以 WFL 高层语义 DSL 表达规则，轻量化运行。

### NEXMark 性能参照

与 Flink 系**同方法论**对照（100M 事件、in-memory 源 + blackhole、同型号云服务器）：

| 对照基线                         | 几何平均领先    | 算术平均领先 |
| ---------------------------- | --------- | ------ |
| Flink OSS（3×12 vCPU / 48GiB） | **24.3×** | 44.7×  |
| 阿里 VVR（8 CU / 32GiB 托管集群）    | **6.8×**  | 10.1×  |

完整口径与逐查询数据见 [NEXMark PK 报告](https://github.com/wp-labs/wf-examples/blob/main/performance/nexmark_pk/NEXMARK_PK_REPORT.md)。

![WarpFusion vs Flink NEXMark 对照](images/vs-flink.jpg)

### WFL 表达能力

- **五原语内核（Bind / Match / Stats / Join / Yield）**：既写逐事件流式检测，也写声明式窗口统计（`stats<dur> [group by] { 聚合 }`）。
- **检测表达力为核心差异化**：时序链 + OR 分支 + 双阶段匹配（实时/窗口关闭），缺失检测（A→NOT B）；一等实体声明 `entity()` 驱动跨规则评分。
- **覆盖范围**：哈希族、网络 `cidr_match`、多精度时间、对象 `merge`、HOP 跳窗、`anti`/延迟触发 join、规则级 `let`、表达式派生分组 key 等；。

## 架构亮点

|   关键设计              |  作用                                                |
| ------------------ | ---------------------------------------------------------- |
| **列批式向量化**  | 减去逐事件对象分配 + 解释器分发                                |
| **数据零拷贝**     | 消灭 Event→Record→DataRecord 多层拷贝                      |
| **内存精确控制**   | 窗口数据仅过期且被下游全部消费后才释放、数据预读总量设上限 |
| **Rust**   | 免去 Java 系引擎（Flink 等）的 JVM GC 停顿                 |
| **规则即规划**     | 运行期逐事件解释（Stats/Match 编译期定型为执行计划）       |

## License

`warp-fusion` 及核心运行时采用 **Elastic License 2.0 (ELv2)**。

- **允许**：个人、研究、教学、非营利组织，以及企业**内部自用**。
- **禁止**：将本软件作为**托管服务 / 产品对外提供**、销售本软件本身、或绕过授权限制。
- 任何超出上述免费范围的商业用途，需与版权人另行签署商业授权协议。

完整条款见 [LICENSE](./LICENSE)；版权归属 `Copyright (c) 2026 zuowenjian`。

> 注：ELv2 不属于 OSI 认证的开源协议（source-available），但允许企业内部使用。
