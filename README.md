# Layer Cake：Agent 机制实验

Rust 提供可复用 Agent 组件、实验执行与观察设施；Lean 4 提供机制性质的形式化模型与证明。

当前实现的初始快照标记为待审基线。后续澄清与修复通过 issue、独立分支、PR、CI 和独立审阅推进，流程见 [版本控制与协作](docs/version-control.md)。

## 1 开始使用

```bash
cargo build
cargo run -- list
cargo run -- run --config examples/react-memory.json --interactive --output runs/manual.json
```

控制台支持 `step`、`continue`、`pause`、`stop`、`input`、`context`、`calls` 和 `report`。每轮完整保存模型调用及顺序工具结果，控制操作在声明的执行边界生效。

```bash
cargo run -- run --config examples/furnace-reactive.json --output runs/reactive.json
cargo run -- report runs/reactive.json --output runs/reactive.html
cargo run -- replay runs/reactive.json
```

## 2 仓库结构

| 路径 | 内容 |
| --- | --- |
| `src/` | Rust 基础组件、实验、控制、回放、报告和 CLI |
| `examples/` | 内存 ReAct、真实模型、熔炉轮询、响应式与计时器配置 |
| `AgentTheory/` | 原自动机定义与示例、MVP 投递闸门证明 |
| `docs/` | 正式使用、设计、对照审阅与更新日志 |
| `materials/` | 需求、机制探索及 MVP 执行文档 |

## 3 文档与证明

[文档首页](docs/index.md) · [完整目录](docs/SUMMARY.md) · [使用指南](docs/usage.md) · [架构与扩展说明](docs/architecture.md) · [Lean 对照审阅](docs/lean-review.md)

```bash
lake build
lake env lean AgentTheory.lean
```

`lean-toolchain` 固定 Lean 4.35.0，`lakefile.toml` 固定 Mathlib v4.35.0。Elan 环境可通过 `source "$HOME/.elan/env"` 加载。原 `Basic.lean` 定义生成后格局与有限步运行；`Examples.lean` 保留 Workflow 终止与自循环 ReAct 反例；`MVP.lean` 证明投递去重与取消性质。`IntroLean.lean` 保留为独立入门示例。

Rust／Lean 的对应审阅限定于明确映射的机制，完整 Agent 行为由 Rust 运行与针对性检查验证。
