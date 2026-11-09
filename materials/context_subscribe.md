# LLM 上下文订阅

Agent 的工作过程中，通常需要持续观测一个可变对象。

一个写代码的 Agent，需要持续观测它正在工作的代码文件

一个办公通用 Agent，需要持续观测当前时间

一个由状态机驱动的 Agent，需要知道自己处于什么状态

那么，我们通常是如何管理这种持续观测的对象的？

对于写代码的 Agent，它直接调用 read 工具来获取代码文件的内容，然而，这就会出现这样的问题，以下是一个示例的 Agent 工作过程：

```
assistant: 用户现在让我修改 main.rs 中的函数

tool_use: read (main.rs, at 1, max 200)

tool_use_result: rust code

assistant: 好的, 我注意到 `main` 函数存在一处设计不当

tool_use: write (main.rs, 3, rust code)

tool_use_result: successful

tool_use: execute (cargo test)

tool_use_result: test result, failed.

assistant: 测试失败了，我需要检查一下代码的状态

tool_use: read (main.rs, at 1, max 200)

tool_use_result: rust code

assistant: 之前调用 write 工具把代码改坏了，让我重新写一下

tool_use: write (main.rs, 3, rust code)

tool_use_result: successful

assistant: 让我检查一下代码

tool_use: read (main.rs, at 1, max 200)

tool_use_result: rust code

assistant: 我想代码已经没问题了

tool_use: execute (cargo test)

tool_use_result: test result, successful.

assistant: 测试成功了！让我向用户进行汇报。
```

可以看到，Agent 每次想要确定 main.rs 的状态，都必须调用一次 `read`，而且，对于整个上下文而言，它每次调用 `read`，以及 `read` 的返回，都成为了噪音。

再来看第二个场景：需要持续观测当前时间，这里的关键在于，“时间”放在哪里？如果放在系统提示词里，那么几乎每一次发起 API 请求，请求的前缀都会不同，这意味着**K-V Cache**几乎失效。对于第三个场景下的状态机而言，如果状态字段放在系统提示词里，虽然不至于像时间那样几乎每次请求都导致K-V Cache失效，但每次状态转移后，依然会发生。

然而，第二、第三种场景虽然会导致 K-V Cache 的失效，它们本身实际上已经具备了“上下文订阅”的雏形——上下文中的一部分内容会自动更新。

而为了拯救 K-V Cache 我认为应当将上下文订阅放在上下文的末尾，一个上下文的结构通常是这样的：


```
system_prompt
user_1
assistant_1
tool_use_1
tool_use_result_1
tool_use_2
tool_use_result_2
assistant_2
```

通常，在上下文以 `user` 或 `tool_use_result` 结尾时，软件会用这个上下文发起一次新的 LLM API 请求，而我们要做的，其实就是在发起请求之前，在上下文里加一个临时提示词，这个临时提示词里包含该上下文所订阅的内容，如当前时间、main.rs、当前状态等。

一次发送时的上下文结构可能是这样：

```
system_prompt
user_1
assistant_1
tool_use_1
tool_use_result_1
tool_use_2
tool_use_result_2
assistant_2
tool_use_3
tool_use_result_3
context_subscribe
```

假设模型还会发起下一次 tool use，那么下一次发送的结构就必然是：

```
system_prompt
user_1
assistant_1
tool_use_1
tool_use_result_1
tool_use_2
tool_use_result_2
assistant_2
tool_use_3
tool_use_result_3
tool_use_4
tool_use_result_4
context_subscribe
```