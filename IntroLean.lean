module

public import Mathlib
public import IntroLean.Examples

public section

namespace IntroLean

-- 使用 Mathlib 的 ring 策略证明多项式恒等式。
theorem square_expansion (a b : ℝ) :
    (a + b) ^ 2 = a ^ 2 + 2 * a * b + b ^ 2 := by
  ring

-- 使用 Mathlib 的 linarith 策略处理线性不等式。
theorem add_one_pos (x : ℝ) (hx : 0 ≤ x) : 0 < x + 1 := by
  linarith

end IntroLean
