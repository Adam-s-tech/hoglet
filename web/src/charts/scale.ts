// Scales, ticks and the categorical palette, shared by every chart.

export const SERIES_COUNT = 8;

/**
 * Categorical slot for a series identity. The validated palette has eight
 * slots, assigned in fixed order and never cycled: a ninth series and beyond
 * render in neutral ink (the legend still names them).
 */
export function seriesColor(index: number): string {
  return index >= 0 && index < SERIES_COUNT ? `var(--s${index + 1})` : "var(--muted-foreground)";
}

export function niceStep(range: number, count: number): number {
  if (range <= 0 || !Number.isFinite(range)) return 1;
  const raw = range / Math.max(1, count);
  const mag = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / mag;
  const step = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 2.5 ? 2.5 : norm <= 5 ? 5 : 10;
  return step * mag;
}

/** Round domain and ticks that include zero. */
export function niceDomain(min: number, max: number, count = 5): { lo: number; hi: number; ticks: number[] } {
  let lo = Math.min(0, min);
  let hi = Math.max(0, max);
  if (lo === hi) hi = lo + 1;
  const step = niceStep(hi - lo, count);
  lo = Math.floor(lo / step) * step;
  hi = Math.ceil(hi / step) * step;
  const ticks: number[] = [];
  for (let v = lo; v <= hi + step / 2; v += step) ticks.push(Math.round(v / step) * step);
  return { lo, hi, ticks };
}

export function linear(d0: number, d1: number, r0: number, r1: number): (v: number) => number {
  const span = d1 - d0 || 1;
  return (v) => r0 + ((v - d0) / span) * (r1 - r0);
}

/** Pick every k-th label so labels never collide. */
export function labelStride(count: number, width: number, minGap = 64): number {
  if (count <= 1) return 1;
  return Math.max(1, Math.ceil(count / Math.max(1, Math.floor(width / minGap))));
}

export function textWidth(text: string, px = 11): number {
  return text.length * px * 0.58;
}

/** Path for a bar with 4px rounded data-end and a square baseline end. */
export function barPath(x: number, y0: number, w: number, y1: number, radius = 4): string {
  const h = Math.abs(y1 - y0);
  if (h < 0.5 || w <= 0) return "";
  const r = Math.min(radius, w / 2, h);
  if (y1 < y0) {
    // Positive bar: rounded top.
    return `M${x},${y0}V${y1 + r}Q${x},${y1} ${x + r},${y1}H${x + w - r}Q${x + w},${y1} ${x + w},${y1 + r}V${y0}Z`;
  }
  return `M${x},${y0}V${y1 - r}Q${x},${y1} ${x + r},${y1}H${x + w - r}Q${x + w},${y1} ${x + w},${y1 - r}V${y0}Z`;
}

/** Sequential blue ramp step 0..6 for a 0..1 magnitude. */
export function seqColor(t: number): { bg: string; fg: string } {
  const step = Math.max(0, Math.min(6, Math.round(t * 6)));
  return { bg: `var(--seq-${step})`, fg: `var(--seq-fg-${step})` };
}
