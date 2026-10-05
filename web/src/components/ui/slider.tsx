import type { ComponentProps } from "react"
import { cn } from "@/lib/utils"

/**
 * Single-value slider on a native range input: keyboard (arrows, Home/End,
 * Page keys), screen readers and touch come from the platform, which is why
 * it replaced the Base UI slider (about 7 KB gzip for the same behaviour here).
 * Takes a plain number and names the control via `label`.
 */
function Slider({
  className,
  value,
  min = 0,
  max = 100,
  step = 1,
  label,
  disabled,
  onValueChange,
}: Pick<ComponentProps<"input">, "className" | "min" | "max" | "step" | "disabled"> & {
  value: number
  label: string
  onValueChange: (value: number) => void
}) {
  const span = Number(max) - Number(min)
  const pct = span > 0 ? ((value - Number(min)) / span) * 100 : 0
  return (
    <input
      type="range"
      data-slot="slider"
      aria-label={label}
      min={min}
      max={max}
      step={step}
      value={value}
      disabled={disabled}
      onChange={(e) => onValueChange(Number(e.target.value))}
      style={{ "--fill": `${pct}%` } as React.CSSProperties}
      className={cn("hoglet-range h-6 w-full", className)}
    />
  )
}

export { Slider }
