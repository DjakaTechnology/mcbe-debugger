<script lang="ts">
  import { slide, fly, fade } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import Bug from "@lucide/svelte/icons/bug";
  import RadioTower from "@lucide/svelte/icons/radio-tower";
  import PlugZap from "@lucide/svelte/icons/plug-zap";
  import Play from "@lucide/svelte/icons/play";
  import Square from "@lucide/svelte/icons/square";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Server from "@lucide/svelte/icons/server";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import AlertCircle from "@lucide/svelte/icons/alert-circle";
  import Activity from "@lucide/svelte/icons/activity";
  import Terminal from "@lucide/svelte/icons/terminal";
  import MapPin from "@lucide/svelte/icons/map-pin";
  import MapPinOff from "@lucide/svelte/icons/map-pin-off";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import type { HandshakeInfo, SourceMapStatus, WorkspaceInfo } from "$lib/types.js";

  let {
    mode,
    host,
    port,
    targetModuleUuid,
    passcode,
    advancedOpen,
    connecting,
    connected,
    handshake,
    error,
    status,
    commandInput,
    commandHistory,
    workspaceRoot,
    workspaceInfo,
    workspaceError,
    workspaceLoading,
    sourceMapPath,
    sourceMapStatus,
    onModeChange,
    onHostChange,
    onPortChange,
    onTargetUuidChange,
    onPasscodeChange,
    onAdvancedToggle,
    onConnect,
    onDisconnect,
    onCancel,
    onCommandInputChange,
    onSendCommand,
    onCommandHistorySelect,
    onOpenWorkspace,
    onClearWorkspace,
    onSourceMapPathChange,
  }: {
    mode: "listen" | "connect";
    host: string;
    port: number;
    targetModuleUuid: string;
    passcode: string;
    advancedOpen: boolean;
    connecting: boolean;
    connected: boolean;
    handshake: HandshakeInfo | null;
    error: string | null;
    status: { label: string; dot: string; badge: string; icon: any | null };
    commandInput: string;
    commandHistory: string[];
    workspaceRoot: string;
    workspaceInfo: WorkspaceInfo | null;
    workspaceError: string | null;
    workspaceLoading: boolean;
    sourceMapPath: string;
    sourceMapStatus: SourceMapStatus;
    onModeChange: (mode: "listen" | "connect") => void;
    onHostChange: (host: string) => void;
    onPortChange: (port: number) => void;
    onTargetUuidChange: (uuid: string) => void;
    onPasscodeChange: (passcode: string) => void;
    onAdvancedToggle: () => void;
    onConnect: () => void;
    onDisconnect: () => void;
    onCancel: () => void;
    onCommandInputChange: (cmd: string) => void;
    onSendCommand: () => void;
    onCommandHistorySelect: (cmd: string) => void;
    onOpenWorkspace: () => void;
    onClearWorkspace: () => void;
    onSourceMapPathChange: (path: string) => void;
  } = $props();

  // Compact inline status for the workspace-root field.
  let sourceMapBadge = $derived.by(() => {
    switch (sourceMapStatus.state) {
      case "loading":
        return {
          label: "Checking source map…",
          dot: "bg-amber-500",
          text: "text-amber-600 dark:text-amber-400",
          icon: LoaderCircle,
          spin: true,
          title: "Loading source map",
        };
      case "loaded":
        return {
          label: "Source map found",
          dot: "bg-emerald-500",
          text: "text-emerald-600 dark:text-emerald-400",
          icon: MapPin,
          spin: false,
          title: sourceMapStatus.mapPath,
        };
      case "unavailable":
        return {
          label: "Source map not found",
          dot: "bg-zinc-400",
          text: "text-zinc-500 dark:text-zinc-400",
          icon: MapPinOff,
          spin: false,
          // Carries the backend status message when present (missing or
          // malformed map); falls back to a helpful default otherwise.
          title: sourceMapStatus.message ?? "No matching source map found under MOJANG_DIR",
        };
      case "error":
        return {
          label: "Source map check failed",
          dot: "bg-rose-500",
          text: "text-rose-600 dark:text-rose-400",
          icon: AlertCircle,
          spin: false,
          title: sourceMapStatus.message,
        };
      case "disabled":
      default:
        return {
          label: "Connect to detect source map",
          dot: "bg-zinc-400",
          text: "text-zinc-500 dark:text-zinc-400",
          icon: MapPinOff,
          spin: false,
          title: "Source maps are detected after connecting to a script module",
        };
    }
  });
