import type { ComponentType, ReactNode, SVGProps } from "react";

export type IconProps = SVGProps<SVGSVGElement>;
export type IconComponent = ComponentType<IconProps>;

/** Hosted's 20-unit glyph grid. Names describe meaning, not a third-party asset. */
function glyph(name: string, geometry: ReactNode): IconComponent {
  function Icon({ className, ...props }: IconProps) {
    const labelled = !!(props["aria-label"] || props["aria-labelledby"]);
    return <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"
      viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5"
      strokeLinecap="round" strokeLinejoin="round" focusable="false"
      aria-hidden={labelled ? undefined : true} role={labelled ? "img" : undefined}
      {...props} className={["relay-icon", className].filter(Boolean).join(" ")}
      data-icon={name}>{geometry}</svg>;
  }
  Icon.displayName = `RelayIcon(${name})`;
  return Icon;
}

// Navigation: open, architectural shapes echo the brand without repeating its mark.
export const LayoutDashboard = glyph("overview", <>
  <rect x="2.5" y="2.5" width="15" height="15" rx="3" />
  <path d="M8 2.5v15M8 8h9.5" />
</>);
export const Activity = glyph("requests", <>
  <path d="M3 5h10m-3-3 3 3-3 3M17 15H7m3-3-3 3 3 3M3 11h2m10-2h2" />
</>);
export const Models = glyph("models", <>
  <rect x="2.5" y="2.5" width="6" height="6" rx="1.5" />
  <rect x="11.5" y="11.5" width="6" height="6" rx="1.5" />
  <path d="M12 3h3a2 2 0 0 1 2 2v3M3 12v3a2 2 0 0 0 2 2h3" />
</>);
export const KeyRound = glyph("key", <>
  <circle cx="6.5" cy="8.5" r="4" />
  <path d="m9.5 11.5 5.5 5.5h2v-3l-2-2m-2 3 2-2" />
  <circle cx="5.5" cy="7.5" r=".75" fill="currentColor" stroke="none" />
</>);
export const Wallet = glyph("wallet", <>
  <path d="M16.5 6V4.5a2 2 0 0 0-2-2h-10a2 2 0 0 0 0 4h12v10h-12a2 2 0 0 1-2-2v-10" />
  <path d="M17.5 9.5h-4a2 2 0 0 0 0 4h4z" />
</>);
export const Settings = glyph("settings", <>
  <path d="M3 5h5m4 0h5M3 15h5m4 0h5M10 2.5v5M10 12.5v5M3 10h2m4 0h8M7 7.5v5" />
</>);
export const Cable = glyph("connection", <>
  <path d="M5 2.5v3m4-3v3M3 5.5h8V8a4 4 0 0 1-8 0zM7 12v2a3 3 0 0 0 6 0v-1" />
  <rect x="10.5" y="7.5" width="5" height="5" rx="1.5" />
  <path d="M15.5 10H18" />
</>);
export const Cloud = glyph("provider", <path d="M5 15.5a4 4 0 0 1-.5-8A5 5 0 0 1 14 6a4.8 4.8 0 0 1 1 9.5Z" />);
export const Server = glyph("api-source", <><rect x="2.5" y="2.5" width="15" height="6" rx="1.5" /><rect x="2.5" y="11.5" width="15" height="6" rx="1.5" /><path d="M5.5 5.5h.5m-.5 9h.5" /></>);
export const Pencil = glyph("edit", <><path d="m12.5 3.5 4 4M3 13l9.8-9.8a2.8 2.8 0 0 1 4 4L7 17l-4 1Z" /><path d="m3 13 4 4" /></>);
export const Layers2 = glyph("pool", <>
  <path d="m10 2.5 7.5 4-7.5 4-7.5-4zM2.5 10.5l7.5 4 7.5-4M2.5 14.5l7.5 4 7.5-4" />
</>);
export const Users = glyph("users", <>
  <circle cx="7.5" cy="6" r="3" />
  <path d="M2 17v-1a5.5 5.5 0 0 1 11 0v1M13 3.5a3 3 0 0 1 0 5.5m2 3a4.5 4.5 0 0 1 3 4.2v.8" />
</>);
export const UserRound = glyph("user", <>
  <circle cx="10" cy="6" r="3.5" />
  <path d="M3.5 17.5v-1a5 5 0 0 1 5-5h3a5 5 0 0 1 5 5v1" />
</>);
export const ShieldCheck = glyph("security", <>
  <path d="M10 2 3 4.5v5c0 4 3.5 6.8 7 8.5 3.5-1.7 7-4.5 7-8.5v-5zM6.5 9.5 9 12l4.5-5" />
</>);
export const ClipboardList = glyph("audit", <>
  <path d="M6.5 4H4a1.5 1.5 0 0 0-1.5 1.5v11A1.5 1.5 0 0 0 4 18h12a1.5 1.5 0 0 0 1.5-1.5v-11A1.5 1.5 0 0 0 16 4h-2.5M6 10h8M6 14h5" />
  <rect x="6.5" y="2" width="7" height="4" rx="1.5" />
</>);
export const LogOut = glyph("sign-out", <>
  <path d="M8 3H4.5A1.5 1.5 0 0 0 3 4.5v11A1.5 1.5 0 0 0 4.5 17H8M8 10h10m-3.5-3.5L18 10l-3.5 3.5" />
</>);

