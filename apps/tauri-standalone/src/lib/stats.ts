import type { StatDataModel, StatSeries, StatCategory, McEvent } from "./types.js";

export const statCategoryMap: Record<string, string> = {
  server_tick_timings: "server-performance",
  entities: "server-performance",
  chunks: "server-performance",
  networking: "server-performance",
  app_memory: "memory",
  dynamic_property_values: "memory",
  handle_counts: "scripting",
  fine_grained_subscribers: "scripting",
  client_stats: "client",
};

export const statCategoryOrder = ["server-performance", "memory", "scripting", "client", "uncategorized"];

export const statCategoryMeta: Record<string, { label: string; icon: string }> = {
  "server-performance": { label: "Server Performance", icon: "gauge" },
  memory: { label: "Memory", icon: "memory-stick" },
  scripting: { label: "Scripting", icon: "code" },
  client: { label: "Client", icon: "monitor" },
  uncategorized: { label: "Uncategorized", icon: "help-circle" },
};

export const kindOrder: McEvent["kind"][] = [
  "protocol",
  "stopped",
  "thread",
  "print",
  "notification",
  "stat2",
  "profilerCapture",
  "schema",
  "terminated",
  "unknown",
];

export function extractLastNumber(values: unknown[] | undefined): number | null {
  if (!values || values.length === 0) return null;
  const last = values[values.length - 1];
  if (typeof last === "number") return last;
  if (typeof last === "string") {
    const n = parseFloat(last);
    return isNaN(n) ? null : n;
  }
  return null;
}

export function accumulateStats(
  statsCollection: Record<string, StatSeries>,
  stats: StatDataModel[],
  tick: number,
  prefix = ""
): Record<string, StatSeries> {
  for (const stat of stats) {
    const path = prefix ? `${prefix}.${stat.name}` : stat.name;
    const value = extractLastNumber(stat.values);
    if (value !== null) {
      if (!statsCollection[path]) {
        statsCollection[path] = { name: path, path, ticks: [], values: [] };
      }
      const s = statsCollection[path];
      s.ticks.push(tick);
      s.values.push(value);
      if (s.ticks.length > 300) {
        s.ticks.shift();
        s.values.shift();
      }
    }
    if (stat.children) {
      accumulateStats(statsCollection, stat.children, tick, path);
    }
  }
  return { ...statsCollection };
}

export function formatStatValue(val: number | undefined): string {
  if (val === undefined) return "—";
  if (Math.abs(val) >= 1e9) return (val / 1e9).toFixed(2) + "B";
  if (Math.abs(val) >= 1e6) return (val / 1e6).toFixed(2) + "M";
  if (Math.abs(val) >= 1e3) return (val / 1e3).toFixed(1) + "K";
  return val.toFixed(1);
}

export function isMemoryGroup(groupName: string): boolean {
  return groupName.toLowerCase().includes("memory");
}

export function formatMemoryValue(mb: number): string {
  return mb.toFixed(2) + " MB";
}

export function formatGroupValue(groupName: string, val: number | undefined): string {
  if (val === undefined) return "—";
  if (isMemoryGroup(groupName)) return formatMemoryValue(val / 1048576);
  return formatStatValue(val);
}

export function isEmptySeries(series: StatSeries): boolean {
  return !series.values.some((v) => v !== null && v !== undefined && v !== 0);
}

export function scaleSeriesForDisplay(series: StatSeries, groupName: string): StatSeries {
  if (!isMemoryGroup(groupName)) return series;
  return {
    ...series,
    values: series.values.map((v) => v / 1048576),
  };
}

export function yAxisLabel(groupName: string): string {
  const n = groupName.toLowerCase();
  if (n.includes("memory")) return "↑ MB";
  if (n.includes("tick") || n.includes("timing")) return "↑ ms";
  if (n.includes("entit") || n.includes("count") || n.includes("handle")) return "↑ count";
  if (n.includes("network") || n.includes("packet")) return "↑ pkts";
  if (n.includes("chunk")) return "↑ chunks";
  return "↑ value";
}

