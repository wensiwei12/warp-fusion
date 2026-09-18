# CLI 命令参考

> Admin API 的状态查询、在线 reload 和发布操作详见 [`admin_api.md`](admin_api.md)。

## `wfusion` — 统一入口

```bash
wfusion <subcommand>
```

### `wfusion run` — 启动引擎

```bash
wfusion run -c ./wfusion.toml
```

| 参数 | 说明 |
|------|------|
| `-c, --config` | 配置文件路径（默认 `conf/wfusion.toml`） |
| `--overlay` | 叠加配置文件（可重复） |
| `--var` | 覆盖变量 `KEY=VALUE`（可重复） |
| `--work-dir` | 运行时工作目录 |
| `--metrics` | 启用运行时指标 |
| `--metrics-interval` | 指标上报间隔 |
| `--metrics-listen` | 指标监听地址 |

### `wfusion config` — 配置检查

```bash
# 渲染完整配置（合并 overlay + 变量展开）
wfusion config render -c wfusion.toml [--raw]

# 查看每个配置项的来源文件
wfusion config origins -c wfusion.toml [--path-prefix runtime]

# 查看所有变量的值和来源
wfusion config vars -c wfusion.toml [--var-prefix WORK_]

# 比较两个配置的差异
wfusion config diff -c wfusion.toml --to-config other.toml [--expanded]
```

### `wfusion rule` — 规则工具

```bash
# 解释编译后的规则（渲染 match plan）
wfusion rule explain --file rules/test.wfl

# Lint 检查
wfusion rule lint --file rules/test.wfl

# 格式化规则文件
wfusion rule fmt rules/*.wfl [--write] [--check]

# 离线回放
wfusion rule replay --file rules/test.wfl --input data/events.ndjson

# 回放 + 验证（对比 Oracle）
wfusion rule verify --file rules/test.wfl --case mycase

# 运行合约测试
wfusion rule test --file rules/test.wfl [--shuffle] [--runs 10]
```

---

## `wfl` — 规则开发（独立工具）

```bash
# 内联测试
wfl test rules/test.wfl --schemas "schemas/*.wfs"

# 离线回放
wfl replay rules/test.wfl --input data/events.ndjson

# 格式化
wfl fmt rules/*.wfl --write

# Lint 检查
wfl lint rules/test.wfl --schemas "schemas/*.wfs"

# 解释编译结果
wfl explain rules/test.wfl --schemas "schemas/*.wfs"

# 回放 + 验证
wfl verify rules/test.wfl --case mycase [--score-tolerance 0.1] [--time-tolerance 5]

# 合约测试
wfl test rules/test.wfl --schemas "schemas/*.wfs" [--runs 100]
```

---

## `wfgen` — 场景生成（独立工具）

从 `.wfg` 场景文件生成"带标签"的测试数据（背景流量 + 定向构造的 hit / near_miss / miss
实体），并在写期望文件之前用硬断言校验标签。场景语法见 [`../scenarios.md`](../scenarios.md)。

```bash
# 校验场景（语法 / 字段 / VN 系列错误）
wfgen lint models/scenarios/port_scan.wfg

# 生成事件 + 期望告警（*.except.jsonl / *.except.meta.jsonl）
wfgen gen --scenario models/scenarios/port_scan.wfg --out out/gen
wfgen gen --scenario s.wfg --format arrow --out out/gen      # 列式 .arrow 输出
wfgen gen --scenario s.wfg --no-oracle --out out/gen         # 只出事件，不写期望文件（仍编译 WFL，注入 use() 生效）
wfgen gen --scenario s.wfg --no-wfl --out out/gen            # 跳过整个 WFL 管线（纯背景随机事件）
wfgen gen --scenario s.wfg --send --addr 127.0.0.1:9800      # 直接发给引擎
wfgen gen --scenario s.wfg --duration 1m --out out/gen       # 覆盖 #[duration]（背景按比例缩，注入不变）

# 对拍：实际告警 vs 期望告警
wfgen verify --expected out/gen/port_scan.except.jsonl --actual out/alerts.ndjson
#   [--meta out/gen/port_scan.except.meta.jsonl] [--score-tolerance 0.1] [--format markdown]

# 发送已生成的事件 JSONL（TCP + Arrow IPC）
wfgen send --scenario s.wfg --input out/gen/port_scan.jsonl [--chunk 5000] [--rate-ms 10]

# 压测生成吞吐（可选 --send）
wfgen bench --scenario s.wfg [--duration 30s] [--send]

# daemon：循环生成多个场景（--wfl 必给，注入要按规则构造）
wfgen stream --scenario-dir models/scenarios --wfl "rules/*.wfl" [--rate 100000] [--interval 60]
```

| 附加参数 | 说明 |
|---|---|
| `--duration <literal>` | 覆盖场景时长（`30s` / `10m` / `2h` / `1d`）。校验按**生效时长**跑，所以 `spread` / `without ... within` / `replay` 跨度仍需 ≤ 该值（VN25） |
| `--ws <file>` | 额外的 `.wfs`（`use` 声明之外） |
| `--wfl <file>` | 额外的 `.wfl`（`use` 声明之外） |

### 仓内语料回归

仓内 `.wfg`（`crates/wfgen/examples/*`、`crates/wfadm/templates/*`、`docker/default_setting/*`）
有两条自动化的对拍测试，随 `cargo test` 一起跑：

```bash
cargo test -p wfgen --test wfg_corpus      # L0–L2：VN 校验 + INJ1/INJ2 断言 + 期望文件 + 条数守恒
cargo test -p wfgen --test wfg_corpus_l3   # L3：生成 → oracle → 真实引擎 → verify 对拍
WFC_CORPUS_ONLY=conv cargo test -p wfgen --test wfg_corpus   # 本地只跑一部分（按路径子串过滤）
```

其它子命令（性能 / 联调工具，参数见 `wfgen <cmd> --help`）：`gen-nexmark` ·
`verify-nexmark` · `diff` · `dump-frames` · `send-arrow` · `shard-frames` · `perf-diag`。
