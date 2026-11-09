module

public import Mathlib

@[expose] public section

namespace AgentTheory

/-- 生成完成后的格局。退出时 `generation` 保留最后一次生成结果；
此时 `prompt` 是 δ 返回的提示词，并不表示又进行了一次生成。 -/
structure Configuration (Prompt State Generation : Type) where
  prompt : Prompt
  state : State
  generation : Generation
  deriving Repr, DecidableEq

/-- 原始五元组的可执行版本。
`Sample` 是显式的抽样输入；尚未为它指定分布，因此此处不作概率断言。
工具和环境的交互暂不建模，`transition` 是纯函数。 -/
structure Automaton (Prompt State Generation Sample : Type) where
  model : Prompt → Sample → Generation
  initialState : State
  exitState : State
  transition : Prompt → Generation → State → Prompt × State

namespace Automaton

variable {Prompt State Generation Sample : Type}

/-- 对应“接收初始 Prompt，然后生成 G”。即使初态就是退出态，也生成一次；
随后运行不再转移或生成。 -/
def initialConfig (machine : Automaton Prompt State Generation Sample)
    (prompt : Prompt) (sample : Sample) : Configuration Prompt State Generation :=
  ⟨prompt, machine.initialState, machine.model prompt sample⟩

/-- 一个控制步：先计算 δ；仅在新状态不是退出态时生成下一次 G。
退出格局是吸收态，额外的运行预算不会产生新的动作。 -/
def step [DecidableEq State] (machine : Automaton Prompt State Generation Sample)
    (config : Configuration Prompt State Generation) (sample : Sample) :
    Configuration Prompt State Generation :=
  if config.state = machine.exitState then
    config
  else
    let next := machine.transition config.prompt config.generation config.state
    if next.2 = machine.exitState then
      ⟨next.1, next.2, config.generation⟩
    else
      ⟨next.1, next.2, machine.model next.1 sample⟩

/-- 运行给定数量的控制步，而不假设机器最终会终止。
`sampleAt n` 是第 n 个控制步可用的抽样输入，不是第 n 次模型调用；
退出步和退出后的步不会使用它进行生成。初始化的抽样输入另行提供。 -/
def run [DecidableEq State] (machine : Automaton Prompt State Generation Sample)
    (config : Configuration Prompt State Generation) (sampleAt : Nat → Sample) :
    Nat → Configuration Prompt State Generation
  | 0 => config
  | n + 1 => machine.step (machine.run config sampleAt n) (sampleAt n)

/-- 在给定的抽样输入序列下，存在有限的退出时刻。 -/
def Terminates [DecidableEq State] (machine : Automaton Prompt State Generation Sample)
    (config : Configuration Prompt State Generation) (sampleAt : Nat → Sample) : Prop :=
  ∃ n, (machine.run config sampleAt n).state = machine.exitState

/-- 已退出的格局不会再变化。 -/
theorem step_of_exit [DecidableEq State]
    (machine : Automaton Prompt State Generation Sample)
    (config : Configuration Prompt State Generation) (sample : Sample)
    (h : config.state = machine.exitState) :
    machine.step config sample = config := by
  simp [step, h]

/-- 一旦退出，任意额外预算都保留完整格局，包括最后的生成结果。 -/
theorem run_of_exit [DecidableEq State]
    (machine : Automaton Prompt State Generation Sample)
    (config : Configuration Prompt State Generation) (sampleAt : Nat → Sample)
    (h : config.state = machine.exitState) (n : Nat) :
    machine.run config sampleAt n = config := by
  induction n with
  | zero => rfl
  | succ n ih =>
    simp [run, ih, step_of_exit machine config (sampleAt n) h]

end Automaton
end AgentTheory
