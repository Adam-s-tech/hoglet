import { useEffect, useState } from "react";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

/** Click-to-edit text (insight and dashboard names). Enter saves, Esc cancels. */
export function InlineEdit({
  value,
  onSave,
  placeholder,
  className = "",
  ariaLabel,
}: {
  value: string;
  onSave: (v: string) => void;
  placeholder?: string;
  className?: string;
  ariaLabel: string;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  if (!editing) {
    return (
      <button
        type="button"
        className={cn("block max-w-full cursor-text rounded-sm text-left hover:bg-muted/60 focus-visible:outline-2", className)}
        onClick={() => setEditing(true)}
        aria-label={`${ariaLabel}: ${value || placeholder}. Click to rename`}
      >
        <span className="block truncate">{value || <span className="text-muted-foreground">{placeholder}</span>}</span>
      </button>
    );
  }
  const commit = () => {
    setEditing(false);
    const next = draft.trim();
    if (next && next !== value) onSave(next);
    else setDraft(value);
  };
  return (
    <Input
      className={cn("h-9 w-[min(520px,100%)] text-[length:inherit] font-[inherit]", className)}
      autoFocus
      value={draft}
      placeholder={placeholder}
      aria-label={ariaLabel}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") {
          setDraft(value);
          setEditing(false);
        }
      }}
    />
  );
}
