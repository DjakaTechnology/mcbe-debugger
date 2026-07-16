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

export type McEvent =
  | { kind: "protocol"; version: number; plugins: PluginInfo[]; requirePasscode: boolean }
  | { kind: "stopped"; reason: string; thread: number }
  | { kind: "thread"; reason: string; thread: number }
  | { kind: "print"; message: string; logLevel: number }
  | { kind: "notification"; message: string; logLevel: number }
  | { kind: "stat2"; tick: number; stats: StatDataModel[] }
  | { kind: "profilerCapture"; captureBasePath: string }
  | { kind: "schema"; count: number }
  | { kind: "terminated"; reason: string | null }
  | { kind: "unknown"; typeName: string };

export type ResponsePayload = {
  success: boolean;
  args?: unknown;
  message?: string;
};

export type StatSeries = { name: string; path: string; ticks: number[]; values: number[] };
export type StatCategory = { key: string; label: string; icon: string; groups: { name: string; series: StatSeries[] }[] };
