import type { McEvent, SourceFrame } from "./types.js";

export function logLevelName(level: number): string {
  return level === 0 ? "LOG" : level === 1 ? "WARN" : "ERROR";
}

// ── Source-frame helpers ─────────────────────────────────────────────
// Frame coordinates are one-based for display (per the IPC contract).

function formatLoc(line: number | null, column: number | null): string {
  if (line == null) return "";
  if (column == null) return `:${line}`;
  return `:${line}:${column}`;
}

/**
 * Compact one-line label for a frame. Mapped frames prioritise the original
 * source location (`functionName — sourcePath:line[:column]`); unmapped frames
 * fall back to the generated location.
 */
export function frameLabel(frame: SourceFrame): string {
  if (frame.mapped && frame.sourcePath) {
    const loc = formatLoc(frame.sourceLine, frame.sourceColumn);
    const fn = frame.functionName ? `${frame.functionName} — ` : "";
    return `${fn}${frame.sourcePath}${loc}`;
  }
  return `${frame.generatedPath}${formatLoc(frame.generatedLine, frame.generatedColumn)}`;
}

/**
 * Accessible title for a frame row. For mapped frames this carries the
 * generated location as provenance, so the subtle on-row text can stay short.
 */
export function frameTitle(frame: SourceFrame): string {
  if (frame.mapped && frame.sourcePath) {
    const src = `${frame.sourcePath}${formatLoc(frame.sourceLine, frame.sourceColumn)}`;
    const gen = `${frame.generatedPath}${formatLoc(frame.generatedLine, frame.generatedColumn)}`;
    return `Source: ${src}\nGenerated: ${gen}`;
  }
  return `${frame.generatedPath}${formatLoc(frame.generatedLine, frame.generatedColumn)}`;
}

/** Returns the frames attached to an event, or an empty array for kinds without them. */
export function getEventFrames(event: McEvent): SourceFrame[] {
  if ((event.kind === "print" || event.kind === "notification") && Array.isArray(event.frames)) {
    return event.frames;
  }
  return [];
}

/**
 * Whether a frame should be rendered as a subordinate row, or whether the raw
 * multiline message already contains its location visually (avoiding duplicate
 * output while preserving the raw message verbatim).
 *
 * A mapped frame is suppressed only when the raw message already contains the
 * mapped original source location (path, optionally with line/column). It is
 * intentionally NOT suppressed by a matching function name or generated path:
 * real stacks always include the function name and generated path, so using
 * those as suppression keys would hide every new original source row. For
 * example, raw text `registerShieldSystem (main.js:2394)` must still surface a
 * mapped `C:\...\src\shield.ts:42` row.
 *
 * An unmapped frame has no original source location, so it is suppressed when
 * the raw message already contains its generated path (the only location it
 * would display).
 */
export function frameVisibleInMessage(frame: SourceFrame, message: string): boolean {
  const msg = message.toLowerCase();
  if (frame.mapped && frame.sourcePath) {
    // Suppress only on the mapped original source path (with optional loc).
    if (msg.includes(frame.sourcePath.toLowerCase())) {
      const loc = formatLoc(frame.sourceLine, frame.sourceColumn);
      if (loc === "" || msg.includes(`${frame.sourcePath}${loc}`.toLowerCase())) return false;
    }
    return true;
  }
  // Unmapped: suppress only when the generated path is already shown verbatim.
  return !msg.includes(frame.generatedPath.toLowerCase());
}

/**
 * Lowercased searchable text for an event, combining the formatted raw line
 * with mapped/generated paths and function names so the existing search box
 * finds source-mapped entries without breaking old message search.
 */
export function eventSearchText(event: McEvent): string {
  const base = formatEvent(event).toLowerCase();
  const frames = getEventFrames(event);
  if (frames.length === 0) return base;
  const framesText = frames
    .map((f) => {
      const parts = [f.functionName, f.sourcePath, f.generatedPath];
      return parts.filter((p): p is string => Boolean(p)).join(" ");
    })
    .join(" ")
    .toLowerCase();
  return `${base} ${framesText}`;
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
