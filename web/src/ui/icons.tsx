// A small stroke icon set (16px grid, 1.6 stroke). Inline SVG, no font.

import type { SVGProps } from "react";

const paths: Record<string, string> = {
  home: "M2.5 7.2 8 2.8l5.5 4.4V13a.7.7 0 0 1-.7.7H9.6V10H6.4v3.7H3.2a.7.7 0 0 1-.7-.7z",
  globe: "M8 1.8a6.2 6.2 0 1 0 0 12.4A6.2 6.2 0 0 0 8 1.8zM1.8 8h12.4M8 1.8c1.7 1.8 2.5 3.8 2.5 6.2S9.7 12.4 8 14.2C6.3 12.4 5.5 10.4 5.5 8S6.3 3.6 8 1.8z",
  trends: "M2 12.5 6 8l2.8 2.6L14 4.5M10.5 4.5H14V8",
  activity: "M1.8 8h2.6l1.8-4.6 3.2 9.2 1.9-4.6h2.9",
  users: "M6 7.2a2.4 2.4 0 1 0 0-4.8 2.4 2.4 0 0 0 0 4.8zM1.8 13.5c.4-2.4 2.1-3.8 4.2-3.8s3.8 1.4 4.2 3.8M10.8 2.6a2.3 2.3 0 0 1 0 4.4M12 9.9c1.3.4 2.1 1.6 2.3 3.6",
  flag: "M3.2 14.2V2.4M3.2 2.8h8.6l-1.8 3 1.8 3H3.2",
  dashboard: "M2.4 2.4h4.8v6H2.4zM8.8 2.4h4.8v3.4H8.8zM8.8 7.4h4.8v6.2H8.8zM2.4 10h4.8v3.6H2.4z",
  settings:
    "M8 10.2a2.2 2.2 0 1 0 0-4.4 2.2 2.2 0 0 0 0 4.4zM13 9.6l1.2.9-1.2 2.1-1.4-.5a4.8 4.8 0 0 1-1.3.8L10 14.3H7.6l-.3-1.4a4.8 4.8 0 0 1-1.3-.8l-1.4.5-1.2-2.1 1.1-.9a4.8 4.8 0 0 1 0-1.6l-1.1-.9 1.2-2.1 1.4.5c.4-.3.8-.6 1.3-.8L7.6 1.7H10l.3 1.4c.5.2.9.5 1.3.8l1.4-.5 1.2 2.1-1.2.9c.1.5.1 1.1 0 1.6z",
  search: "M7 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10zM10.6 10.6 14 14",
  plus: "M8 3v10M3 8h10",
  x: "M4 4l8 8M12 4l-8 8",
  chevronDown: "M4 6l4 4 4-4",
  chevronRight: "M6 4l4 4-4 4",
  chevronLeft: "M10 4 6 8l4 4",
  chevronUpDown: "M5 6l3-3 3 3M5 10l3 3 3-3",
  check: "M3 8.5 6.5 12 13 4.5",
  copy: "M5.5 5.5V3.2c0-.4.3-.7.7-.7h6.6c.4 0 .7.3.7.7v6.6c0 .4-.3.7-.7.7h-2.3M3.2 5.5h6.6c.4 0 .7.3.7.7v6.6c0 .4-.3.7-.7.7H3.2a.7.7 0 0 1-.7-.7V6.2c0-.4.3-.7.7-.7z",
  trash: "M2.8 4.2h10.4M6.2 4.2V2.6h3.6v1.6M4.2 4.2l.6 9.2h6.4l.6-9.2M6.8 6.8v4.2M9.2 6.8v4.2",
  edit: "M10.6 2.6l2.8 2.8-7.8 7.8H2.8v-2.8z",
  more: "M3.5 8h.01M8 8h.01M12.5 8h.01",
  play: "M4.5 2.8v10.4L13 8z",
  refresh: "M13.2 6.5A5.3 5.3 0 0 0 3.3 5M2.8 9.5a5.3 5.3 0 0 0 9.9 1.5M3 2.5V5h2.5M13 13.5V11h-2.5",
  calendar: "M2.6 3.6h10.8v9.8H2.6zM2.6 6.6h10.8M5.4 2v2.6M10.6 2v2.6",
  filter: "M2.2 3h11.6L9.4 8.4v4.4l-2.8 1.4V8.4z",
  external: "M9.5 2.5h4v4M13.5 2.5 7.5 8.5M11.5 9.5v3.2c0 .4-.3.8-.8.8H3.3a.8.8 0 0 1-.8-.8V5.3c0-.5.4-.8.8-.8h3.2",
  logout: "M6 13.5H3.2a.7.7 0 0 1-.7-.7V3.2c0-.4.3-.7.7-.7H6M10.5 11l3-3-3-3M13.5 8H6",
  sun: "M8 10.8a2.8 2.8 0 1 0 0-5.6 2.8 2.8 0 0 0 0 5.6zM8 1.5v1.3M8 13.2v1.3M1.5 8h1.3M13.2 8h1.3M3.4 3.4l.9.9M11.7 11.7l.9.9M3.4 12.6l.9-.9M11.7 4.3l.9-.9",
  moon: "M13.3 9.6A5.6 5.6 0 0 1 6.4 2.7a5.6 5.6 0 1 0 6.9 6.9z",
  monitor: "M2 3h12v8H2zM5.5 14h5M8 11v3",
  key: "M10.4 9.2a3.4 3.4 0 1 0-3.2-2.3L2.4 11.7v2h2v-1.4h1.4v-1.4h1.4l1-1a3.4 3.4 0 0 0 2.2-.7zM11 5.2h.01",
  info: "M8 14.2A6.2 6.2 0 1 0 8 1.8a6.2 6.2 0 0 0 0 12.4zM8 7.4v3.6M8 5h.01",
  alert: "M8 2.2 1.6 13.4h12.8zM8 6.4v3.2M8 11.6h.01",
  funnel: "M2 3h12M3.8 6.6h8.4M5.6 10.2h4.8M7.2 13.6h1.6",
  retention: "M2.4 2.4h3.2v3.2H2.4zM6.4 2.4h3.2v3.2H6.4zM10.4 2.4h3.2v3.2h-3.2zM2.4 6.4h3.2v3.2H2.4zM6.4 6.4h3.2v3.2H6.4zM2.4 10.4h3.2v3.2H2.4z",
  lifecycle: "M3 8V3.5M6.3 8V5M9.7 8V2.5M13 8V5.5M3 9v2.5M6.3 9v4M9.7 9v1.8M13 9v3",
  stickiness: "M2.5 13.5h11M3.5 13.5V5M6.5 13.5V8M9.5 13.5v-3M12.5 13.5V12",
  paths: "M2 4h3c3 0 3 8 6 8h3M2 12h3c1.2 0 2-1.3 2.6-2.8M8.4 6.8C9 5.3 9.8 4 11 4h3",
  sql: "M5.5 4.5 2 8l3.5 3.5M10.5 4.5 14 8l-3.5 3.5M9.2 2.8 6.8 13.2",
  menu: "M2.5 4h11M2.5 8h11M2.5 12h11",
  arrowUp: "M8 13V3M4 7l4-4 4 4",
  arrowDown: "M8 3v10M4 9l4 4 4-4",
  arrowRight: "M3 8h10M9 4l4 4-4 4",
  share: "M10.8 5.2 5.2 7.4M5.2 8.6l5.6 2.2M12 5.6a1.8 1.8 0 1 0 0-3.6 1.8 1.8 0 0 0 0 3.6zM4 9.8a1.8 1.8 0 1 0 0-3.6 1.8 1.8 0 0 0 0 3.6zM12 14a1.8 1.8 0 1 0 0-3.6 1.8 1.8 0 0 0 0 3.6z",
  save: "M3 2.5h8.2l2.3 2.3v8.4c0 .2-.1.3-.3.3H2.8a.3.3 0 0 1-.3-.3V2.8c0-.2.1-.3.3-.3zM5 2.5v3.3h5V2.5M4.6 13.5V9.2h6.8v4.3",
  pause: "M5.2 3v10M10.8 3v10",
  clock: "M8 14.2A6.2 6.2 0 1 0 8 1.8a6.2 6.2 0 0 0 0 12.4zM8 4.6V8l2.4 1.6",
  bolt: "M9 1.8 3.4 9h4.2L7 14.2 12.6 7H8.4z",
  table: "M2.4 3h11.2v10H2.4zM2.4 6.4h11.2M2.4 9.8h11.2M6.2 3v10",
  pie: "M8 1.8v6.2h6.2A6.2 6.2 0 1 1 8 1.8zM10 1.8a4.6 4.6 0 0 1 4.2 4.2H10z",
  bar: "M3 13V8M6.3 13V4M9.7 13V6.5M13 13V2.5",
  area: "M2 13.5V9l3.5-3 3 2.5L14 3.5v10z",
  hash: "M3 6h10.5M2.5 10H13M6.5 2.5 5.5 13.5M10.5 2.5 9.5 13.5",
  grip: "M6 4h.01M10 4h.01M6 8h.01M10 8h.01M6 12h.01M10 12h.01",
  eye: "M1.6 8S4 3.6 8 3.6 14.4 8 14.4 8 12 12.4 8 12.4 1.6 8 1.6 8zM8 10a2 2 0 1 0 0-4 2 2 0 0 0 0 4z",
  person: "M8 7.4a2.6 2.6 0 1 0 0-5.2 2.6 2.6 0 0 0 0 5.2zM3 14c.5-2.8 2.4-4.4 5-4.4s4.5 1.6 5 4.4",
  terminal: "M2.5 3h11v10h-11zM4.8 6.2 6.8 8l-2 1.8M8.4 10h3",
  folder: "M2.4 4c0-.4.3-.7.7-.7h3.1l1.3 1.5h5.4c.4 0 .7.3.7.7v6.8c0 .4-.3.7-.7.7H3.1a.7.7 0 0 1-.7-.7z",
  sparkle: "M8 2.2l1.3 3.5L12.8 7l-3.5 1.3L8 11.8 6.7 8.3 3.2 7l3.5-1.3z",
};

