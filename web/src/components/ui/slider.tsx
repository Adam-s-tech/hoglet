import { Slider as SliderPrimitive } from "@base-ui/react/slider"
import { cn } from "@/lib/utils"

/**
 * Single-value slider. Differs from stock shadcn in two ways: it takes a plain
 * number (not an array), and it names the thumb input via `label` (Base UI
 * does not forward aria-label from the root to the thumb).
 */
function Slider({
  className,
  value,
  min = 0,
  max = 100,
  step = 1,
  label,
  onValueChange,
  ...props
}: Omit<SliderPrimitive.Root.Props, "value" | "defaultValue" | "onValueChange" | "aria-label"> & {
  value: number
  label: string
  onValueChange: (value: number) => void
}) {
  return (
    <SliderPrimitive.Root
      className={cn("data-horizontal:w-full", className)}
      data-slot="slider"
      value={value}
      min={min}
      max={max}
      step={step}
      thumbAlignment="edge"
      onValueChange={(v) => {
        const n = Array.isArray(v) ? v[0] : v
        if (typeof n === "number") onValueChange(n)
      }}
      {...props}
    >
      <SliderPrimitive.Control className="relative flex h-6 w-full touch-none items-center select-none data-disabled:opacity-50">
        <SliderPrimitive.Track
          data-slot="slider-track"
          className="relative h-1.5 w-full grow overflow-hidden rounded-full bg-muted select-none"
        >
          <SliderPrimitive.Indicator
            data-slot="slider-range"
            className="h-full bg-primary select-none"
          />
        </SliderPrimitive.Track>
        <SliderPrimitive.Thumb
          data-slot="slider-thumb"
          getAriaLabel={() => label}
          className="relative block size-4 shrink-0 rounded-full border border-ring bg-background ring-ring/50 transition-[color,box-shadow] select-none after:absolute after:-inset-2 hover:ring-3 has-focus-visible:ring-3 active:ring-3 data-disabled:pointer-events-none"
        />
      </SliderPrimitive.Control>
    </SliderPrimitive.Root>
  )
}

export { Slider }
