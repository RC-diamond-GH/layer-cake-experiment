module

public import AgentTheory.Basic

@[expose] public section

namespace AgentTheory.Examples

/-- 退出态独立于第二个任务，避免在执行最后一个任务之前就退出。 -/
inductive WorkflowState where
  | first
  | second
  | exit
  deriving Repr, DecidableEq

/-- 用自然数代替文本，隔离并检验控制流。模型每次生成 prompt + 1。
两个任务都将生成结果传入下一份提示词；第二个任务随后退出。 -/
def workflow : Automaton Nat WorkflowState Nat Unit where
  model prompt _ := prompt + 1
  initialState := .first
  exitState := .exit
  transition _ generation state :=
    match state with
    | .first => (generation, .second)
    | .second => (generation, .exit)
    | .exit => (generation, .exit)

def workflowStart : Configuration Nat WorkflowState Nat :=
  workflow.initialConfig 0 ()

def workflowResult : Configuration Nat WorkflowState Nat :=
  ⟨2, .exit, 2⟩

/-- 第一个任务结束后，第二个任务已生成结果，但尚未执行它的 δ。 -/
theorem workflow_after_one :
    workflow.run workflowStart (fun _ => ()) 1 = ⟨1, .second, 2⟩ := by
  rfl

/-- 两个任务完成后退出，最后结果为 2，而不是退出时再次生成的 3。 -/
theorem workflow_after_two :
    workflow.run workflowStart (fun _ => ()) 2 = workflowResult := by
  rfl

theorem workflow_terminates :
    workflow.Terminates workflowStart (fun _ => ()) := by
  exact ⟨2, rfl⟩

/-- 加大预算不会重复执行已经完成的 Workflow。 -/
theorem workflow_stays_finished (n : Nat) :
    workflow.run workflowStart (fun _ => ()) (n + 2) = workflowResult := by
  induction n with
  | zero => exact workflow_after_two
  | succ n ih =>
    change workflow.step (workflow.run workflowStart (fun _ => ()) (n + 2)) () =
      workflowResult
    rw [ih]
    rfl

inductive ReActState where
  | reasoning
  | exit
  deriving Repr, DecidableEq

/-- ReAct 控制形状的自循环实例：将 G 作为下一轮 Prompt，永不请求退出。
这里没有模拟工具或真实 LLM；它是“有限控制状态不保证终止”的反例。 -/
def loopingReAct : Automaton Nat ReActState Nat Unit where
  model prompt _ := prompt + 1
  initialState := .reasoning
  exitState := .exit
  transition _ generation state := (generation, state)

def loopingStart : Configuration Nat ReActState Nat :=
  loopingReAct.initialConfig 0 ()

/-- 每一步都更新记忆和生成结果，控制状态却始终相同。 -/
theorem looping_run (n : Nat) :
    loopingReAct.run loopingStart (fun _ => ()) n = ⟨n, .reasoning, n + 1⟩ := by
  induction n with
  | zero => rfl
  | succ n ih =>
    rw [Automaton.run, ih]
    rfl

theorem looping_does_not_terminate :
    ¬ loopingReAct.Terminates loopingStart (fun _ => ()) := by
  intro h
  obtain ⟨n, hn⟩ := h
  rw [looping_run] at hn
  change ReActState.reasoning = ReActState.exit at hn
  cases hn

end AgentTheory.Examples
