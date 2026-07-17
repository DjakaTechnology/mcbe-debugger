export type PluginInfo = { name: string; module_uuid: string };
export type HandshakeInfo = {
  version: number;
  plugins: PluginInfo[];
  requirePasscode: boolean;
};
export type StatDataModel = {
  name: string;
  children?: StatDataModel[];
  values?: unknown[];
  should_aggregate?: boolean;
};

/**
 * A parsed source-map frame attached to a print/notification event.
 *
 * All coordinates are one-based for display. `frames` is optional so events
 * from older backend builds (which never carry it) remain valid.
 */
export type SourceFrame = {
  functionName: string | null;
  generatedPath: string;
  generatedLine: number;
  generatedColumn: number | null;
  sourcePath: string | null;
  sourceLine: number | null;
  sourceColumn: number | null;
  mapped: boolean;
};

export type McEvent =
  | { kind: "protocol"; version: number; plugins: PluginInfo[]; requirePasscode: boolean }
  | { kind: "stopped"; reason: string; thread: number }
  | { kind: "thread"; reason: string; thread: number }
  | { kind: "print"; message: string; logLevel: number; frames?: SourceFrame[] }
  | { kind: "notification"; message: string; logLevel: number; frames?: SourceFrame[] }
  | { kind: "stat2"; tick: number; stats: StatDataModel[] }
  | { kind: "profilerCapture"; captureBasePath: string }
  | { kind: "schema"; count: number }
  | { kind: "terminated"; reason: string | null }
  | { kind: "unknown"; typeName: string };

/**
 * Result of the `set_workspace_root` Tauri command.
 * `enabled` is false when no workspace root is configured; `mapPath` is the
 * resolved absolute path to the loaded map when mapping is active.
 */
export type SetWorkspaceRootResult = {
  enabled: boolean;
  mapPath: string | null;
  error: string | null;
};

/**
 * Frontend view of source-map status, derived from {@link SetWorkspaceRootResult}.
 * A missing map is surfaced as `unavailable`, never as the page error.
 */
export type SourceMapStatus =
  | { state: "loading" }
  | { state: "loaded"; mapPath: string }
  | { state: "disabled" }
  | { state: "unavailable"; message?: string }
  | { state: "error"; message: string };

export type ResponsePayload = {
  success: boolean;
  args?: unknown;
  message?: string;
};

export type StatSeries = { name: string; path: string; ticks: number[]; values: number[] };
export type StatCategory = { key: string; label: string; icon: string; groups: { name: string; series: StatSeries[] }[] };
