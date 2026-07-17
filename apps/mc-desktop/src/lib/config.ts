import { Store } from "@tauri-apps/plugin-store";
import type { McEvent } from "./types.js";

// ── Versioned config shape ────────────────────────────────────────────

export interface AppConfigV1 {
  version: 1;
  knownPlugins: Array<{ name: string; moduleUuid: string }>;
  passcode: string;
  lastTargetModuleUuid: string;
  searchQuery: string;
  kindFilters: Record<string, boolean>;
  logLevel: "all" | 0 | 1 | 2;
  workspaceRoot: string;
}

export type AppConfig = AppConfigV1;

// ── Constants ─────────────────────────────────────────────────────────

const STORE_FILE = "settings.json";
const STORE_KEY = "app_config";

const VALID_KINDS: McEvent["kind"][] = [
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

const DEFAULT_KIND_FILTERS: Record<string, boolean> = {};
for (const k of VALID_KINDS) {
  DEFAULT_KIND_FILTERS[k] = true;
}

const DEFAULT_CONFIG: AppConfig = {
  version: 1,
  knownPlugins: [],
  passcode: "",
  lastTargetModuleUuid: "",
  searchQuery: "",
  kindFilters: { ...DEFAULT_KIND_FILTERS },
  logLevel: "all",
  workspaceRoot: "",
};

// ── Sanitize / merge ─────────────────────────────────────────────────

function sanitize(raw: unknown): AppConfig {
  if (!raw || typeof raw !== "object") {
    return { ...DEFAULT_CONFIG, kindFilters: { ...DEFAULT_KIND_FILTERS } };
  }

  const r = raw as Record<string, unknown>;

  // knownPlugins
  const knownPlugins: AppConfig["knownPlugins"] = [];
  if (Array.isArray(r.knownPlugins)) {
    for (const p of r.knownPlugins) {
      if (
        p &&
        typeof p === "object" &&
        typeof (p as Record<string, unknown>).name === "string" &&
        typeof (p as Record<string, unknown>).moduleUuid === "string"
      ) {
        knownPlugins.push({
          name: (p as Record<string, unknown>).name as string,
          moduleUuid: (p as Record<string, unknown>).moduleUuid as string,
        });
      }
    }
  }

  // kindFilters – only accept known keys with boolean values,
  // fall back to default for any missing key.
  const kindFilters: Record<string, boolean> = {};
  const rawKF = r.kindFilters;
  if (rawKF && typeof rawKF === "object") {
    for (const k of VALID_KINDS) {
      const v = (rawKF as Record<string, unknown>)[k];
      kindFilters[k] = typeof v === "boolean" ? v : DEFAULT_KIND_FILTERS[k];
    }
  } else {
    Object.assign(kindFilters, DEFAULT_KIND_FILTERS);
  }

  // logLevel
  const rawLevel = r.logLevel;
  const logLevel: AppConfig["logLevel"] =
    rawLevel === "all" || rawLevel === 0 || rawLevel === 1 || rawLevel === 2
      ? (rawLevel as AppConfig["logLevel"])
      : DEFAULT_CONFIG.logLevel;

  return {
    version: 1,
    knownPlugins,
    passcode: typeof r.passcode === "string" ? r.passcode : DEFAULT_CONFIG.passcode,
    lastTargetModuleUuid:
      typeof r.lastTargetModuleUuid === "string"
        ? r.lastTargetModuleUuid
        : DEFAULT_CONFIG.lastTargetModuleUuid,
    searchQuery:
      typeof r.searchQuery === "string" ? r.searchQuery : DEFAULT_CONFIG.searchQuery,
    kindFilters,
    logLevel,
    // Older configs predate source-map support; tolerate missing/invalid values.
    workspaceRoot:
      typeof r.workspaceRoot === "string" ? r.workspaceRoot : DEFAULT_CONFIG.workspaceRoot,
  };
}

// ── Store singleton ───────────────────────────────────────────────────

let _store: Store | null = null;
let _config: AppConfig | null = null;

async function getStore(): Promise<Store> {
  if (!_store) {
    _store = await Store.load(STORE_FILE);
  }
  return _store;
}

// ── Public API ────────────────────────────────────────────────────────

/** Load config from disk (or return cached). */
export async function loadConfig(): Promise<AppConfig> {
  if (_config) return _config;
  const store = await getStore();
  const raw = await store.get<unknown>(STORE_KEY);
  _config = sanitize(raw);
  return _config;
}

// ── Debounced persistence ─────────────────────────────────────────────

let _saveTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleSave(debounceMs = 400): void {
  if (_saveTimer) clearTimeout(_saveTimer);
  _saveTimer = setTimeout(async () => {
    _saveTimer = null;
    if (_config) {
      const store = await getStore();
      await store.set(STORE_KEY, _config);
      await store.save();
    }
  }, debounceMs);
}

// ── Individual save helpers ───────────────────────────────────────────

export function savePasscode(passcode: string): void {
  if (!_config) return;
  _config.passcode = passcode;
  scheduleSave();
}

export function saveLastTargetModuleUuid(uuid: string): void {
  if (!_config) return;
  _config.lastTargetModuleUuid = uuid;
  scheduleSave();
}

/**
 * Merge incoming plugins by UUID: add new UUIDs and update names for
 * existing ones. Accepts IPC-style PluginInfo (snake_case) or internal
 * camelCase – normalises to camelCase for storage.
 */
export function saveKnownPlugins(
  plugins: Array<{ name: string; moduleUuid?: string; module_uuid?: string }>,
): void {
  if (!_config) return;
  const map = new Map(_config.knownPlugins.map((p) => [p.moduleUuid, p]));
  for (const p of plugins) {
    const uuid = p.moduleUuid ?? p.module_uuid ?? "";
    if (uuid) {
      map.set(uuid, { name: p.name, moduleUuid: uuid });
    }
  }
  _config.knownPlugins = Array.from(map.values());
  scheduleSave();
}

export function saveFilterState(
  searchQuery: string,
  kindFilters: Record<string, boolean>,
  logLevel: "all" | 0 | 1 | 2,
): void {
  if (!_config) return;
  _config.searchQuery = searchQuery;
  const sanitized: Record<string, boolean> = {};
  for (const k of VALID_KINDS) {
    sanitized[k] = typeof kindFilters[k] === "boolean" ? kindFilters[k] : DEFAULT_KIND_FILTERS[k];
  }
  _config.kindFilters = sanitized;
  _config.logLevel = logLevel;
  scheduleSave();
}

/**
 * Persist the workspace root used for source-map resolution. An empty string
 * means "no workspace configured"; the backend receives null in that case.
 */
export function saveWorkspaceRoot(workspaceRoot: string): void {
  if (!_config) return;
  _config.workspaceRoot = workspaceRoot;
  scheduleSave();
}
