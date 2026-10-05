import { cn } from "@/lib/utils";

// Fixed, dark enough that white initials clear 4.5:1 in both themes.
const COLORS = ["#1f66c1", "#a64a1d", "#0e7a54", "#5b4fc4", "#b83a6d", "#0f766e", "#c23b3a", "#8f5a00"];

/** Initials on a colour derived from the id, so a person looks the same everywhere. */
export function Avatar({ name, id, large, className }: { name: string; id: string; large?: boolean; className?: string }) {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  const initials = name
    .replace(/@.*/, "")
    .split(/[ ._-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((s) => s[0]?.toUpperCase() ?? "")
    .join("");
  return (
    <span
      className={cn(
        "inline-grid flex-none place-items-center rounded-full font-semibold text-white",
        large ? "size-[52px] text-lg" : "size-6 text-[10.5px]",
        className,
      )}
      style={{ background: COLORS[Math.abs(h) % COLORS.length] }}
      aria-hidden="true"
    >
      {initials || "?"}
    </span>
  );
}
