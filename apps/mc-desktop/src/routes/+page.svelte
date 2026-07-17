<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  import Wifi from "@lucide/svelte/icons/wifi";
  import WifiOff from "@lucide/svelte/icons/wifi-off";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Activity from "@lucide/svelte/icons/activity";
  import ScrollText from "@lucide/svelte/icons/scroll-text";
  import BarChart3 from "@lucide/svelte/icons/bar-chart-3";

  import type {
    McEvent,
    HandshakeInfo,
    ResponsePayload,
    StatSeries,
    PluginInfo,
    SetWorkspaceRootResult,
    SourceMapStatus,
  } from "$lib/types.js";
  import { accumulateStats, buildChartGroups, buildCategorizedGroups, buildClientIds, buildSubscriberAddonIds, kindOrder } from "$lib/stats.js";
  import { eventSearchText } from "$lib/events.js";
  import {
    loadConfig,
    savePasscode,
    saveLastTargetModuleUuid,
    saveKnownPlugins,
    saveFilterState,
    saveWorkspaceRoot,
  } from "$lib/config.js";

  import Sidebar from "$lib/components/Sidebar.svelte";
  import DebugControls from "$lib/components/DebugControls.svelte";
  import EventLogPanel from "$lib/components/EventLogPanel.svelte";
  import StatsPanel from "$lib/components/StatsPanel.svelte";
  import EvaluatePanel from "$lib/components/EvaluatePanel.svelte";
  import TargetSelectionModal from "$lib/components/TargetSelectionModal.svelte";

  let mode = $state<"listen" | "connect">("listen");
  let host = $state("127.0.0.1");
  let port = $state(19144);
  let targetModuleUuid = $state("");
  let passcode = $state("");
  let advancedOpen = $state(false);

  let connecting = $state(false);
  let connected = $state(false);
  let disconnected = $state(false);
  let handshake = $state<HandshakeInfo | null>(null);
  let error = $state<string | null>(null);
  let events = $state<McEvent[]>([]);

  let stopped = $state(false);
  let stoppedThreadId = $state<number | null>(null);
  let stopReason = $state<string>("");
  let busy = $state(false);

  let searchQuery = $state("");
  let kindFilters = $state<Record<McEvent["kind"], boolean>>({
    protocol: true,
    stopped: true,
    thread: true,
    print: true,
    notification: true,
    stat2: true,
    profilerCapture: true,
    schema: true,
    terminated: true,
    unknown: true,
  });
  let logLevel = $state<"all" | 0 | 1 | 2>("all");

  // ── Source-map workspace state ────────────────────────────────────────
  let workspaceRoot = $state("");
  let sourceMapStatus = $state<SourceMapStatus>({ state: "disabled" });

  // ── Config / auto-relisten state ──────────────────────────────────────
  let configLoaded = $state(false);
  let intentionalDisconnect = $state(false);
  let wasListenMode = $state(false);
  let autoRelistening = $state(false);

  let evalExpression = $state("");
  let evalHistory = $state<{ expression: string; result: ResponsePayload }[]>([]);

  let commandInput = $state("");
  let commandHistory = $state<string[]>([]);

  let activeTab = $state<"log" | "stats">("log");

  let statsCollection = $state<Record<string, StatSeries>>({});

  let activeStatsCategory = $state<string>("all");
  let selectedClient = $state<string | "all">("all");
  let selectedAddon = $state<string | "all">("all");

  // ── Target selection modal state ─────────────────────────────────────
  let targetSelectionPlugins = $state<PluginInfo[] | null>(null);
  let selectingTarget = $state(false);
  let targetSelectionError = $state<string | null>(null);

  let filteredEvents = $derived.by(() => {
    const query = searchQuery.trim().toLowerCase();
    return events.filter((event) => {
      if (!kindFilters[event.kind]) return false;
      if (logLevel !== "all" && (event.kind === "print" || event.kind === "notification")) {
        if (event.logLevel !== logLevel) return false;
      }
      // Search covers the raw formatted line plus any attached source frames
      // (function names, mapped source paths, generated paths).
      if (query && !eventSearchText(event).includes(query)) return false;
      return true;
    });
  });

  let status = $derived.by(() => {
    if (connected) {
      return {
        label: "Connected",
        dot: "bg-emerald-500",
        badge: "bg-emerald-50 text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300",
        icon: Wifi,
      };
    }
    if (disconnected) {
      return {
        label: "Disconnected",
        dot: "bg-rose-500",
        badge: "bg-rose-50 text-rose-700 dark:bg-rose-950/40 dark:text-rose-300",
        icon: WifiOff,
      };
    }
    if (connecting) {
      return {
        label: mode === "listen" ? "Waiting for MC..." : "Connecting...",
        dot: "bg-amber-500",
        badge: "bg-amber-50 text-amber-700 dark:bg-amber-950/40 dark:text-amber-300",
        icon: LoaderCircle,
      };
    }
    return {
      label: "Idle",
      dot: "bg-zinc-400",
      badge: "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-400",
      icon: null,
    };
  });

  let chartGroups = $derived.by(() => buildChartGroups(statsCollection));
  let categorizedGroups = $derived.by(() => buildCategorizedGroups(chartGroups));
  let clientIds = $derived.by(() => buildClientIds(statsCollection));
  let addonIds = $derived.by(() => buildSubscriberAddonIds(statsCollection));

  $effect(() => {
    if (selectedAddon !== "all" && !addonIds.includes(selectedAddon)) selectedAddon = "all";
  });

  onMount(() => {
    let unlistens: Array<() => void> = [];
    let cancelled = false;

    // 1) Load persisted config FIRST, then hydrate state
    loadConfig().then((cfg) => {
      if (cancelled) return;
      passcode = cfg.passcode;
      targetModuleUuid = cfg.lastTargetModuleUuid;
      searchQuery = cfg.searchQuery;
      for (const k of Object.keys(cfg.kindFilters)) {
        if (k in kindFilters) {
          (kindFilters as Record<string, boolean>)[k] = cfg.kindFilters[k];
        }
      }
      logLevel = cfg.logLevel;
      workspaceRoot = cfg.workspaceRoot;
      configLoaded = true;
    }).catch((err) => {
      if (cancelled) return;
      // Config load failed – retain defaults, mark loaded so saves work, surface error
      configLoaded = true;
      error = `Config load failed: ${err}`;
    });

    // 2) Set up event listeners
    const setupListeners = Promise.all([
      listen<McEvent>("mc-event", (e) => {
        events = [...events, e.payload].slice(-500);
        if (e.payload.kind === "stat2") {
          statsCollection = accumulateStats(statsCollection, e.payload.stats, e.payload.tick);
        }
        if (e.payload.kind === "stopped") {
          stopped = true;
          stoppedThreadId = e.payload.thread;
          stopReason = e.payload.reason;
        } else if (e.payload.kind === "thread" && e.payload.reason === "exited" && e.payload.thread === stoppedThreadId) {
          stopped = false;
          stoppedThreadId = null;
          stopReason = "";
        }
      }),
      listen<PluginInfo[]>("mc-target-selection-required", (e) => {
        // Ignore stale queued events after the user cancelled or disconnected
        if (!connecting) return;
        targetSelectionPlugins = e.payload;
        selectingTarget = false;
        targetSelectionError = null;
      }),
      listen("mc-disconnected", () => {
        disconnected = true;
        connected = false;
        handshake = null;
        stopped = false;
        stoppedThreadId = null;
        stopReason = "";
        targetSelectionError = null;
        // Auto-relisten if this was a listen-mode session not torn down by user
        if (!intentionalDisconnect && wasListenMode && !autoRelistening) {
          autoRelisten();
        }
        intentionalDisconnect = false;
      }),
      listen("mc-terminated", () => {
        disconnected = true;
        connected = false;
        handshake = null;
        stopped = false;
        stoppedThreadId = null;
        stopReason = "";
        targetSelectionError = null;
        if (!intentionalDisconnect && wasListenMode && !autoRelistening) {
          autoRelisten();
        }
        intentionalDisconnect = false;
      }),
    ]);

    setupListeners.then((uls) => {
      if (cancelled) {
        uls.forEach((ul) => ul());
      } else {
        unlistens = uls;
      }
    });

    return () => {
      cancelled = true;
      unlistens.forEach((ul) => ul());
    };
  });

  async function handleConnect() {
    connecting = true;
    error = null;
    targetSelectionError = null;
    disconnected = false;
    intentionalDisconnect = false;
    wasListenMode = mode === "listen";
    try {
      const targetUuid = targetModuleUuid.trim() || null;
      const pass = passcode.trim() || null;
      if (mode === "listen") {
        handshake = await invoke<HandshakeInfo>("listen_to_minecraft", {
          port,
          targetModuleUuid: targetUuid,
          passcode: pass,
        });
      } else {
        handshake = await invoke<HandshakeInfo>("connect_to_minecraft", {
          host,
          port,
          targetModuleUuid: targetUuid,
          passcode: pass,
        });
      }
      connected = true;
      targetSelectionPlugins = null;

      // Persist config after successful handshake
      if (handshake) {
        updateConfigAfterHandshake(handshake, passcode, targetModuleUuid);
      }
    } catch (e) {
      if (String(e) !== "cancelled") {
        error = String(e);
      }
      connected = false;
      targetSelectionPlugins = null;
    } finally {
      connecting = false;
    }
  }

  async function handleCancel() {
    intentionalDisconnect = true;
    autoRelistening = false; // abort any pending auto-relisten (during the 800ms delay)
    targetSelectionPlugins = null;
    selectingTarget = false;
    targetSelectionError = null;
    try {
      await invoke("cancel_pending_connect");
    } catch (e) {
      error = String(e);
    }
  }

  async function handleDisconnect() {
    intentionalDisconnect = true;
    try {
      await invoke("disconnect");
    } catch (e) {
      error = String(e);
    } finally {
      connected = false;
      handshake = null;
      stopped = false;
      stoppedThreadId = null;
      stopReason = "";
      targetSelectionPlugins = null;
      selectingTarget = false;
      targetSelectionError = null;
    }
  }

  async function handleSendCommand() {
    const cmd = commandInput.trim();
    if (!cmd) return;
    try {
      await invoke("send_minecraft_command", { command: cmd });
      commandHistory = [cmd, ...commandHistory.filter((c) => c !== cmd)].slice(0, 8);
      commandInput = "";
    } catch (e) {
      error = String(e);
    }
  }

  async function runDebugCommand(command: string, threadId: number, clearStopped: boolean) {
    busy = true;
    error = null;
    try {
      await invoke(command, { threadId });
      if (clearStopped) {
        stopped = false;
      }
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function pauseThread() {
    await runDebugCommand("pause_thread", stoppedThreadId ?? 0, false);
  }

  async function resumeThread() {
    await runDebugCommand("continue_thread", stoppedThreadId!, true);
  }

  async function stepNext() {
    await runDebugCommand("step_next", stoppedThreadId!, true);
  }

  async function stepIn() {
    await runDebugCommand("step_in", stoppedThreadId!, true);
  }

  async function stepOut() {
    await runDebugCommand("step_out", stoppedThreadId!, true);
  }

  async function handleEvaluate() {
    const expr = evalExpression.trim();
    if (!expr) return;
    busy = true;
    error = null;
    try {
      const result = await invoke<ResponsePayload>("evaluate", { expression: expr });
      evalHistory = [{ expression: expr, result }, ...evalHistory].slice(0, 10);
      if (!result.success) {
        error = result.message ?? "Evaluation failed";
      }
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
      evalExpression = "";
    }
  }

  function clearLog() {
    events = [];
  }

  function handleKindToggle(kind: McEvent["kind"]) {
    kindFilters[kind] = !kindFilters[kind];
  }

  function handleResetFilters() {
    searchQuery = "";
    kindFilters = {
      protocol: true,
      stopped: true,
      thread: true,
      print: true,
      notification: true,
      stat2: true,
      profilerCapture: true,
      schema: true,
      terminated: true,
      unknown: true,
    };
    logLevel = "all";
  }

  async function handleSelectTarget(plugin: PluginInfo) {
    selectingTarget = true;
    error = null;
    targetSelectionError = null;
    targetModuleUuid = plugin.module_uuid;
    try {
      await invoke("select_target_module", { moduleUuid: plugin.module_uuid });
      targetSelectionPlugins = null;
      targetSelectionError = null;
    } catch (e) {
      error = String(e);
      targetSelectionError = String(e);
      selectingTarget = false;
    }
  }

  function handleCancelTargetSelection() {
    targetSelectionPlugins = null;
    selectingTarget = false;
    targetSelectionError = null;
    handleCancel();
  }

  // ── Auto-save filter state (debounced) ────────────────────────────────
  let _filterSaveTimer: ReturnType<typeof setTimeout> | null = null;
  $effect(() => {
    // Track these reactive values
    searchQuery;
    kindFilters;
    logLevel;
    configLoaded;

    if (!configLoaded) return;

    if (_filterSaveTimer) clearTimeout(_filterSaveTimer);
    _filterSaveTimer = setTimeout(() => {
      _filterSaveTimer = null;
      saveFilterState(searchQuery, kindFilters, logLevel);
    }, 500);

    return () => {
      if (_filterSaveTimer) clearTimeout(_filterSaveTimer);
    };
  });

  // ── Source-map workspace root: persist + sync to backend ──────────────
  // Debounced so rapid edits (and filesystem path typing) don't trigger a
  // backend reload on every keystroke. A monotonic request id guards against
  // stale async responses overwriting a newer edit.
  let _workspaceTimer: ReturnType<typeof setTimeout> | null = null;
  let _workspaceReqId = 0;

  /**
   * Map a successful `set_workspace_root` response to frontend status.
   * A backend-reported status error (missing/malformed map) is `unavailable`
   * with its message — NOT `error`, which is reserved for an actual Tauri
   * invoke exception surfaced in the catch below. A missing map is never the
   * page's primary connection error.
   */
  function parseSourceMapStatus(r: SetWorkspaceRootResult): SourceMapStatus {
    if (r.error) return { state: "unavailable", message: r.error };
    if (!r.enabled) return { state: "disabled" };
    if (r.mapPath) return { state: "loaded", mapPath: r.mapPath };
    return { state: "unavailable" };
  }

  async function syncWorkspaceRoot(value: string): Promise<void> {
    const reqId = ++_workspaceReqId;
    const trimmed = value.trim();
    sourceMapStatus = { state: "loading" };
    try {
      const result = await invoke<SetWorkspaceRootResult>("set_workspace_root", {
        workspaceRoot: trimmed || null,
      });
      // Ignore stale responses so an old request can't overwrite a newer edit.
      if (reqId !== _workspaceReqId) return;
      sourceMapStatus = parseSourceMapStatus(result);
    } catch (e) {
      if (reqId !== _workspaceReqId) return;
      // Reserved for an actual Tauri invoke exception (command missing,
      // serialization failure, etc.), not a missing/malformed map.
      sourceMapStatus = { state: "error", message: String(e) };
    }
  }

  $effect(() => {
    workspaceRoot;
    configLoaded;

    if (!configLoaded) return;

    if (_workspaceTimer) clearTimeout(_workspaceTimer);
    _workspaceTimer = setTimeout(() => {
      _workspaceTimer = null;
      saveWorkspaceRoot(workspaceRoot);
      void syncWorkspaceRoot(workspaceRoot);
    }, 600);

    return () => {
      if (_workspaceTimer) clearTimeout(_workspaceTimer);
    };
  });

  // ── Auto-relisten ─────────────────────────────────────────────────────
  async function autoRelisten() {
    if (autoRelistening || connected) return;
    autoRelistening = true;
    connecting = true;
    error = null;
    targetSelectionError = null;
    disconnected = false;

    try {
      // Brief delay so UI can settle to "Waiting for MC..."
      await new Promise((r) => setTimeout(r, 800));
      if (!autoRelistening || connected) return;

      const targetUuid = targetModuleUuid.trim() || null;
      const pass = passcode.trim() || null;

      handshake = await invoke<HandshakeInfo>("listen_to_minecraft", {
        port,
        targetModuleUuid: targetUuid,
        passcode: pass,
      });
      connected = true;
      targetSelectionPlugins = null;

      // Persist config after successful reconnection
      if (handshake) {
        updateConfigAfterHandshake(handshake, passcode, targetModuleUuid);
      }
    } catch (e) {
      if (String(e) !== "cancelled") {
        error = String(e);
      }
      connected = false;
      targetSelectionPlugins = null;
    } finally {
      connecting = false;
      autoRelistening = false;
      selectingTarget = false;
      targetSelectionError = null;
    }
  }

  function updateConfigAfterHandshake(hs: HandshakeInfo, currentPasscode: string, requestedTargetUuid: string) {
    // Merge incoming plugins by UUID
    saveKnownPlugins(hs.plugins);

    // Derive the target UUID to persist from what was requested for this connection,
    // falling back to the sole handshake plugin UUID (auto-selected by the backend).
    const effectiveUuid = requestedTargetUuid.trim() || (hs.plugins.length === 1 ? hs.plugins[0].module_uuid : null);
    if (effectiveUuid) {
      saveLastTargetModuleUuid(effectiveUuid);
    }

    // Persist current passcode
    savePasscode(currentPasscode);
  }
</script>

<div class="flex h-screen w-full overflow-hidden bg-zinc-50 text-zinc-900 dark:bg-zinc-950 dark:text-zinc-100">
  <Sidebar
    {mode}
    {host}
    {port}
    {targetModuleUuid}
    {passcode}
    {advancedOpen}
    {connecting}
    {connected}
    {handshake}
    {error}
    {status}
    {commandInput}
    {commandHistory}
    {workspaceRoot}
    {sourceMapStatus}
    onModeChange={(m) => (mode = m)}
    onHostChange={(h) => (host = h)}
    onPortChange={(p) => (port = p)}
    onTargetUuidChange={(u) => (targetModuleUuid = u)}
    onPasscodeChange={(p) => (passcode = p)}
    onAdvancedToggle={() => (advancedOpen = !advancedOpen)}
    onConnect={handleConnect}
    onDisconnect={handleDisconnect}
    onCancel={handleCancel}
    onCommandInputChange={(c) => (commandInput = c)}
    onSendCommand={handleSendCommand}
    onCommandHistorySelect={(c) => (commandInput = c)}
    onWorkspaceRootChange={(w) => (workspaceRoot = w)}
  />

  <main class="flex min-w-0 flex-1 flex-col">
    <header class="flex items-center justify-between border-b border-zinc-200 bg-white px-4 py-2.5 dark:border-zinc-800 dark:bg-zinc-950">
      <div class="flex items-center gap-2">
        <Activity class="h-4 w-4 text-zinc-400 dark:text-zinc-500" />
        <span class="text-sm font-medium text-zinc-900 dark:text-zinc-100">{status.label}</span>
      </div>
      <div class="flex items-center gap-4 text-xs text-zinc-500 dark:text-zinc-400">
        <span>{events.length} events</span>
        {#if handshake}
          <span class="hidden sm:inline">Protocol v{handshake.version}</span>
        {/if}
      </div>
    </header>

    <DebugControls
      {connected}
      {stopped}
      {stopReason}
      {busy}
      onPause={pauseThread}
      onContinue={resumeThread}
      onStepNext={stepNext}
      onStepIn={stepIn}
      onStepOut={stepOut}
    />

    {#if connected}
      <div class="flex gap-1 border-b border-zinc-200 bg-zinc-50 px-3 py-1.5 dark:border-zinc-800 dark:bg-zinc-900">
        <button
          type="button"
          onclick={() => (activeTab = "log")}
          class="flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors {activeTab === 'log'
            ? 'bg-white text-indigo-600 shadow-sm dark:bg-zinc-700 dark:text-indigo-400'
            : 'text-zinc-500 hover:text-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-300'}"
        >
          <ScrollText class="h-3.5 w-3.5" />
          Event Log
        </button>
        <button
          type="button"
          onclick={() => (activeTab = "stats")}
          class="flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors {activeTab === 'stats'
            ? 'bg-white text-indigo-600 shadow-sm dark:bg-zinc-700 dark:text-indigo-400'
            : 'text-zinc-500 hover:text-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-300'}"
        >
          <BarChart3 class="h-3.5 w-3.5" />
          Stats
          {#if Object.keys(statsCollection).length > 0}
            <span class="ml-0.5 rounded-full bg-indigo-100 px-1.5 py-0.5 text-[9px] font-semibold text-indigo-600 dark:bg-indigo-950/40 dark:text-indigo-400">{Object.keys(statsCollection).length}</span>
          {/if}
        </button>
      </div>
    {/if}

    {#if activeTab === "log" || !connected}
      <EventLogPanel
        {events}
        {filteredEvents}
        {searchQuery}
        {kindFilters}
        {logLevel}
        onSearchChange={(q) => (searchQuery = q)}
        onKindToggle={handleKindToggle}
        onLogLevelChange={(l) => (logLevel = l)}
        onClearLog={clearLog}
        onResetFilters={handleResetFilters}
      />
    {/if}

    {#if connected && activeTab === "stats"}
      <StatsPanel
        {categorizedGroups}
        activeCategory={activeStatsCategory}
        {selectedClient}
        {clientIds}
        {selectedAddon}
        {addonIds}
        onCategoryChange={(c) => (activeStatsCategory = c)}
        onClientChange={(c) => (selectedClient = c)}
        onAddonChange={(a) => (selectedAddon = a)}
      />
    {/if}

    {#if connected && stopped}
      <EvaluatePanel
        {evalExpression}
        {evalHistory}
        {busy}
        onEvaluate={handleEvaluate}
        onExpressionChange={(e) => (evalExpression = e)}
      />
    {/if}
  </main>

  {#if targetSelectionPlugins}
    <TargetSelectionModal
      plugins={targetSelectionPlugins}
      selecting={selectingTarget}
      selectionError={targetSelectionError}
      onSelect={handleSelectTarget}
      onCancel={handleCancelTargetSelection}
    />
  {/if}
</div>
