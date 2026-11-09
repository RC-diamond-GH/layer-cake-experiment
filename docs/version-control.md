# 版本控制与协作

## 1 仓库与现状基线

公开远端：[RC-diamond-GH/layer-cake-experiment](https://github.com/RC-diamond-GH/layer-cake-experiment)。

| 对象 | 身份与用途 |
| --- | --- |
| 集成分支 | `main` |
| 现状基线提交 | `f460c368392c7010021351e62c3ef62dce0cc363` |
| 基线标签 | `baseline/unreviewed-mvp-2026-11-09` |
| 已知待澄清项 | [实验配置契约与类型判别 #1](https://github.com/RC-diamond-GH/layer-cake-experiment/issues/1) |

基线保留建立流程时的完整实现、正式文档及开发者修改，审阅状态为待审。基线身份用于追溯；设计澄清与行为验收在后续 issue、PR 和审阅记录中完成。发布标签由开发者明确发起的发布流程创建。

初始化顺序为：保存待审基线，提交版本控制流程 PR，经 CI 和独立 AI 审阅后由维护者完成一次引导合并，随后启用第 4 节的服务器保护，再检查合并后的 main CI。该引导过程建立后续提交的强制门禁。

## 2 角色与任务边界

| 角色 | 职责 |
| --- | --- |
| Manager | 澄清问题、记录决策、确定单一变更范围与可观察验收标准，分派 Worker 和 Reviewer |
| Worker | 在独立分支及 worktree 实现任务，提交实际验证证据并创建 PR |
| Reviewer | 只读检查需求对应、接口语义、错误路径、性能及测试有效性，逐项记录发现与结论 |
| 维护者 | 确认 CI、独立 AI 审阅证据和讨论均完成，通过 squash merge 集成 |

CI 负责可执行检查；独立审阅负责设计与实现的对应关系。Agent 的独立审阅使用单独会话或 agent，并在 PR 中注明身份、检查范围、发现及处理结果。当前采用开发者选定的单账号流程：GitHub 强制 PR 和 CI，维护者依独立 AI 审阅证据决定合并。作者发布的 AI 审阅记录明确标注来源，服务器所需的其他账号审批数量为零。

已知问题先登记 issue，区分已观察到的缺陷、待决定的设计和待调查的风险。每个 PR 聚焦一个明确目标；继承问题在 issue 中保留，变更范围以开发者确认的验收标准为准。

## 3 分支与 worktree

一个 Worker 对应一个 `fix/...`、`feat/...` 或 `chore/...` 分支及 worktree。以配置契约任务为例，以下命令在原仓库执行：

```bash
git fetch origin
git worktree add -b fix/config-contract ../layer-cake-worktrees/config-contract origin/main
```

进入新 worktree 后按路径显式暂存、提交并推送，使用 PR 模板填写问题、验收、验证证据和风险：

```bash
git add -- src/experiment/config.rs
# 按本次实际变更继续添加其他明确路径。
git commit -m "fix: clarify experiment configuration contract"
git push -u origin fix/config-contract
gh pr create --base main --head fix/config-contract
```

分支落后于 main 时，在工作分支执行 `git fetch origin` 和 `git merge origin/main`，处理冲突并重新验证。新增提交后 CI 重跑，独立审阅补充覆盖新增改动；如有 GitHub 审批，旧审批随新增提交失效。PR 合并后远端工作分支自动删除；本地 worktree 确认内容已提交后由维护者清理。

本地防护使用版本化 hooks，克隆仓库后在仓库内启用：

```bash
git config --local core.hooksPath .githooks
git config --local pull.ff only
git config --local fetch.prune true
git config --local push.default simple
```

该仓库配置由 linked worktree 共享，hooks 随各 worktree 的工作分支读取。pre-commit 阻止直接在 main 提交并检查暂存路径；pre-push 阻止任何指向远端 main 的更新。本地 hooks 提供即时反馈，服务器保护执行强制集成策略。

## 4 PR、CI 与合并约束

工作流为 `.github/workflows/ci.yml`，对 main 的 push、指向 main 的 PR 及手动触发运行：

| 必需检查 | 内容 |
| --- | --- |
| `Repository policy` | 策略脚本行为测试、已跟踪路径检查，PR 中检查指导文件变更 |
| `Rust` | 固定 Rust 工具链、rustfmt 检查、锁定依赖的全部 target 测试 |
| `Lean` | 固定 Elan 与现有 Lean／Mathlib 版本、获取缓存、构建证明库 |

工作流以只读仓库权限运行，测试使用本地模拟和合成凭据。构建与测试失败反馈到同一 PR；后续提交再次检查全部必需项目。

服务器 main 保护定义保存在 `.github/branch-protection.json`，包含：

- 三项必需检查通过，分支基于最新 main。
- PR 作为统一集成入口，独立 AI 审阅证据由维护者确认；服务器使用零个其他账户审批的单账号模式。
- 讨论全部解决，提交历史线性。
- 管理员同样遵守保护，main 的强制推送及删除被阻止。

仓库仅启用 squash merge。仓库设置定义保存在 `.github/repository-settings.json`。有管理权限的维护者可重新应用：

```bash
gh api --method PATCH repos/RC-diamond-GH/layer-cake-experiment --input .github/repository-settings.json
gh api --method PUT repos/RC-diamond-GH/layer-cake-experiment/branches/main/protection --input .github/branch-protection.json
```

## 5 文件与凭据保护

`.env`、`.env.*`、运行数据、构建产物、临时文档及仓库内 `.worktrees/` 由忽略规则与路径策略共同排除。策略检查文件名与 Git 元数据，凭据文件内容保持私有。GitHub secret scanning 与 push protection 检查其支持的密钥模式。

```bash
python3 scripts/check_repository.py
python3 scripts/check_repository.py --staged
```

暂存区和 PR 的变更检查保护 `AGENTS.md`。该文件的现有内容保存在基线中；后续变更遵守其中的只读约束。依赖锁文件和固定工具链进入版本控制。实验输入与真实模型返回可能含敏感材料，运行产物保留在被忽略的 `runs/`。

## 6 验证与恢复

本地验证命令以 CI 工作流为准。路径策略及 hooks 的测试使用临时仓库与合成文件，真实 `.env` 保持私有。

Git 保留每次提交、PR 和基线标签。恢复已合并的变更使用独立回退 PR，保留问题、验证及审阅记录。main 的历史连续性由线性历史和强制推送保护维护。工作树中的未提交内容由开发者所有；整理、切换或清理前先确认归属与保存位置。
