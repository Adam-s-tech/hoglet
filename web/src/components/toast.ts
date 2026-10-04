import { toast as sonner } from "sonner";

/** `toast("Saved")`, `toast("Couldn't save", true)`. Errors linger longer. */
export function toast(text: string, bad = false): void {
  if (bad) sonner.error(text, { duration: 6000 });
  else sonner.success(text, { duration: 3000 });
}