export type IconName = keyof typeof paths;

interface IconProps extends SVGProps<SVGSVGElement> {
  name: IconName;
  size?: number;
}

export function Icon({ name, size = 16, strokeWidth = 1.6, ...rest }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...rest}
    >
      <path d={paths[name]} />
    </svg>
  );
}

const QUILLS =
  "M2 24.5 L3.9 23.7 L1.9 22.5 L4.2 22.1 L2.4 20.5 L4.8 20.5 L3.2 18.7 L5.6 19.1 L4.4 17 L6.6 17.8 L5.8 15.6 L7.9 16.7 L7.4 14.4 L9.3 15.9 L9.3 13.5 L10.8 15.3 L11.2 12.9 L12.4 15 L13.2 12.7 L14.1 14.9 L15.2 12.8 L15.7 15.2 L17.2 13.3 L17.3 15.7 L19.1 14.1 L18.7 16.5 L20.8 15.2 L21.5 24.5 Z";
const FACE = "M18.2 16.6 C21.6 14.6 25.4 16.2 27.4 18.9 L30.2 20.3 C30.9 20.7 30.8 21.8 30 22 L25.5 23.3 C23.2 24.3 20.6 24.6 18.2 24.5 Z";

/** The hedgehog mark: a round back of quills, a soft face, one bright eye. */
export function Logo({ size = 24 }: { size?: number }) {
  const h = size * 0.75;
  return (
    <svg width={h * 1.88} height={h} viewBox="1 10.5 31 16.5" aria-label="Hoglet" role="img" style={{ flex: "none" }}>
      <path d={QUILLS} fill="var(--accent)" stroke="var(--accent)" strokeWidth="0.8" strokeLinejoin="round" />
      <path d={FACE} fill="#e3bf98" />
      <circle cx="30" cy="20.9" r="1.05" fill="#1b1a17" />
      <circle cx="24.2" cy="19.2" r="1.05" fill="#1b1a17" />
      <path d="M7 24.6v1.4M12 24.6v1.4M20.5 24.6v1.4" stroke="var(--accent)" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}
