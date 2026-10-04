// One icon language: lucide, imported per icon (tree-shaken), addressed by a
// short semantic name so call sites stay readable. Plus the Hoglet mark.

import {
  Activity,
  ArrowDown,
  ArrowRight,
  ArrowUp,
  Calendar,
  ChartArea,
  ChartColumn,
  ChartColumnStacked,
  ChartPie,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronsUpDown,
  Clock,
  Code,
  Copy,
  ExternalLink,
  Eye,
  Filter,
  Flag,
  Folder,
  Globe,
  Grid3x3,
  GripVertical,
  Hash,
  House,
  Info,
  KeyRound,
  LayoutDashboard,
  LogOut,
  Menu,
  Monitor,
  Moon,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Save,
  Search,
  Settings,
  Share2,
  Sparkles,
  Sun,
  Table,
  Terminal,
  Trash2,
  TrendingUp,
  TriangleAlert,
  User,
  Users,
  Workflow,
  X,
  Zap,
  Ellipsis,
  type LucideProps,
} from "lucide-react";
import type { ComponentType } from "react";

const icons = {
  home: House,
  globe: Globe,
  trends: TrendingUp,
  activity: Activity,
  users: Users,
  flag: Flag,
  dashboard: LayoutDashboard,
  settings: Settings,
  search: Search,
  plus: Plus,
  x: X,
  chevronDown: ChevronDown,
  chevronRight: ChevronRight,
  chevronLeft: ChevronLeft,
  chevronUpDown: ChevronsUpDown,
  check: Check,
  copy: Copy,
  trash: Trash2,
  edit: Pencil,
  more: Ellipsis,
  play: Play,
  refresh: RefreshCw,
  calendar: Calendar,
  filter: Filter,
  external: ExternalLink,
  logout: LogOut,
  sun: Sun,
  moon: Moon,
  monitor: Monitor,
  key: KeyRound,
  info: Info,
  alert: TriangleAlert,
  funnel: Filter,
  retention: Grid3x3,
  lifecycle: ChartColumnStacked,
  stickiness: ChartColumn,
  paths: Workflow,
  sql: Code,
  menu: Menu,
  arrowUp: ArrowUp,
  arrowDown: ArrowDown,
  arrowRight: ArrowRight,
  share: Share2,
  save: Save,
  pause: Pause,
  clock: Clock,
  bolt: Zap,
  table: Table,
  pie: ChartPie,
  bar: ChartColumn,
  area: ChartArea,
  hash: Hash,
  grip: GripVertical,
  eye: Eye,
  person: User,
  terminal: Terminal,
  folder: Folder,
  sparkle: Sparkles,
} satisfies Record<string, ComponentType<LucideProps>>;

export type IconName = keyof typeof icons;

interface IconProps extends Omit<LucideProps, "ref"> {
  name: IconName;
}

export function Icon({ name, size = 16, strokeWidth = 1.75, ...rest }: IconProps) {
  const Cmp = icons[name];
  return <Cmp size={size} strokeWidth={strokeWidth} aria-hidden="true" {...rest} />;
}

const QUILLS =
  "M2 24.5 L3.9 23.7 L1.9 22.5 L4.2 22.1 L2.4 20.5 L4.8 20.5 L3.2 18.7 L5.6 19.1 L4.4 17 L6.6 17.8 L5.8 15.6 L7.9 16.7 L7.4 14.4 L9.3 15.9 L9.3 13.5 L10.8 15.3 L11.2 12.9 L12.4 15 L13.2 12.7 L14.1 14.9 L15.2 12.8 L15.7 15.2 L17.2 13.3 L17.3 15.7 L19.1 14.1 L18.7 16.5 L20.8 15.2 L21.5 24.5 Z";
const FACE = "M18.2 16.6 C21.6 14.6 25.4 16.2 27.4 18.9 L30.2 20.3 C30.9 20.7 30.8 21.8 30 22 L25.5 23.3 C23.2 24.3 20.6 24.6 18.2 24.5 Z";

/** The hedgehog mark: a round back of quills, a soft face, one bright eye. */
export function Logo({ size = 24 }: { size?: number }) {
  const h = size * 0.75;
  return (
    <svg width={h * 1.88} height={h} viewBox="1 10.5 31 16.5" aria-label="Hoglet" role="img" className="flex-none">
      <path d={QUILLS} fill="var(--brand)" stroke="var(--brand)" strokeWidth="0.8" strokeLinejoin="round" />
      <path d={FACE} fill="#e3bf98" />
      <circle cx="30" cy="20.9" r="1.05" fill="#1b1a17" />
      <circle cx="24.2" cy="19.2" r="1.05" fill="#1b1a17" />
      <path d="M7 24.6v1.4M12 24.6v1.4M20.5 24.6v1.4" stroke="var(--brand)" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}
