// Small composed controls on shadcn/Base UI primitives.

import type { ReactNode } from "react";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "@/lib/utils";

/** A segmented single-choice control (interval, chart type). Always has a value. */
export function Seg<T extends string>({
  value,
  options,
  onChange,
  label,
  className,
}: {
  value: T;
  options: { value: T; label: ReactNode; title?: string }[];
  onChange: (v: T) => void;
  label: string;
  className?: string;
}) {
  return (
    <ToggleGroup
      aria-label={label}
      value={[value]}
      onValueChange={(v) => {
        const next = v[0] as T | undefined;
        if (next !== undefined) onChange(next);
      }}
      variant="outline"
      size="sm"
      spacing={0}
      className={className}
    >
      {options.map((o) => (
        <ToggleGroupItem key={o.value} value={o.value} title={o.title}>
          {o.label}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}

/** Underlined tab strip. The panel is yours: render it under the bar keyed on `value`. */
export function TabsBar<T extends string>({
  value,
  options,
  onChange,
  className,
}: {
  value: T;
  options: { value: T; label: ReactNode }[];
  onChange: (v: T) => void;
  className?: string;
}) {
  return (
    <Tabs value={value} onValueChange={(v) => onChange(v as T)} className={cn("mb-4", className)}>
      <TabsList variant="line" className="h-9 w-full justify-start gap-1 overflow-x-auto border-b">
        {options.map((o) => (
          <TabsTrigger key={o.value} value={o.value} className="flex-none px-3">
            {o.label}
          </TabsTrigger>
        ))}
      </TabsList>
    </Tabs>
  );
}
