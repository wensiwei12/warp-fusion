请读取 `.moju/ai/context.md`。

任务：
当前 module 里的 Struct 过多，请进行合理拆分

任务 ID：
structures.split_many_structs

当前视图：
Structures

当前选中：Struct `Replay.ConsumerRoute`

具体要求：
请读取 `.moju/ai/context.md`，从 Structures 视图角度分析当前 module 里的 struct 过多问题，并进行合理拆分。

重点检查：
1. 当前选中 module 的 owns 是否包含过多 struct，导致职责边界不清。
2. 这些 struct 是否可以按业务职责、聚合边界或 bounded context 拆成多个 module。
3. 是否存在过大的 struct，需要拆分为更清晰的业务概念或值对象。
4. 是否存在重复、近似或命名不清的 struct，需要合并或重命名。
5. 是否有实现细节 struct 混入当前 module，应该移动到 config / topology / binding / implementation 相关模型。
6. 拆分后 Structure 视图是否能按 module 更清晰地查看。

改进要求：
- 优先调整 module 定义、module owns 和 module depends 来拆分当前 module。
- 对确实过大的 struct，可以拆分字段到新的业务 struct 或值对象。
- 不要为了减少数量而删除真实业务概念。
- 保持 flow、event、interface、binding 中对 struct 的引用一致。
- 修改后运行 `moju verify .` / `moju readiness .`，或项目已有验证命令。

通用约束：
1. 不要改无关文件。
2. 保持现有模型语义，不要做无关重构。
3. 如果必须修改项目实现代码，先说明原因，并保持改动最小。
4. 最后总结修改内容、验证命令和结果。
