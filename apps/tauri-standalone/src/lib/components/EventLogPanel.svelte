<script lang="ts">
  import { fade } from "svelte/transition";
  import ScrollText from "@lucide/svelte/icons/scroll-text";
  import Search from "@lucide/svelte/icons/search";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import Server from "@lucide/svelte/icons/server";
  import Pause from "@lucide/svelte/icons/pause";
  import Activity from "@lucide/svelte/icons/activity";
  import Terminal from "@lucide/svelte/icons/terminal";
  import Bell from "@lucide/svelte/icons/bell";
  import BarChart3 from "@lucide/svelte/icons/bar-chart-3";
  import Timer from "@lucide/svelte/icons/timer";
  import Layers from "@lucide/svelte/icons/layers";
  import AlertCircle from "@lucide/svelte/icons/alert-circle";
  import HelpCircle from "@lucide/svelte/icons/help-circle";
  import type { McEvent } from "$lib/types.js";
  import { formatEvent, eventIcon, eventColor } from "$lib/events.js";
  import { kindOrder } from "$lib/stats.js";

  let {
    events,
    filteredEvents,
    searchQuery,
    kindFilters,
    logLevel,
    onSearchChange,
    onKindToggle,
    onLogLevelChange,
    onClearLog,
  }: {
    events: McEvent[];
    filteredEvents: McEvent[];
    searchQuery: string;
    kindFilters: Record<McEvent["kind"], boolean>;
    logLevel: "all" | 0 | 1 | 2;
    onSearchChange: (q: string) => void;
    onKindToggle: (kind: McEvent["kind"]) => void;
    onLogLevelChange: (level: "all" | 0 | 1 | 2) => void;
    onClearLog: () => void;
  } = $props();

  let logElement = $state<HTMLDivElement | null>(null);

  $effect(() => {
    filteredEvents.length;
    if (logElement) {
      logElement.scrollTop = logElement.scrollHeight;
    }
  });

  const iconComponents: Record<string, any> = {
    server: Server,
    pause: Pause,
    activity: Activity,
    terminal: Terminal,
    bell: Bell,
    "bar-chart-3": BarChart3,
    timer: Timer,
    layers: Layers,
    "alert-circle": AlertCircle,
    "help-circle": HelpCircle,
  };

  const logLevels = [
    { label: "All", value: "all" },
    { label: "LOG", value: 0 },
    { label: "WARN", value: 1 },
    { label: "ERROR", value: 2 },
  ] as const;
</script>

<div class="flex min-h-0 flex-1 flex-col p-4">
  <section class="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-zinc-200 bg-white shadow-sm dark:border-zinc-800 dark:bg-zinc-950">
    <div class="flex flex-col gap-3 border-b border-zinc-200 px-4 py-3 dark:border-zinc-800">
      <div class="flex items-center justify-between">
        <div class="flex items-center gap-2">
          <ScrollText class="h-4 w-4 text-zinc-500 dark:text-zinc-400" />
          <h2 class="text-sm font-semibold text-zinc-900 dark:text-zinc-100">Event Log</h2>
        </div>
        <div class="flex items-center gap-3">
          <span class="text-xs text-zinc-500 dark:text-zinc-400">{filteredEvents.length} of {events.length} events</span>
          <button
            type="button"
            onclick={onClearLog}
            disabled={events.length === 0}
            class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium text-zinc-600 transition-colors hover:bg-zinc-100 hover:text-zinc-900 disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
          >
            <Trash2 class="h-3.5 w-3.5" />
            Clear
          </button>
        </div>
      </div>

      <div class="flex flex-col gap-2 sm:flex-row">
        <div class="relative flex-1">
          <Search class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400 dark:text-zinc-500" />
          <input
            type="text"
            value={searchQuery}
            oninput={(e) => onSearchChange(e.currentTarget.value)}
            placeholder="Search events..."
            class="w-full rounded-md border border-zinc-300 bg-white py-1.5 pl-8 pr-3 text-xs text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
          />
        </div>
        <div class="flex rounded-md bg-zinc-100 p-0.5 dark:bg-zinc-800">
          {#each logLevels as level}
            <button
              type="button"
              onclick={() => onLogLevelChange(level.value as typeof logLevel)}
              class="px-2.5 py-1 text-[10px] font-semibold uppercase transition-colors {logLevel === level.value
                ? 'rounded-md bg-white text-indigo-600 shadow-sm dark:bg-zinc-700 dark:text-indigo-400'
                : 'text-zinc-500 hover:text-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-300'}"
            >
              {level.label}
            </button>
          {/each}
        </div>
      </div>

      <div class="flex flex-wrap gap-1.5">
        {#each kindOrder as kind}
          {@const Icon = iconComponents[eventIcon(kind)]}
          <button
            type="button"
            onclick={() => onKindToggle(kind)}
            class="flex items-center gap-1 rounded-full border px-2 py-1 text-[10px] font-medium uppercase tracking-wide transition-colors {kindFilters[kind]
              ? 'border-indigo-200 bg-indigo-50 text-indigo-700 dark:border-indigo-900/50 dark:bg-indigo-950/30 dark:text-indigo-300'
              : 'border-zinc-200 bg-white text-zinc-500 opacity-70 hover:opacity-100 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-500'}"
          >
            <Icon class="h-3 w-3 {kindFilters[kind] ? eventColor(kind) : 'text-zinc-400 dark:text-zinc-500'}" />
            {kind}
          </button>
        {/each}
      </div>
    </div>

    <div bind:this={logElement} class="flex-1 overflow-y-auto p-2">
      {#if events.length === 0}
        <div class="flex h-full flex-col items-center justify-center text-zinc-400 dark:text-zinc-600">
          <ScrollText class="mb-2 h-8 w-8 opacity-50" />
          <p class="text-sm">No events yet.</p>
        </div>
      {:else if filteredEvents.length === 0}
        <div class="flex h-full flex-col items-center justify-center text-zinc-400 dark:text-zinc-600">
          <Search class="mb-2 h-8 w-8 opacity-50" />
          <p class="text-sm">No events match the current filter.</p>
        </div>
      {:else}
        <ul class="space-y-0.5 font-mono text-xs">
          {#each filteredEvents as event, i (i)}
            {@const Icon = iconComponents[eventIcon(event.kind)]}
            <li class="flex items-start gap-2 rounded-md px-2 py-1.5 transition-colors hover:bg-zinc-50 dark:hover:bg-zinc-900">
              <Icon class="mt-0.5 h-3.5 w-3.5 shrink-0 {eventColor(event.kind)}" />
              <span class="break-all text-zinc-700 dark:text-zinc-300">{formatEvent(event)}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </section>
</div>
