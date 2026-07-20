<script lang="ts">
  import { fade } from "svelte/transition";
  import ScrollText from "@lucide/svelte/icons/scroll-text";
  import Search from "@lucide/svelte/icons/search";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import Filter from "@lucide/svelte/icons/filter";
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
  import MapPin from "@lucide/svelte/icons/map-pin";
  import FileCode from "@lucide/svelte/icons/file-code";
  import type { LogLevel, McEvent, SourceFrame } from "$lib/types.js";
  import {
    formatEvent,
    eventIcon,
    eventColor,
    frameLabel,
    frameTitle,
    getEventFrames,
    frameVisibleInMessage,
  } from "$lib/events.js";
  import { kindOrder } from "$lib/stats.js";

  let {
    events,
    filteredEvents,
    searchQuery,
    kindFilters,
    logLevel,
    onSearchChange,
    onKindToggle,
    onClearEventKinds,
    onLogLevelChange,
    onClearLog,
    onResetFilters,
  }: {
    events: McEvent[];
    filteredEvents: McEvent[];
    searchQuery: string;
    kindFilters: Record<McEvent["kind"], boolean>;
    logLevel: "all" | LogLevel;
    onSearchChange: (q: string) => void;
    onKindToggle: (kind: McEvent["kind"]) => void;
    onClearEventKinds: () => void;
    onLogLevelChange: (level: "all" | LogLevel) => void;
    onClearLog: () => void;
    onResetFilters: () => void;
  } = $props();

  // Subordinate source-frame rows for an event. The raw message is always
  // preserved verbatim; a frame is suppressed only when the message already
  // shows that same frame's location (mapped source path for mapped frames,
  // generated path for unmapped frames). Function names are never used as a
  // suppression key, so a mapped original source row still surfaces even when
  // the raw stack mentions the function name.
  function visibleFramesFor(event: McEvent): SourceFrame[] {
    if (event.kind !== "print" && event.kind !== "notification") return [];
    const frames = getEventFrames(event);
    if (frames.length === 0) return [];
    return frames.filter((f) => frameVisibleInMessage(f, event.message));
  }

  let logElement = $state<HTMLDivElement | null>(null);
  let filterOpen = $state(false);
  let filterContainer = $state<HTMLDivElement | null>(null);

  $effect(() => {
    filteredEvents.length;
    if (logElement) {
      logElement.scrollTop = logElement.scrollHeight;
    }
  });

  function handleWindowClick(event: MouseEvent) {
    if (filterOpen && filterContainer && !filterContainer.contains(event.target as Node)) {
      filterOpen = false;
    }
  }

  $effect(() => {
    window.addEventListener("click", handleWindowClick);
    return () => window.removeEventListener("click", handleWindowClick);
  });

  let activeFilterCount = $derived.by(() => {
    let count = 0;
    if (searchQuery.trim()) count++;
    const kindValues = Object.values(kindFilters);
    if (kindValues.some(Boolean) && kindValues.some((value) => !value)) count++;
    if (logLevel !== "all") count++;
    return count;
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
    { label: "Verbose", value: 0 },
    { label: "Log", value: 1 },
    { label: "Warn", value: 2 },
    { label: "Error", value: 3 },
    { label: "Stop", value: 4 },
  ] as const;

  const eventLogKinds = kindOrder.filter((kind) => kind !== "stat2");
</script>

<div class="flex min-h-0 flex-1 flex-col p-4">
  <section class="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-zinc-200 bg-white shadow-sm dark:border-zinc-800 dark:bg-zinc-950">
    <div class="flex items-center justify-between border-b border-zinc-200 px-4 py-3 dark:border-zinc-800">
      <div class="flex items-center gap-2">
        <ScrollText class="h-4 w-4 text-zinc-500 dark:text-zinc-400" />
        <h2 class="text-sm font-semibold text-zinc-900 dark:text-zinc-100">Event Log</h2>
      </div>
      <div class="flex items-center gap-2">
        <span class="text-xs text-zinc-500 dark:text-zinc-400">{filteredEvents.length} of {events.length} events</span>

        <div bind:this={filterContainer} class="relative">
          <button
            type="button"
            onclick={() => (filterOpen = !filterOpen)}
            class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium transition-colors {activeFilterCount > 0
              ? 'bg-indigo-50 text-indigo-600 dark:bg-indigo-950/30 dark:text-indigo-400'
              : 'text-zinc-600 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100'}"
          >
            <Filter class="h-3.5 w-3.5" />
            Filter
            {#if activeFilterCount > 0}
              <span class="ml-0.5 rounded-full bg-indigo-600 px-1.5 py-0.5 text-[9px] font-semibold text-white dark:bg-indigo-500">{activeFilterCount}</span>
            {/if}
          </button>

          {#if filterOpen}
            <div
              transition:fade={{ duration: 100 }}
              class="absolute right-0 top-full z-20 mt-1.5 w-80 rounded-xl border border-zinc-200 bg-white p-3 shadow-lg dark:border-zinc-800 dark:bg-zinc-950"
            >
              <div class="mb-2 border-b border-zinc-100 pb-2 dark:border-zinc-800">
                <span class="text-xs font-semibold uppercase tracking-wide text-zinc-500 dark:text-zinc-400">Filters</span>
              </div>

              <div class="space-y-3">
                <div class="relative">
                  <Search class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400 dark:text-zinc-500" />
                  <input
                    type="text"
                    value={searchQuery}
                    oninput={(e) => onSearchChange(e.currentTarget.value)}
                    placeholder="Search events..."
                    class="w-full rounded-md border border-zinc-300 bg-white py-1.5 pl-8 pr-3 text-xs text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
                  />
                </div>

                <div class="space-y-1.5">
                  <span class="block text-[10px] font-medium uppercase tracking-wide text-zinc-500 dark:text-zinc-400">Log level</span>
                  <div class="flex rounded-md bg-zinc-100 p-0.5 dark:bg-zinc-800">
                    {#each logLevels as level}
                      <button
                        type="button"
                        onclick={() => onLogLevelChange(level.value as typeof logLevel)}
                        class="flex-1 px-1 py-1 text-[9px] font-semibold uppercase transition-colors {logLevel === level.value
                          ? 'rounded-md bg-white text-indigo-600 shadow-sm dark:bg-zinc-700 dark:text-indigo-400'
                          : 'text-zinc-500 hover:text-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-300'}"
                      >
                        {level.label}
                      </button>
                    {/each}
                  </div>
                </div>

                <div class="space-y-1.5">
                  <div class="flex items-center justify-between">
                    <span class="text-[10px] font-medium uppercase tracking-wide text-zinc-500 dark:text-zinc-400">Event kinds</span>
                    <button
                      type="button"
                      onclick={onClearEventKinds}
                      disabled={!Object.values(kindFilters).some(Boolean)}
                      aria-label="Clear event kinds"
                      title="No selected kinds shows all events"
                      class="text-[10px] font-medium text-indigo-600 transition-colors hover:text-indigo-700 disabled:cursor-not-allowed disabled:text-zinc-300 dark:text-indigo-400 dark:hover:text-indigo-300 dark:disabled:text-zinc-700"
                    >
                      Clear all
                    </button>
                  </div>
                  <div class="flex flex-wrap gap-1.5">
                    {#each eventLogKinds as kind}
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
              </div>

              <div class="mt-3 border-t border-zinc-100 pt-2 dark:border-zinc-800">
                <button
                  type="button"
                  onclick={onResetFilters}
                  class="w-full rounded-md py-1.5 text-xs font-medium text-indigo-600 transition-colors hover:bg-indigo-50 dark:text-indigo-400 dark:hover:bg-indigo-950/30"
                >
                  Reset filters
                </button>
              </div>
            </div>
          {/if}
        </div>

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
            {@const frames = visibleFramesFor(event)}
            <li class="rounded-md px-2 py-1.5 transition-colors hover:bg-zinc-50 dark:hover:bg-zinc-900">
              <div class="flex items-start gap-2">
                <Icon class="mt-0.5 h-3.5 w-3.5 shrink-0 {eventColor(event.kind)}" />
                <div class="min-w-0 flex-1">
                  <span class="break-all text-zinc-700 dark:text-zinc-300">{formatEvent(event)}</span>
                  {#if frames.length > 0}
                    <ul
                      class="mt-1 space-y-0.5 border-l border-zinc-200 pl-2 dark:border-zinc-700"
                      aria-label="Source frames"
                    >
                      {#each frames as frame, fi (fi)}
                        <li class="flex items-start gap-1.5 text-[11px] leading-snug">
                          {#if frame.mapped}
                            <MapPin
                              class="mt-px h-3 w-3 shrink-0 text-emerald-500 dark:text-emerald-400"
                              aria-label="Mapped source frame"
                            />
                          {:else}
                            <FileCode
                              class="mt-px h-3 w-3 shrink-0 text-zinc-400 dark:text-zinc-500"
                              aria-label="Unmapped generated frame"
                            />
                          {/if}
                          <span
                            class="block min-w-0 flex-1 truncate {frame.mapped
                              ? 'text-zinc-600 dark:text-zinc-300'
                              : 'text-zinc-500 dark:text-zinc-400'}"
                            title={frameTitle(frame)}
                          >
                            {frameLabel(frame)}
                          </span>
                        </li>
                      {/each}
                    </ul>
                  {/if}
                </div>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </section>
</div>
