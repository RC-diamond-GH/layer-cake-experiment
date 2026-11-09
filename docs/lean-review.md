# Lean 机制对照

## 1 机制与适用范围

选定机制为单个订阅的投递闸门。Rust `src/events/gate.rs` 中的 `DeliveryGate` 被 EventHub 用于投递准入；Lean `AgentTheory/MVP.lean` 在 `AgentTheory.MVP.DeliveryGate` 中定义相同状态与转移。

审阅结论：在顺序调用、事件身份相等关系保持、字段由机制接口更新的假设下，两份实现对活动状态、已准入集合以及准入返回值具有相同转移。该结论覆盖闸门的准入与取消。EventHub 的匹配、保留历史、队列清理及 Agent 调度通过 Rust 行为检查验证。

## 2 状态与输入映射

| Rust | Lean | 映射 |
| --- | --- | --- |
| `active: bool` | `active: Bool` | 相同真值 |
| `seen: BTreeSet<String>` | `seen: Finset Nat` | 字符串身份通过保持并反映相等的单射编码到自然数 |
| `admit(&str)` | `admit gate eventId` | 同一事件身份输入 |
| 原地更新状态和返回 `bool` | `(下一状态, Bool)` | 取 Rust 调用后的状态及返回值 |
| `cancel()` | `cancel gate` | 设为 inactive，保留 seen |

两份 `admit` 均按如下分支转移：inactive 时原样拒绝；已见时原样拒绝；active 且新身份时插入 seen 并返回 true。`cancel` 关闭活动标志。Rust 有序集合与 Lean 有限集合在此使用集合成员语义。

## 3 已证明性质

| Lean 定理 | 性质 |
| --- | --- |
| `admit_of_inactive` | inactive 拒绝并保留状态 |
| `admit_of_seen` | 已见身份拒绝并保留状态 |
| `admit_of_fresh_active` | 新身份成功插入 |
| `admit_active_preserved` | 准入保留活动标志 |
| `admit_seen_monotone` | 已见集合单调增长 |
| `admitted_event_seen` | 成功准入的身份进入 seen |
| `admit_duplicate`、`duplicate_suppressed` | 第二次准入同一身份被拒绝 |
| `admit_next_idempotent` | 重复准入后的状态幂等 |
| `cancel_absorbs_admit` | 取消吸收所有后续准入 |
| `cancel_idempotent`、`cancel_seen_preserved` | 取消幂等且保留已见集合 |

证明依赖 Lean 标准公理 `propext`、`Classical.choice`、`Quot.sound`。核心性质经 `#print axioms` 检查，证明由内核验证。

## 4 构建与运行

```bash
lake build
printf 'import AgentTheory.MVP\nopen AgentTheory.MVP\n#eval (DeliveryGate.initial.admit 7).2\n#eval ((DeliveryGate.initial.admit 7).1.admit 7).2\n#print axioms DeliveryGate.admit_duplicate\n#print axioms DeliveryGate.cancel_absorbs_admit\n#print axioms DeliveryGate.admit_seen_monotone\n' | lake env lean --stdin
```

两次计算依次为 true、false。Rust 对应算法为确定性的集合判定与插入。审阅假设调用顺序一致、身份编码单射、集合操作完成；并发访问和运行时分配失败由各运行时负责。

## 5 原形式化模型

`AgentTheory/Basic.lean` 保留通用 `Automaton`、`Configuration`、初始化、一步转移、有限步运行和退出吸收性质。`AgentTheory/Examples.lean` 保留两任务 Workflow 的终止证明与有限控制状态自循环实例。MVP 闸门证明通过 `AgentTheory.lean` 一并导入。

## 6 对应版本

2026-11-09 由 AI 对照上述两个源文件的状态、输入与全部转移分支完成审阅。对应源文件以 SHA-256 固定：

| 源文件 | SHA-256 |
| --- | --- |
| `src/events/gate.rs` | `394e37024bbca68153935dbefd710b6ad9638b719fb9c98f4f7c1065e88f7114` |
| `AgentTheory/MVP.lean` | `1d9e47dab298ffc126e6183a4edb06f5c842d52202fbdd11ba30d070305167bc` |

`lake build` 已通过；第 3 节列出的三个核心性质已执行公理检查。源文件发生变化后，应重新执行构建与分支对照，并更新对应哈希。
