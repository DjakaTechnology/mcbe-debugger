<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  import Bug from "@lucide/svelte/icons/bug";
  import RadioTower from "@lucide/svelte/icons/radio-tower";
  import PlugZap from "@lucide/svelte/icons/plug-zap";
  import Play from "@lucide/svelte/icons/play";
  import Square from "@lucide/svelte/icons/square";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Server from "@lucide/svelte/icons/server";
  import ScrollText from "@lucide/svelte/icons/scroll-text";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import AlertCircle from "@lucide/svelte/icons/alert-circle";
  import Activity from "@lucide/svelte/icons/activity";
  import Terminal from "@lucide/svelte/icons/terminal";
  import Bell from "@lucide/svelte/icons/bell";
  import BarChart3 from "@lucide/svelte/icons/bar-chart-3";
  import Timer from "@lucide/svelte/icons/timer";
  import Layers from "@lucide/svelte/icons/layers";
  import HelpCircle from "@lucide/svelte/icons/help-circle";
  import Wifi from "@lucide/svelte/icons/wifi";
  import WifiOff from "@lucide/svelte/icons/wifi-off";
  import Pause from "@lucide/svelte/icons/pause";

  type PluginInfo = { name: string; module_uuid: string };
  type HandshakeInfo = {
    version: number;
    plugins: PluginInfo[];
    requirePasscode: boolean;
  };

  type McEvent =
    | { kind: "protocol"; version: number; plugins: PluginInfo[]; requirePasscode: boolean }
    | { kind: "stopped"; reason: string; thread: number }
    | { kind: "thread"; reason: string; thread: number }
    | { kind: "print"; message: string; logLevel: number }
    | { kind: "notification"; message: string; logLevel: number }
    | { kind: "stat2"; tick: number }
    | { kind: "profilerCapture"; captureBasePath: string }
    | { kind: "schema"; count: number }
    | { kind: "terminated"; reason: string | null }
    | { kind: "unknown"; typeName: string };

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
  let logElement = $state<HTMLDivElement | null>(null);

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

  onMount(() => {
    let unlistens: Array<() => void> = [];
    let cancelled = false;

    Promise.all([
      listen<McEvent>("mc-event", (e) => {
        events = [...events, e.payload].slice(-500);
      }),
      listen("mc-disconnected", () => {
        disconnected = true;
        connected = false;
        handshake = null;
      }),
      listen("mc-terminated", () => {
        disconnected = true;
        connected = false;
        handshake = null;
      }),
    ]).then((uls) => {
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

  $effect(() => {
    events.length;
    if (logElement) {
      logElement.scrollTop = logElement.scrollHeight;
    }
  });

  async function handleConnect() {
    connecting = true;
    error = null;
    disconnected = false;
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
    } catch (e) {
      error = String(e);
      connected = false;
    } finally {
      connecting = false;
    }
  }

  async function handleDisconnect() {
    try {
      await invoke("disconnect");
    } catch (e) {
      error = String(e);
    } finally {
      connected = false;
      handshake = null;
    }
  }

  function clearLog() {
    events = [];
  }

  function logLevelName(level: number): string {
    return level === 0 ? "LOG" : level === 1 ? "WARN" : "ERROR";
  }

  function formatEvent(event: McEvent): string {
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
      case "stat2":
        return `${t} STAT tick=${event.tick}`;
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

  function eventIcon(event: McEvent) {
    switch (event.kind) {
      case "protocol":
        return Server;
      case "stopped":
        return Pause;
      case "thread":
        return Activity;
      case "print":
        return Terminal;
      case "notification":
        return Bell;
      case "stat2":
        return BarChart3;
      case "profilerCapture":
        return Timer;
      case "schema":
        return Layers;
      case "terminated":
        return AlertCircle;
      case "unknown":
        return HelpCircle;
    }
  }

  function eventColor(event: McEvent): string {
    switch (event.kind) {
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
</script>

<div class="flex h-screen w-full overflow-hidden bg-zinc-50 text-zinc-900 dark:bg-zinc-950 dark:text-zinc-100">
  <aside class="flex w-80 flex-col border-r border-zinc-200 bg-zinc-100 dark:border-zinc-800 dark:bg-zinc-900">
    <div class="flex items-center gap-3 border-b border-zinc-200 p-4 dark:border-zinc-800">
      <div class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-indigo-600 text-white shadow-md shadow-indigo-600/20">
        <Bug class="h-6 w-6" />
      </div>
      <div class="min-w-0">
        <h1 class="truncate text-sm font-semibold tracking-tight text-zinc-900 dark:text-zinc-100">Minecraft Debugger</h1>
        <p class="text-xs text-zinc-500 dark:text-zinc-400">Standalone</p>
      </div>
    </div>

    <div class="flex-1 space-y-5 overflow-y-auto p-4">
      {#if error}
        <div class="flex items-start gap-2 rounded-lg border border-rose-200 bg-rose-50 p-3 text-xs text-rose-700 dark:border-rose-900/50 dark:bg-rose-950/30 dark:text-rose-300">
          <AlertCircle class="mt-0.5 h-4 w-4 shrink-0" />
          <span class="break-words">{error}</span>
        </div>
      {/if}

      <section class="space-y-3">
        <div class="flex items-center gap-2">
          <Activity class="h-4 w-4 text-zinc-500 dark:text-zinc-400" />
          <h2 class="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400">Connection</h2>
        </div>

        <div class="grid grid-cols-2 gap-1 rounded-lg bg-zinc-200/60 p-1 dark:bg-zinc-800/60">
          <button
            type="button"
            disabled={connecting || connected}
            onclick={() => (mode = "listen")}
            class="flex items-center justify-center gap-2 rounded-md px-3 py-2 text-xs font-medium transition-colors {mode === 'listen'
              ? 'bg-white text-indigo-600 shadow-sm dark:bg-zinc-700 dark:text-indigo-400'
              : 'text-zinc-600 hover:bg-zinc-200/50 dark:text-zinc-400 dark:hover:bg-zinc-800/50'} disabled:cursor-not-allowed disabled:opacity-50"
          >
            <RadioTower class="h-3.5 w-3.5" />
            Listen
          </button>
          <button
            type="button"
            disabled={connecting || connected}
            onclick={() => (mode = "connect")}
            class="flex items-center justify-center gap-2 rounded-md px-3 py-2 text-xs font-medium transition-colors {mode === 'connect'
              ? 'bg-white text-indigo-600 shadow-sm dark:bg-zinc-700 dark:text-indigo-400'
              : 'text-zinc-600 hover:bg-zinc-200/50 dark:text-zinc-400 dark:hover:bg-zinc-800/50'} disabled:cursor-not-allowed disabled:opacity-50"
          >
            <PlugZap class="h-3.5 w-3.5" />
            Connect
          </button>
        </div>

        <div class="space-y-3">
          {#if mode === "connect"}
            <label class="block">
              <span class="mb-1 block text-xs font-medium text-zinc-600 dark:text-zinc-400">Host</span>
              <input
                type="text"
                bind:value={host}
                disabled={connecting || connected}
                class="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 disabled:cursor-not-allowed disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
              />
            </label>
          {/if}

          <label class="block">
            <span class="mb-1 block text-xs font-medium text-zinc-600 dark:text-zinc-400">Port</span>
            <input
              type="number"
              bind:value={port}
              disabled={connecting || connected}
              class="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 disabled:cursor-not-allowed disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
            />
          </label>
        </div>

        <button
          type="button"
          onclick={() => (advancedOpen = !advancedOpen)}
          class="flex w-full items-center justify-between text-xs font-medium text-zinc-500 transition-colors hover:text-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-300"
        >
          <span>Advanced options</span>
          {#if advancedOpen}
            <ChevronDown class="h-3.5 w-3.5" />
          {:else}
            <ChevronRight class="h-3.5 w-3.5" />
          {/if}
        </button>

        {#if advancedOpen}
          <div class="space-y-3 pt-1">
            <label class="block">
              <span class="mb-1 block text-xs font-medium text-zinc-600 dark:text-zinc-400">Target module UUID</span>
              <input
                type="text"
                bind:value={targetModuleUuid}
                disabled={connecting || connected}
                placeholder="Required if multiple plugins"
                class="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 disabled:cursor-not-allowed disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
              />
            </label>

            <label class="block">
              <span class="mb-1 block text-xs font-medium text-zinc-600 dark:text-zinc-400">Passcode</span>
              <input
                type="text"
                bind:value={passcode}
                disabled={connecting || connected}
                placeholder="Only if MC requires one"
                class="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 disabled:cursor-not-allowed disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
              />
            </label>
          </div>
        {/if}

        {#if !connected}
          <button
            type="button"
            onclick={handleConnect}
            disabled={connecting}
            class="flex w-full items-center justify-center gap-2 rounded-lg bg-indigo-600 px-4 py-2.5 text-sm font-semibold text-white shadow-md shadow-indigo-600/20 transition-colors hover:bg-indigo-700 focus:outline-none focus:ring-2 focus:ring-indigo-500/40 disabled:cursor-not-allowed disabled:bg-indigo-600 disabled:opacity-60"
          >
            {#if connecting}
              <LoaderCircle class="h-4 w-4 animate-spin" />
              {mode === "listen" ? "Waiting for MC..." : "Connecting..."}
            {:else}
              <Play class="h-4 w-4" />
              {mode === "listen" ? "Listen" : "Connect"}
            {/if}
          </button>
        {:else}
          <button
            type="button"
            onclick={handleDisconnect}
            class="flex w-full items-center justify-center gap-2 rounded-lg bg-rose-600 px-4 py-2.5 text-sm font-semibold text-white shadow-md shadow-rose-600/20 transition-colors hover:bg-rose-700 focus:outline-none focus:ring-2 focus:ring-rose-500/40"
          >
            <Square class="h-4 w-4" />
            Disconnect
          </button>
        {/if}
      </section>

      {#if handshake}
        <section class="space-y-3">
          <div class="flex items-center gap-2">
            <Server class="h-4 w-4 text-indigo-500 dark:text-indigo-400" />
            <h2 class="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400">Handshake</h2>
          </div>

          <div class="space-y-2 rounded-lg border border-zinc-200 bg-white p-3 text-xs dark:border-zinc-800 dark:bg-zinc-950">
            <div class="flex items-center justify-between">
              <span class="text-zinc-500 dark:text-zinc-400">Protocol</span>
              <span class="font-mono font-semibold text-zinc-900 dark:text-zinc-100">v{handshake.version}</span>
            </div>
            <div class="flex items-center justify-between">
              <span class="text-zinc-500 dark:text-zinc-400">Passcode</span>
              <span class="font-medium text-zinc-900 dark:text-zinc-100">{handshake.requirePasscode ? "Required" : "Not required"}</span>
            </div>
            <div class="pt-1">
              <span class="text-zinc-500 dark:text-zinc-400">Plugins ({handshake.plugins.length})</span>
              {#if handshake.plugins.length === 0}
                <p class="mt-1 italic text-zinc-400 dark:text-zinc-500">none</p>
              {:else}
                <ul class="mt-1 space-y-1">
                  {#each handshake.plugins as p (p.module_uuid)}
                    <li class="rounded-md bg-zinc-50 px-2 py-1.5 dark:bg-zinc-900">
                      <div class="font-medium text-zinc-900 dark:text-zinc-100">{p.name}</div>
                      <div class="font-mono text-[10px] text-zinc-500 dark:text-zinc-400">{p.module_uuid}</div>
                    </li>
                  {/each}
                </ul>
              {/if}
            </div>
          </div>
        </section>
      {/if}
    </div>

    <div class="flex items-center justify-between border-t border-zinc-200 p-3 dark:border-zinc-800">
      <span class="text-xs font-medium text-zinc-500 dark:text-zinc-400">Status</span>
      <div class="flex items-center gap-2 rounded-md {status.badge} px-2.5 py-1.5 text-xs font-medium">
        {#if status.icon === LoaderCircle}
          <LoaderCircle class="h-3.5 w-3.5 animate-spin" />
        {:else if status.icon}
          {@const Icon = status.icon}
          <Icon class="h-3.5 w-3.5" />
        {:else}
          <span class="h-1.5 w-1.5 rounded-full {status.dot}"></span>
        {/if}
        {status.label}
      </div>
    </div>
  </aside>

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

    <div class="flex min-h-0 flex-1 flex-col p-4">
      <section class="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-zinc-200 bg-white shadow-sm dark:border-zinc-800 dark:bg-zinc-950">
        <div class="flex items-center justify-between border-b border-zinc-200 px-4 py-2.5 dark:border-zinc-800">
          <div class="flex items-center gap-2">
            <ScrollText class="h-4 w-4 text-zinc-500 dark:text-zinc-400" />
            <h2 class="text-sm font-semibold text-zinc-900 dark:text-zinc-100">Event Log</h2>
          </div>
          <button
            type="button"
            onclick={clearLog}
            disabled={events.length === 0}
            class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium text-zinc-600 transition-colors hover:bg-zinc-100 hover:text-zinc-900 disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
          >
            <Trash2 class="h-3.5 w-3.5" />
            Clear
          </button>
        </div>

        <div bind:this={logElement} class="flex-1 overflow-y-auto p-2">
          {#if events.length === 0}
            <div class="flex h-full flex-col items-center justify-center text-zinc-400 dark:text-zinc-600">
              <ScrollText class="mb-2 h-8 w-8 opacity-50" />
              <p class="text-sm">No events yet.</p>
            </div>
          {:else}
            <ul class="space-y-0.5 font-mono text-xs">
              {#each events as event, i (i)}
                {@const Icon = eventIcon(event)}
                <li class="flex items-start gap-2 rounded-md px-2 py-1.5 transition-colors hover:bg-zinc-50 dark:hover:bg-zinc-900">
                  <Icon class="mt-0.5 h-3.5 w-3.5 shrink-0 {eventColor(event)}" />
                  <span class="break-all text-zinc-700 dark:text-zinc-300">{formatEvent(event)}</span>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      </section>
    </div>
  </main>
</div>
