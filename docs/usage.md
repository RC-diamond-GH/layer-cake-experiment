# MVP 使用指南

## 1 环境与构建

Rust 工程与 Lean 工程共用仓库根目录。Cargo 生成 `layer-cake` 可执行文件；Lake 构建 `AgentTheory` 证明库。

```bash
cargo build
lake build
```

`Cargo.lock` 固定 Rust 依赖；`lean-toolchain`、`lakefile.toml` 和 `lake-manifest.json` 固定 Lean 与 Mathlib。`target/`、`.lake/`、`runs/` 是本地构建或运行产物。`materials/` 保留需求与探索材料，正式文档由 [文档首页](index.md) 索引。

## 2 实验入口与配置

```bash
cargo run -- list
cargo run -- run --config examples/react-memory.json --output runs/memory.json
cargo run -- run --config examples/furnace-polling.json --output runs/polling.json
cargo run -- run --config examples/furnace-reactive.json --output runs/reactive.json
cargo run -- run --config examples/timer-reactive.json --output runs/timer.json
cargo run -- run --config examples/reactive-edges.json --output runs/edges.json
```

每次启动创建独立 UUID。`--output` 选择保存文件；省略时使用 `runs/<UUID>.json`。配置的 `experiment` 选择已注册的 Rust 装配函数；`--experiment` 可以覆盖该项。`max_rounds` 控制内存 ReAct 的生成轮数，`max_boundaries` 控制实验边界数。熔炉与计时器样例使用确定性模拟时钟及脚本策略，便于比较与回放。

真实模型通过同一 `ModelClient` 及记录入口调用。CLI 的 `run` 命令加载本地 `.env`，已有进程环境变量优先。可在 `.env` 中设置：

```dotenv
OPENAI_API_KEY=你的密钥
OPENAI_BASE_URL=https://api.openai.com/v1
OPENAI_MODEL=gpt-4.1-mini
```

```bash
cargo run -- run --config examples/openai.json --output runs/openai.json
```

示例配置从这三个变量取得模型访问方式。`OPENAI_BASE_URL` 是 API 基础地址：程序保留路径前缀并追加 `/chat/completions`，例如 `/v1` 与 `/v1/` 均请求 `/v1/chat/completions`；服务根地址则请求 `/chat/completions`。

显式 `model.endpoint` 和 `model.model` 分别优先于环境变量；`endpoint` 使用完整 Chat Completions URL，`api_key_env` 默认 `OPENAI_API_KEY`。真实模型名称由 `model.model` 或 `OPENAI_MODEL` 确定。解析后的地址与模型名称进入运行配置，回放可直接使用保存配置。

`.env` 已由 `.gitignore` 排除，密钥仅用于 HTTP Authorization。加载失败时错误信息仅提示检查文件权限与语法。HTTP 失败保留状态码，并区分空响应、非 JSON 响应和提供方错误；404 附带接口路径提示。运行文件含完整提示词、工具参数和返回值，应按实验数据的敏感级别管理。

## 3 交互控制

```bash
cargo run -- run --config examples/react-memory.json --interactive --output runs/manual.json
```

控制台命令：

| 命令 | 效果 |
| --- | --- |
| `step` | 执行一个完整边界后暂停 |
| `continue` | 连续执行至结束或暂停 |
| `pause` | 在当前完整边界结束后暂停 |
| `stop` | 保存当前在途调用的实际返回，阻止下一次模型或工具派发 |
| `input 补充说明` | 在边界追加用户输入；也接受 JSON |
| `state`、`objects`、`records` | 查看状态、对象索引、观测 |
| `context [version]` | 查看最新或指定版本上下文 |
| `calls [record-id]` | 查看完整调用，包含模型实际输入与工具来源 |
| `report runs/manual.html` | 生成当前记录截止位置的报告 |
| `save`、`quit` | 保存；协作停止后退出 |

ReAct 的一个边界包含一次生成及其全部工具结果写回。暂停和用户输入在完整轮次边界生效；停止与退出立即锁存停止信号，在当前调用保存返回后阻止下一次派发。`control_requested` 记录请求，`control` 记录确认、生效边界与错误。终态后的继续、步进和输入返回明确错误。`quit` 及输入结束等待在途调用完成。回放按调用记录位置重新施加在途停止信号。

响应式输入支持字符串或 `{"kind":"user","content":"说明"}`、`{"kind":"tick","ticks":2}`、`{"kind":"mutate","raw":1,"fuel":1,"product":2}` 和 `{"kind":"cancel"}`。取消关闭订阅并停止该实验。

## 4 查询与报告

```bash
cargo run -- show runs/memory.json state
cargo run -- show runs/memory.json objects
cargo run -- show runs/memory.json context --version 1
cargo run -- show runs/memory.json calls
cargo run -- show runs/memory.json records --object '<对象 ID>'
cargo run -- show runs/memory.json calls --id '<记录 ID>'
cargo run -- report runs/reactive.json --output runs/reactive.html
cargo run -- compare runs/polling.json runs/reactive.json --output runs/comparison.json
```

上下文查询支持 `--id` 选择上下文身份，`--version` 选择版本。调用记录保存输入、输出、错误、耗时及关联身份；工具调用的 `cause` 指向提出调用的模型记录。模型输入包含完整实际快照和生成参数。HTML 可直接在浏览器打开，提供时间线、对象过滤、上下文和调用详情。报告标记记录截止位置，运行期间和结束后使用同一存储格式。

## 5 确定性回放

```bash
cargo run -- replay runs/memory.json
cargo run -- replay runs/reactive.json
```

回放重建原实验及原运行身份，按原边界重新施加已确认输入和停止操作。模型与工具适配器共享严格调用游标，逐次核对请求后返回已保存的结果或错误；工具适配器替代外部副作用。响应式实验从已保存工具结果投影模拟环境，再由原控制器推进模拟时钟和事件。请求顺序、参数、可见工具、上下文或调用数量发生差异时，回放停止并报告首个分歧位置。暂停中的运行按保存的边界截止位置回放。

## 6 Rust 装配扩展

公共组件由 `layer_cake` 导出。实验作者实现自己的 Rust 装配函数和控制器，在 `experiment::definitions` 与装配分派处登记入口。模型实现 `ModelClient`，工具实现 `Tool`；实验观测对象通过 `RunLog::register` 登记，使用 `observe` 保存自定义内容和来源。模型及工具调用使用公共记录入口，保留跨机制统一的输入与关联格式。新增实验可声明自己的边界和输入；控制台通过控制句柄提交请求。
