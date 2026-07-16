import type { McEvent } from "./types.js";

export function logLevelName(level: number): string {
  return level === 0 ? "LOG" : level === 1 ? "WARN" : "ERROR";
}

export function formatEvent(event: McEvent): string {
  const t = new Date().toLocaleTimeString();
  switch (event.kind) {
    case "protocol":
      return `${t} PROTOCOL v${event.version} (${event.plugins.length} plugins)`;
    case "stopped":
      return `${t} STOPPED ${event.reason} thread=${event.thread}`;
    case "thread":
      return `${t} THREAD ${event.reason} thread=${event.thread}`;
    case "print":
      return `${t} ${logLevelName(event.logLevel)} ${event.message}`;
    case "notification":
      return `${t} NOTICE ${event.message}`;
    case "stat2": {
      const names = event.stats.map((s) => s.name).join(", ");
      return `${t} STAT tick=${event.tick} [${names || "empty"}]`;
    }
    case "profilerCapture":
      return `${t} PROFILER ${event.captureBasePath}`;
    case "schema":
      return `${t} SCHEMA (${event.count} tabs)`;
    case "terminated":
      return `${t} TERMINATED ${event.reason ?? ""}`;
    case "unknown":
      return `${t} UNKNOWN ${event.typeName}`;
  }
}

export function eventIcon(kind: McEvent["kind"]): string {
  switch (kind) {
    case "protocol":
      return "server";
    case "stopped":
      return "pause";
    case "thread":
      return "activity";
    case "print":
      return "terminal";
    case "notification":
      return "bell";
    case "stat2":
      return "bar-chart-3";
    case "profilerCapture":
      return "timer";
    case "schema":
      return "layers";
    case "terminated":
      return "alert-circle";
    case "unknown":
      return "help-circle";
  }
}

export function eventColor(kind: McEvent["kind"]): string {
  switch (kind) {
    case "protocol":
      return "text-indigo-500 dark:text-indigo-400";
    case "stopped":
      return "text-rose-500 dark:text-rose-400";
    case "thread":
      return "text-amber-500 dark:text-amber-400";
    case "print":
      return "text-emerald-500 dark:text-emerald-400";
    case "notification":
      return "text-sky-500 dark:text-sky-400";
    case "stat2":
      return "text-violet-500 dark:text-violet-400";
    case "profilerCapture":
      return "text-fuchsia-500 dark:text-fuchsia-400";
    case "schema":
      return "text-cyan-500 dark:text-cyan-400";
    case "terminated":
      return "text-red-500 dark:text-red-400";
    case "unknown":
      return "text-stone-500 dark:text-stone-400";
  }
}
