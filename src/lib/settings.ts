import type { Protocol } from "./trace.ts";

/** Whether this is Windows, where the Windows ICMP API exists. */
export const WINDOWS = typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");
/** Whether the system can trace by ICMP without a raw socket: the Windows ICMP API, or Linux ping sockets. */
export const SYSTEM_ICMP = WINDOWS || (typeof navigator !== "undefined" && navigator.userAgent.includes("Linux"));
/** What raw sockets need, as the packet type menus say it. */
export const RAW_SOCKET_RIGHTS = WINDOWS ? "Administrator" : "raw sockets";
/** Whether this is macOS, where copy and paste in the page need an Edit menu. */
export const MACOS = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac OS");

/** The times between rounds the app offers, in ms. */
export const INTERVALS = [500, 1000, 2500, 5000, 10000];

/** A time between rounds in seconds, as the menus show it: "2.5 seconds". */
export const intervalLabel = (ms: number) => `${ms / 1000} ${ms === 1000 ? "second" : "seconds"}`;

/**
 * Reads a typed time between rounds ("1.5", "1.5s", "750ms", "1m") in ms, within the
 * 0.1 to 60 seconds the backend probes at. A number without a unit is in seconds up to 60,
 * where the menus stop, and in ms above that: "500" means 500 ms, not the 60 s it would clamp to.
 */
export function parseInterval(text: string): number | null {
  const match = /^(\d+(?:[.,]\d+)?|[.,]\d+)\s*([a-z]*)$/.exec(text.trim().toLowerCase());
  if (!match) return null;
  const count = Number(match[1].replace(",", "."));
  const bare = count > 60 ? 1 : 1000;
  const unit = !match[2] ? bare : match[2].startsWith("ms") || match[2].startsWith("mil") ? 1 : match[2].startsWith("m") ? 60_000 : 1000;
  if (match[2] && unit === 1000 && !match[2].startsWith("s")) return null;
  const ms = Math.round(count * unit);
  return Math.min(60_000, Math.max(100, ms));
}

const PROTOCOLS: Protocol[] = ["winicmp", "icmp", "udp", "tcp"];

/** The settings of the Settings tab. They belong to the app, not to one target. */
export type Settings = {
  // What a new target starts with. Each tab keeps its own once it is open.
  protocol: Protocol;
  intervalMs: number;
  timeoutMs: number;
  /** The whole IP packet, in bytes. */
  packetSize: number;
  /** Alert thresholds; 0 is off. */
  alertLatencyMs: number;
  alertLossPct: number;
  /** How many seconds of the latest probes an alert looks at. */
  alertSeconds: number;
  /** Port of the MCP server on the loopback address. */
  mcpPort: number;
  /** Latency in ms where the colour coding turns yellow, then red. */
  warnMs: number;
  badMs: number;
  /** How much history a tab keeps. */
  /** How many days of history a tab keeps on disk. */
  historyDays: number;
  /** Whether alerts and route changes raise a desktop notification. */
  notifications: boolean;
};

export const DEFAULTS: Settings = {
  protocol: "icmp",
  intervalMs: 1000,
  timeoutMs: 1000,
  packetSize: 84,
  alertLatencyMs: 0,
  alertLossPct: 0,
  alertSeconds: 30,
  mcpPort: 3917,
  warnMs: 100,
  badMs: 200,
  historyDays: 7,
  notifications: true,
};

/** The lowest and highest value of each setting that is a free number. */
export const LIMITS = {
  timeoutMs: [100, 10_000],
  packetSize: [48, 1024],
  alertLatencyMs: [0, 60_000],
  alertLossPct: [0, 100],
  alertSeconds: [1, 3600],
  mcpPort: [1024, 65_535],
  warnMs: [1, 10_000],
  badMs: [2, 60_000],
  historyDays: [1, 365],
} as const;

/**
 * Settings from whatever was stored or typed: every value that is missing, of the wrong
 * kind or not offered gets its default, and numbers are brought within their limits.
 * A new target probes through the system's ICMP (`systemIcmp`) where there is one, as that
 * needs neither raw sockets nor a firewall rule, and through a raw socket elsewhere.
 */
export function cleanSettings(raw: unknown, systemIcmp: boolean): Settings {
  const given = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const clean: Settings = { ...DEFAULTS, protocol: systemIcmp ? "winicmp" : "icmp" };
  for (const [key, [min, max]] of Object.entries(LIMITS) as [keyof typeof LIMITS, readonly [number, number]][]) {
    const value = Math.round(Number(given[key]));
    if (given[key] != null && given[key] !== "" && Number.isFinite(value)) {
      clean[key] = Math.min(max, Math.max(min, value));
    }
  }
  if (INTERVALS.includes(Number(given.intervalMs))) clean.intervalMs = Number(given.intervalMs);
  const protocol = given.protocol as Protocol;
  if (PROTOCOLS.includes(protocol) && (systemIcmp || protocol !== "winicmp")) clean.protocol = protocol;
  if (typeof given.notifications === "boolean") clean.notifications = given.notifications;
  // Red starts above yellow.
  clean.badMs = Math.max(clean.badMs, clean.warnMs + 1);
  return clean;
}

/**
 * Hours of the newest rounds a tab holds in the window. Older periods are read from the
 * history files when they are looked at. Matches the backend's WINDOW_HOURS.
 */
export const WINDOW_HOURS = 24;

// Most rounds the window holds whatever the interval. Matches the backend's MAX_HISTORY.
const MAX_ROUNDS = 345_600;

/** How many rounds `hours` are at an interval, up to MAX_ROUNDS. Matches the backend's `history_rounds`. */
export const historyRounds = (hours: number, intervalMs: number) =>
  Math.min(MAX_ROUNDS, Math.max(1, Math.floor((hours * 3_600_000) / intervalMs)));

const KEY = "traceplotter.settings";

function stored(): unknown {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "null");
  } catch {
    return null;
  }
}

/** The settings in use: a plain object changed in place by `changeSetting`. */
export const settings = cleanSettings(stored(), SYSTEM_ICMP);

/** Takes over one setting, kept within its limits, and remembers it. */
export function changeSetting(key: keyof Settings, value: unknown) {
  Object.assign(settings, cleanSettings({ ...settings, [key]: value }, SYSTEM_ICMP));
  localStorage.setItem(KEY, JSON.stringify(settings));
}