export function makeChartOptions(groupName: string, seriesNames: string[]) {
  const colors = ["#396cd8", "#10b981", "#f59e0b", "#ef4444", "#8b5cf6", "#06b6d4", "#ec4899"];
  const theme = chartThemeColors();
  return {
    height: 140,
    series: [
      { label: "tick" },
      ...seriesNames.map((name, i) => ({
        label: name,
        stroke: colors[i % colors.length],
        width: 1.5,
        fill: seriesNames.length === 1 ? theme.fill : undefined,
      })),
    ],
    scales: { x: { time: false }, y: { auto: true } },
    axes: [
      {
        show: true,
        ticks: { show: false },
        grid: { show: false },
        values: () => [],
        label: "→ time",
        labelFont: "9px ui-sans-serif, sans-serif",
        size: 18,
        stroke: theme.text,
        labelColor: theme.text,
      },
      {
        show: true,
        label: yAxisLabel(groupName),
        labelFont: "9px ui-sans-serif, sans-serif",
        size: 50,
        font: "10px monospace",
        stroke: theme.text,
        labelColor: theme.text,
        grid: { stroke: theme.grid, width: 1 },
      },
    ],
    legend: { show: seriesNames.length > 1, live: true, font: "10px monospace" },
    cursor: { show: true, points: { size: 4 } },
  };
}

export function chartThemeColors() {
  const isDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  return {
    text: isDark ? "#a1a1aa" : "#71717a",
    grid: isDark ? "#27272a" : "#e4e4e7",
    fill: isDark ? "rgba(57, 108, 216, 0.15)" : "rgba(57, 108, 216, 0.08)",
  };
}

export function shortName(path: string): string {
  const parts = path.split(".");
  return parts.length > 1 ? parts.slice(1).join(".") : parts[0];
}

export function buildGroupData(series: StatSeries[]): any[] {
  if (series.length === 0) return [[], []];
  const longest = series.reduce((a, b) => (a.ticks.length > b.ticks.length ? a : b));
  const xTicks = [...longest.ticks];
  const result: any[] = [xTicks];
  for (const s of series) {
    if (s.ticks.length === xTicks.length) {
      result.push([...s.values]);
    } else {
      const tickMap = new Map<number, number>();
      for (let i = 0; i < s.ticks.length; i++) tickMap.set(s.ticks[i], s.values[i]);
      result.push(
        xTicks.map((t) => {
          const v = tickMap.get(t);
          return v === undefined ? null : v;
        })
      );
    }
  }
  return result;
}

export function getClientId(path: string): string | null {
  const parts = path.split(".");
  if (parts.length < 3 || parts[0] !== "client_stats") return null;
  return parts[1];
}

export function buildChartGroups(statsCollection: Record<string, StatSeries>) {
  const groups: Record<string, { name: string; series: StatSeries[] }> = {};
  for (const series of Object.values(statsCollection)) {
    const topLevel = series.path.split(".")[0];
    if (!groups[topLevel]) groups[topLevel] = { name: topLevel, series: [] };
    groups[topLevel].series.push(series);
  }
  return Object.values(groups).sort((a, b) => a.name.localeCompare(b.name));
}

export function buildCategorizedGroups(chartGroups: { name: string; series: StatSeries[] }[]): StatCategory[] {
  const categories: Record<string, StatCategory> = {};
  for (const key of statCategoryOrder) {
    const meta = statCategoryMeta[key];
    categories[key] = { key, label: meta.label, icon: meta.icon, groups: [] };
  }
  for (const group of chartGroups) {
    const categoryKey = statCategoryMap[group.name] ?? "uncategorized";
    categories[categoryKey].groups.push(group);
  }
  return Object.values(categories).filter((cat) => cat.groups.length > 0);
}

export function buildClientIds(statsCollection: Record<string, StatSeries>): string[] {
  const ids = new Set<string>();
  for (const series of Object.values(statsCollection)) {
    const clientId = getClientId(series.path);
    if (clientId) ids.add(clientId);
  }
  return Array.from(ids).sort();
}
