module

public import AgentTheory.Basic

@[expose] public section

namespace AgentTheory.MVP

/-- 单个订阅的投递闸门。自然数仅表示事件身份；对应 Rust 字符串 ID 时，
要求编码保持并反映相等。`seen` 表示已经获准的事件，而非已被接收者取出的事件。 -/
structure DeliveryGate where
  active : Bool
  seen : Finset Nat
  deriving DecidableEq

namespace DeliveryGate

/-- 新订阅处于活动状态，尚未准入任何事件。 -/
def initial : DeliveryGate := ⟨true, ∅⟩

/-- 与 Rust `admit` 相同的三个分支：停用、重复、新事件。
返回值依次为下一状态与是否准入；拒绝时保留整个状态。 -/
def admit (gate : DeliveryGate) (eventId : Nat) : DeliveryGate × Bool :=
  if gate.active = false then
    (gate, false)
  else if eventId ∈ gate.seen then
    (gate, false)
  else
    ({ gate with seen := insert eventId gate.seen }, true)

/-- 取消仅关闭闸门，不清除已准入事件的集合。队列清理属于 EventHub。 -/
def cancel (gate : DeliveryGate) : DeliveryGate :=
  { gate with active := false }

/-- 停用闸门拒绝事件，并且完全保留状态。 -/
theorem admit_of_inactive (gate : DeliveryGate) (eventId : Nat)
    (inactive : gate.active = false) :
    gate.admit eventId = (gate, false) := by
  simp [admit, inactive]

/-- 已见事件无论当前是否活动都被拒绝，且不修改状态。 -/
theorem admit_of_seen (gate : DeliveryGate) (eventId : Nat)
    (seen : eventId ∈ gate.seen) :
    gate.admit eventId = (gate, false) := by
  simp [admit, seen]

/-- 活动闸门恰好插入新事件，并报告准入成功。 -/
theorem admit_of_fresh_active (gate : DeliveryGate) (eventId : Nat)
    (active : gate.active = true) (fresh : eventId ∉ gate.seen) :
    gate.admit eventId = ({ gate with seen := insert eventId gate.seen }, true) := by
  simp [admit, active, fresh]

/-- 准入操作不改变活动标志，因此它不能重新激活已取消的订阅。 -/
theorem admit_active_preserved (gate : DeliveryGate) (eventId : Nat) :
    (gate.admit eventId).1.active = gate.active := by
  by_cases inactive : gate.active = false
  · simp [admit, inactive]
  · by_cases seen : eventId ∈ gate.seen <;> simp [admit, inactive, seen]

/-- 任意准入尝试都保留所有旧的已见事件。 -/
theorem admit_seen_monotone (gate : DeliveryGate) (eventId : Nat) :
    gate.seen ⊆ (gate.admit eventId).1.seen := by
  intro oldId oldSeen
  by_cases inactive : gate.active = false
  · simpa [admit, inactive] using oldSeen
  · by_cases seen : eventId ∈ gate.seen
    · simpa [admit, inactive, seen] using oldSeen
    · simp [admit, inactive, seen, oldSeen]

/-- 成功准入的事件被记录于下一状态，以便以后去重。 -/
theorem admitted_event_seen (gate : DeliveryGate) (eventId : Nat)
    (admitted : (gate.admit eventId).2 = true) :
    eventId ∈ (gate.admit eventId).1.seen := by
  by_cases inactive : gate.active = false
  · simp [admit, inactive] at admitted
  · by_cases seen : eventId ∈ gate.seen
    · simp [admit, inactive, seen] at admitted
    · simp [admit, inactive, seen]

/-- 对同一事件再试一次必定拒绝，并保留第一次尝试后的完整状态。
即便第一次已被拒绝，这个性质仍然成立。 -/
theorem admit_duplicate (gate : DeliveryGate) (eventId : Nat) :
    (gate.admit eventId).1.admit eventId = ((gate.admit eventId).1, false) := by
  by_cases inactive : gate.active = false
  · simp [admit, inactive]
  · by_cases seen : eventId ∈ gate.seen <;> simp [admit, inactive, seen]

/-- 重复尝试的返回布尔值为 false；这不声称第一次也返回 false。 -/
theorem duplicate_suppressed (gate : DeliveryGate) (eventId : Nat) :
    ((gate.admit eventId).1.admit eventId).2 = false := by
  exact congrArg Prod.snd (admit_duplicate gate eventId)

/-- 幂等的是下一状态，而不是整个包含准入布尔值的返回对。 -/
theorem admit_next_idempotent (gate : DeliveryGate) (eventId : Nat) :
    ((gate.admit eventId).1.admit eventId).1 = (gate.admit eventId).1 := by
  simpa using congrArg (fun result : DeliveryGate × Bool => result.1)
    (admit_duplicate gate eventId)

/-- 取消之后的任何准入尝试都被吸收，既不投递也不改变状态。 -/
theorem cancel_absorbs_admit (gate : DeliveryGate) (eventId : Nat) :
    gate.cancel.admit eventId = (gate.cancel, false) := by
  exact admit_of_inactive gate.cancel eventId rfl

/-- 重复取消不产生进一步的状态变化。 -/
theorem cancel_idempotent (gate : DeliveryGate) :
    gate.cancel.cancel = gate.cancel := by
  rfl

/-- 取消保留完整已见集合。 -/
theorem cancel_seen_preserved (gate : DeliveryGate) :
    gate.cancel.seen = gate.seen := by
  rfl

end DeliveryGate
end AgentTheory.MVP