// Utilities and actions use the same optical weight as the navigation.
export const Menu = glyph("menu", <path d="M3 5h14M3 10h10M3 15h14" />);
export const X = glyph("close", <path d="m5 5 10 10M15 5 5 15" />);
export const Plus = glyph("add", <path d="M10 3.5v13M3.5 10h13" />);
export const MoreHorizontal = glyph("more", <><circle cx="4" cy="10" r=".75" fill="currentColor" stroke="none" /><circle cx="10" cy="10" r=".75" fill="currentColor" stroke="none" /><circle cx="16" cy="10" r=".75" fill="currentColor" stroke="none" /></>);
export const ListPlus = glyph("pool-add", <path d="M3 5h14M3 10h7M3 15h7M15 10v7m-3.5-3.5h7" />);
export const ListMinus = glyph("pool-remove", <path d="M3 5h14M3 10h7M3 15h7m2-1.5h6" />);
export const Pause = glyph("pause", <path d="M6.5 4v12M13.5 4v12" />);
export const Play = glyph("play", <path d="m6.5 3.5 10 6.5-10 6.5Z" />);
export const Trash = glyph("trash", <><path d="M3 5.5h14M7 5.5v-3h6v3M5 5.5l.8 12h8.4l.8-12M8.5 8.5v6M11.5 8.5v6" /></>);
export const Calendar = glyph("calendar", <>
  <rect x="2.5" y="4" width="15" height="13.5" rx="2.5" />
  <path d="M6.5 2v4M13.5 2v4M2.5 8.5h15M6.5 12h2M11.5 12h2" />
</>);
export const Search = glyph("search", <><circle cx="8.5" cy="8.5" r="5.5" /><path d="m12.5 12.5 4.5 4.5" /></>);
export const Copy = glyph("copy", <>
  <rect x="6.5" y="6.5" width="11" height="11" rx="2.5" />
  <path d="M3.5 13.5H3A1.5 1.5 0 0 1 1.5 12V3A1.5 1.5 0 0 1 3 1.5h9A1.5 1.5 0 0 1 13.5 3v.5" />
</>);
export const Download = glyph("download", <path d="M10 2.5v10m-3.5-3.5 3.5 3.5 3.5-3.5M3 13.5v3A1.5 1.5 0 0 0 4.5 18h11a1.5 1.5 0 0 0 1.5-1.5v-3" />);
export const RefreshCw = glyph("refresh", <path d="M16.5 7A7 7 0 0 0 4 5l-1.5 2m0-4v4h4M3.5 13A7 7 0 0 0 16 15l1.5-2m0 4v-4h-4" />);
export const RotateCcw = glyph("reset", <path d="M3 8a7 7 0 1 1 1 7M3 3.5V8h4.5M10 6v4l2.5 1.5" />);
export const SlidersHorizontal = glyph("filters", <>
  <path d="M2.5 5h3m4 0h8M2.5 15h8m4 0h3" />
  <rect x="5.5" y="2.5" width="4" height="5" rx="1.5" />
  <rect x="10.5" y="12.5" width="4" height="5" rx="1.5" />
</>);
export const Palette = glyph("appearance", <>
  <circle cx="10" cy="10" r="7.5" />
  <path d="M10 2.5v15M10 5h5.5M10 8h7M10 11h7M10 14h6" />
</>);
export const Braces = glyph("technical", <path d="M6.5 2.5H5A1.5 1.5 0 0 0 3.5 4v3.5L1.5 10l2 2.5V16A1.5 1.5 0 0 0 5 17.5h1.5M13.5 2.5H15A1.5 1.5 0 0 1 16.5 4v3.5l2 2.5-2 2.5V16a1.5 1.5 0 0 1-1.5 1.5h-1.5" />);
export const Bell = glyph("notifications", <>
  <path d="M5 8a5 5 0 0 1 10 0v3.5l2 3h-14l2-3zM8 17.5h4M10 1.5V3" />
</>);
export const Moon = glyph("dark-theme", <path d="M16.8 12A7.5 7.5 0 0 1 8 3a7.5 7.5 0 1 0 8.8 9Z" />);
export const Sun = glyph("light-theme", <>
  <circle cx="10" cy="10" r="3.5" />
  <path d="M10 1.5V3m0 14v1.5M1.5 10H3m14 0h1.5M4 4l1 1m10 10 1 1M4 16l1-1M15 5l1-1" />
</>);

