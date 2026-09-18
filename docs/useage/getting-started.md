# 快速开始

## 安装

```bash
git clone https://github.com/wp-labs/warp-fusion.git
cd warp-fusion
cargo build --release
```

编译产物：
- `target/release/wfusion` — 统一入口（引擎 + 规则工具）
- `target/release/wfgen` — 场景生成（可选）
- `target/release/wfl` — 规则开发（可选）

## 第一个示例

以 `port_scan_whitelist` 为例：

```bash
cd examples/rules/port_scan_whitelist

# 1. 内联测试 —— 验证规则逻辑
wfl test rules/port_scan_whitelist.wfl --schemas "schemas/*.wfs"

# 2. 离线回放 —— 用历史数据验证
wfl replay rules/port_scan_whitelist.wfl --input data/conn_events.ndjson

# 3. 引擎运行 —— 完整管道
wfusion run -c ./wfusion.toml
```

## 用 `.wfg` 场景生成"带标签"的测试数据

没有历史数据时，用 `.wfg` 描述场景：一段背景流量 + 若干**定向构造**的实体。生成出来的
数据自带标签——`hit` 的实体必须报警、`near_miss` / `miss` 的实体必须不报警，`wfgen gen`
在写期望文件之前就把这件事断言掉：

```bash
# 校验场景（语法、字段、VN 系列错误都在这一步暴露）
wfgen lint crates/wfgen/examples/distinct/scenarios/port_scan.wfg

# 生成事件 + 期望告警（*.except.jsonl 可与真实引擎输出对拍）
wfgen gen --scenario crates/wfgen/examples/distinct/scenarios/port_scan.wfg --out out/gen

# 拿生成的事件喂引擎后，逐条对拍
wfgen verify --expected out/gen/port_scan.except.jsonl --actual out/alerts.ndjson
```

场景语法见 [`scenarios.md`](scenarios.md)；仓库内可直接照抄的场景样本：

| 路径 | 侧重 |
|------|------|
| `crates/wfgen/examples/count/scenarios/` | count 阈值 |
| `crates/wfgen/examples/distinct/scenarios/` | distinct 基数 |
| `crates/wfgen/examples/{avg,sum,conv,multi_step}/scenarios/` | avg / sum / conv top-N / 多步链 |
| `crates/wfadm/templates/models/scenarios/` | `wfadm init` 项目模板自带的 4 个场景 |

## 目录结构

```
warp-fusion/
├── wfusion.toml           # 主配置
├── rules/                 # .wfl 规则文件
│   └── _global.wfl        # 可选：项目级 yield preset
├── schemas/               # .wfs schema 文件
├── sinks/                 # sink 配置
│   ├── infra.d/           #   基础设施 sink（default/error/monitor）
│   ├── business.d/        #   业务路由 sink
│   ├── connectors/        #   connector 定义
│   └── defaults.toml      #   全局 sink 默认值
├── data/                  # 离线回放数据
├── out/                   # 输出目录
├── examples/              # 检测场景示例
└── docs/                  # 文档
```

## 示例

| 示例 | 检测场景 | 核心模式 |
|------|---------|---------|
| `port_scan_whitelist/` | 端口扫描 + 白名单 | distinct + count + join anti |
| `ssh_brute_force/` | SSH 暴力破解 | count 阈值 + 多目标 |
| `sqli_probe/` | SQL 注入探测 | regex_match + count |
| `rat_propagation/` | 远控扩散 | 多步 scan→login→xfer |

详见 [`examples/rules/README.md`](../../examples/rules/README.md)。

## 文档索引

| 文档 | 内容 |
|------|------|
| [`config/configuration.md`](config/configuration.md) | `wfusion.toml` 完整配置参考 |
| [`config/wparse-window-routing.md`](config/wparse-window-routing.md) | `warp-parse` 输出如何分发到 window |
| [`config/schema.md`](config/schema.md) | `.wfs` Schema 定义 |
| [`rules.md`](rules.md) | `.wfl` 规则编写、yield 时间变量、稳定统计上下文 |
| [`scenarios.md`](scenarios.md) | `.wfg` 场景：背景/注入、三种模式=硬断言、数量口径、值与值来源 |
| [`cli/cli.md`](cli/cli.md) | CLI 命令参考 |
| [`config/metrics.md`](config/metrics.md) | 监控指标配置 |