</script>

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
      <div transition:fade={{ duration: 150 }} class="flex items-start gap-2 rounded-lg border border-rose-200 bg-rose-50 p-3 text-xs text-rose-700 dark:border-rose-900/50 dark:bg-rose-950/30 dark:text-rose-300">
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
          onclick={() => onModeChange("listen")}
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
          onclick={() => onModeChange("connect")}
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
              value={host}
              oninput={(e) => onHostChange(e.currentTarget.value)}
              disabled={connecting || connected}
              class="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 disabled:cursor-not-allowed disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
            />
          </label>
        {/if}

        <label class="block">
          <span class="mb-1 block text-xs font-medium text-zinc-600 dark:text-zinc-400">Port</span>
          <input
            type="number"
            value={port}
            oninput={(e) => onPortChange(Number(e.currentTarget.value))}
            disabled={connecting || connected}
            class="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 disabled:cursor-not-allowed disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
          />
        </label>
      </div>

      <button
        type="button"
        onclick={onAdvancedToggle}
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
        <div transition:slide={{ duration: 200, easing: cubicOut }} class="space-y-3 pt-1">
          <label class="block">
            <span class="mb-1 block text-xs font-medium text-zinc-600 dark:text-zinc-400">Target module UUID</span>
            <input
              type="text"
              value={targetModuleUuid}
              oninput={(e) => onTargetUuidChange(e.currentTarget.value)}
              disabled={connecting || connected}
              placeholder="Required if multiple plugins"
              class="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 disabled:cursor-not-allowed disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
            />
          </label>

          <label class="block">
            <span class="mb-1 block text-xs font-medium text-zinc-600 dark:text-zinc-400">Passcode</span>
            <input
              type="text"
              value={passcode}
              oninput={(e) => onPasscodeChange(e.currentTarget.value)}
              disabled={connecting || connected}
              placeholder="Only if MC requires one"
              class="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 disabled:cursor-not-allowed disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
            />
          </label>

          <div class="rounded-md border border-zinc-200 bg-white/60 p-2.5 dark:border-zinc-800 dark:bg-zinc-950/50">
            <span class="flex items-center gap-1.5">
              <FolderOpen class="h-3 w-3 text-zinc-500 dark:text-zinc-400" />
              <span class="text-xs font-medium text-zinc-600 dark:text-zinc-400">Workspace</span>
            </span>
            <span class="mt-0.5 block text-[10px] leading-relaxed text-zinc-400 dark:text-zinc-500">
              Open a Regolith folder containing <code class="font-mono">config.json</code>.
            </span>
            <div class="mt-2 flex gap-1.5">
              <button
                type="button"
                onclick={onOpenWorkspace}
                class="flex flex-1 items-center justify-center gap-1 rounded-md border border-zinc-200 px-2 py-1.5 text-[10px] font-medium text-zinc-600 transition-colors hover:bg-zinc-100 hover:text-zinc-800 dark:border-zinc-700 dark:text-zinc-300 dark:hover:bg-zinc-800"
              >
                {#if workspaceLoading}
                  <LoaderCircle class="h-3 w-3 animate-spin" />
                {:else}
                  <FolderOpen class="h-3 w-3" />
                {/if}
                {workspaceRoot ? "Change workspace" : "Open workspace"}
              </button>
              {#if workspaceRoot}
                <button
                  type="button"
                  onclick={onClearWorkspace}
                  class="rounded-md border border-zinc-200 px-2 text-[10px] font-medium text-zinc-500 transition-colors hover:bg-zinc-100 hover:text-zinc-700 dark:border-zinc-700 dark:text-zinc-400 dark:hover:bg-zinc-800"
                >
                  Clear
                </button>
              {/if}
            </div>
            {#if workspaceInfo}
              <span class="mt-1.5 block truncate font-mono text-[9px] text-zinc-500 dark:text-zinc-500" title={workspaceInfo.root}>
                {workspaceInfo.root}
              </span>
              <div class="mt-1 grid grid-cols-[auto_1fr] gap-x-2 text-[9px] text-zinc-400 dark:text-zinc-600">
                <span>BP</span>
                <span class="truncate font-mono" title={workspaceInfo.behaviorPackUuid ?? "No header UUID"}>{workspaceInfo.behaviorPackUuid ?? "No header UUID"}</span>
                <span>RP</span>
                <span class="truncate font-mono" title={workspaceInfo.resourcePackUuid ?? "Not configured"}>{workspaceInfo.resourcePackUuid ?? "Not configured"}</span>
              </div>
            {:else if workspaceError}
              <span class="mt-1.5 block break-words text-[9px] leading-relaxed text-rose-500 dark:text-rose-400">{workspaceError}</span>
            {/if}
          </div>

          <div class="rounded-md border border-zinc-200 bg-white/60 p-2.5 dark:border-zinc-800 dark:bg-zinc-950/50">
            <span class="flex items-center gap-1.5">
              <MapPin class="h-3 w-3 text-zinc-500 dark:text-zinc-400" />
              <span class="text-xs font-medium text-zinc-600 dark:text-zinc-400">Source maps</span>
            </span>
            <span class="mt-0.5 block text-[10px] leading-relaxed text-zinc-400 dark:text-zinc-500">
              Uses the open workspace with <code class="font-mono">MOJANG_DIR</code>, or a manual map override.
            </span>
            <div class="mt-2 flex gap-1.5">
              <input
                type="text"
                value={sourceMapPath}
                oninput={(event) => onSourceMapPathChange(event.currentTarget.value)}
                placeholder="C:\path\to\script.js.map"
                spellcheck="false"
                autocomplete="off"
                aria-label="Manual source map path"
                class="min-w-0 flex-1 rounded-md border border-zinc-300 bg-white px-2 py-1.5 font-mono text-[10px] text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
              />
              {#if sourceMapPath}
                <button
                  type="button"
                  onclick={() => onSourceMapPathChange("")}
                  class="rounded-md border border-zinc-200 px-2 text-[10px] font-medium text-zinc-500 transition-colors hover:bg-zinc-100 hover:text-zinc-700 dark:border-zinc-700 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-200"
                  title="Clear override and use automatic detection"
                >
                  Auto
                </button>
              {/if}
            </div>
            <div
              class="mt-1.5 flex items-center gap-1.5 text-[11px] {sourceMapBadge.text}"
              title={sourceMapBadge.title}
            >
              <span class="h-1.5 w-1.5 shrink-0 rounded-full {sourceMapBadge.dot} {sourceMapBadge.spin ? 'animate-pulse' : ''}"></span>
              {#if sourceMapBadge.spin}
                <LoaderCircle class="h-3 w-3 shrink-0 animate-spin" />
              {:else}
                {@const StatusIcon = sourceMapBadge.icon}
                <StatusIcon class="h-3 w-3 shrink-0" />
              {/if}
              <span class="truncate">{sourceMapBadge.label}</span>
            </div>
            {#if sourceMapStatus.state === "loaded"}
              <span class="mt-1 block truncate font-mono text-[9px] text-zinc-400 dark:text-zinc-600" title={sourceMapStatus.mapPath}>
                {sourceMapStatus.mapPath}
              </span>
            {/if}
          </div>
        </div>
      {/if}

      {#if connecting}
        <button
          type="button"
          onclick={onCancel}
          class="flex w-full items-center justify-center gap-2 rounded-lg bg-rose-600 px-4 py-2.5 text-sm font-semibold text-white shadow-md shadow-rose-600/20 transition-colors hover:bg-rose-700 focus:outline-none focus:ring-2 focus:ring-rose-500/40"
        >
          <LoaderCircle class="h-4 w-4 animate-spin" />
          {mode === "listen" ? "Stop listening" : "Stop connecting"}
        </button>
      {:else if !connected}
        <button
          type="button"
          onclick={onConnect}
          class="flex w-full items-center justify-center gap-2 rounded-lg bg-indigo-600 px-4 py-2.5 text-sm font-semibold text-white shadow-md shadow-indigo-600/20 transition-colors hover:bg-indigo-700 focus:outline-none focus:ring-2 focus:ring-indigo-500/40"
        >
          <Play class="h-4 w-4" />
          {mode === "listen" ? "Listen" : "Connect"}
        </button>
      {:else}
        <button
          type="button"
          onclick={onDisconnect}
          class="flex w-full items-center justify-center gap-2 rounded-lg bg-rose-600 px-4 py-2.5 text-sm font-semibold text-white shadow-md shadow-rose-600/20 transition-colors hover:bg-rose-700 focus:outline-none focus:ring-2 focus:ring-rose-500/40"
        >
          <Square class="h-4 w-4" />
          Disconnect
        </button>
      {/if}
    </section>

    {#if handshake}
      <section transition:fly={{ y: -12, duration: 200, easing: cubicOut }} class="space-y-3">
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

    {#if connected}
      <section class="space-y-3">
        <div class="flex items-center gap-2">
          <Terminal class="h-4 w-4 text-zinc-500 dark:text-zinc-400" />
          <h2 class="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400">Commands</h2>
        </div>

        <form onsubmit={(e) => { e.preventDefault(); onSendCommand(); }}>
          <div class="relative">
            <Terminal class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400 dark:text-zinc-500" />
            <input
              type="text"
              value={commandInput}
              oninput={(e) => onCommandInputChange(e.currentTarget.value)}
              placeholder="/say hello"
              class="w-full rounded-md border border-zinc-300 bg-white py-2 pl-8 pr-3 text-sm text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
            />
          </div>
        </form>

        {#if commandHistory.length > 0}
          <div class="flex flex-wrap gap-1">
            {#each commandHistory as cmd, i (i)}
              <button
                type="button"
                onclick={() => onCommandHistorySelect(cmd)}
                title={cmd}
                class="max-w-full truncate rounded-full border border-zinc-200 bg-white px-2 py-1 font-mono text-[10px] text-zinc-600 transition-colors hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
              >
                {cmd}
              </button>
            {/each}
          </div>
        {/if}
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