// Direction is literal; status remains distinguishable without colour.
export const ArrowUpRight = glyph("arrow-up-right", <path d="M5 15 15 5M5 5h10v10" />);
export const ArrowRight = glyph("arrow-right", <path d="M3 10h14m-6-6 6 6-6 6" />);
export const ArrowUp = glyph("arrow-up", <path d="M10 17V3m-6 6 6-6 6 6" />);
export const ArrowDown = glyph("arrow-down", <path d="M10 3v14m-6-6 6 6 6-6" />);
export const ListOrdered = glyph("ordered-list", <><path d="M8 5h9M8 10h9M8 15h9M2.5 3.5H4V7M2.5 12.5c0-2 3-2 3-.2 0 1.2-3 2.1-3 4h3" /></>);
export const Sparkles = glyph("automatic", <><path d="m11 3 1.8 5.2L18 10l-5.2 1.8L11 17l-1.8-5.2L4 10l5.2-1.8Z" /><path d="M4 2v4M2 4h4" /></>);
export const GripVertical = glyph("reorder", <>{[5, 10, 15].map(y => <g key={y}><circle cx="7" cy={y} r=".8" fill="currentColor" stroke="none" /><circle cx="13" cy={y} r=".8" fill="currentColor" stroke="none" /></g>)}</>);
export const ChevronDown = glyph("chevron-down", <path d="m5 7.5 5 5 5-5" />);
export const ChevronUp = glyph("chevron-up", <path d="m5 12.5 5-5 5 5" />);
export const ChevronLeft = glyph("chevron-left", <path d="m12.5 5-5 5 5 5" />);
export const ChevronRight = glyph("chevron-right", <path d="m7.5 5 5 5-5 5" />);
export const Check = glyph("check", <path d="m3.5 10 4 4 9-9" />);
export const Minus = glyph("selection-partial", <path d="M4 10h12" />);
export const Square = glyph("selection-empty", <rect x="3.5" y="3.5" width="13" height="13" rx="2" />);
export const CheckCheck = glyph("read-all", <path d="m2 10 3.5 3.5L13 6M9 12l1.5 1.5L18 6" />);
export const CircleCheck = glyph("success", <><circle cx="10" cy="10" r="7.5" /><path d="m6.5 10 2.5 2.5 4.5-5" /></>);
export const CircleX = glyph("error", <><circle cx="10" cy="10" r="7.5" /><path d="m7.5 7.5 5 5m0-5-5 5" /></>);
export const CircleAlert = glyph("warning", <><circle cx="10" cy="10" r="7.5" /><path d="M10 5.5v5" /><circle cx="10" cy="14" r=".75" fill="currentColor" stroke="none" /></>);
export const Clock3 = glyph("waiting", <><circle cx="10" cy="10" r="7.5" /><path d="M10 5.5V10h4" /></>);

// Data and timing glyphs are small structural symbols, not decorative charts.
export const CreditCard = glyph("payment", <>
  <rect x="2.5" y="4" width="15" height="12" rx="2.5" />
  <path d="M2.5 8h15M6 12h3" />
</>);
export const Database = glyph("tokens", <>
  <ellipse cx="10" cy="4.5" rx="7" ry="2.5" />
  <path d="M3 4.5v10c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5v-10M3 9.5c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5" />
</>);
export const Gauge = glyph("speed", <>
  <path d="M4 16a7.5 7.5 0 1 1 12 0M10 3v2M3 10h2m10 0h2m-7 0 3-3" />
  <circle cx="10" cy="10" r="1" fill="currentColor" stroke="none" />
  <path d="M7 16h6" />
</>);
export const Zap = glyph("fast", <path d="m11.5 2-8 9H9l-.5 7 8-9H11Z" />);
export const Rocket = glyph("ultrafast", <><path d="M8 12c1-5 5-9 9-9 0 4-4 8-9 9Zm0 0-3-3-3 5 4 1 1 3 4-4-3-2Z" /><circle cx="13" cy="7" r="1.2" /><path d="m4 16-2 2" /></>);
export const Cpu = glyph("processor", <><rect x="5" y="5" width="10" height="10" rx="2" /><path d="M8 2v3m4-3v3M8 15v3m4-3v3M2 8h3m-3 4h3m10-4h3m-3 4h3" /><rect x="8" y="8" width="4" height="4" rx=".5" /></>);
export const Timer = glyph("duration", <>
  <circle cx="10" cy="11" r="6.5" />
  <path d="M7.5 1.5h5M10 1.5v3M15 6l1.5-1.5M10 7.5V11l2 2" />
</>);
export const Radio = glyph("in-flight", <>
  <path d="M3.5 6.5a7.5 7.5 0 0 1 13 0M16.5 13.5a7.5 7.5 0 0 1-13 0M2.5 3.5v3h3m12 10v-3h-3" />
  <circle cx="10" cy="10" r="1.5" fill="currentColor" stroke="none" />
</>);
