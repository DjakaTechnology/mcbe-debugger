<script lang="ts">
  import BarChart3 from "@lucide/svelte/icons/bar-chart-3";
  import Gauge from "@lucide/svelte/icons/gauge";
  import MemoryStick from "@lucide/svelte/icons/memory-stick";
  import Code from "@lucide/svelte/icons/code";
  import Monitor from "@lucide/svelte/icons/monitor";
  import HelpCircle from "@lucide/svelte/icons/help-circle";
  import UPlotChart from "$lib/UPlotChart.svelte";
  import type { StatSeries, StatCategory } from "$lib/types.js";
  import {
    shortName,
    buildGroupData,
    formatGroupValue,
    formatStatValue,
    isEmptySeries,
    scaleSeriesForDisplay,
    isMemoryGroup,
    formatMemoryValue,
    makeChartOptions,
    getClientId,
  } from "$lib/stats.js";

  let {
    categorizedGroups,
    activeCategory,
    selectedClient,
    clientIds,
    onCategoryChange,
    onClientChange,
  }: {
    categorizedGroups: StatCategory[];
    activeCategory: string;
    selectedClient: string | "all";
    clientIds: string[];
    onCategoryChange: (cat: string) => void;
    onClientChange: (client: string | "all") => void;
  } = $props();

  const categoryIcons: Record<string, any> = {
    "server-performance": Gauge,
    memory: MemoryStick,
    scripting: Code,
    client: Monitor,
    uncategorized: HelpCircle,
  };

  let displayCategories = $derived(
    activeCategory === "all"
      ? categorizedGroups
      : categorizedGroups.filter((c) => c.key === activeCategory),
  );

  let showClientDropdown = $derived(
    (activeCategory === "all" || activeCategory === "client") && clientIds.length > 1,
  );
</script>

<div class="flex min-h-0 flex-1 flex-col p-4">
  {#if categorizedGroups.length > 0}
    <div class="mb-4 flex flex-wrap gap-1 rounded-lg bg-zinc-200/60 p-1 dark:bg-zinc-800/60">
      <button
        type="button"
        onclick={() => onCategoryChange("all")}
        class="flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors {activeCategory === 'all'
          ? 'bg-white text-indigo-600 shadow-sm dark:bg-zinc-700 dark:text-indigo-400'
          : 'text-zinc-600 hover:bg-zinc-200/50 dark:text-zinc-400 dark:hover:bg-zinc-800/50'}"
      >
        All
      </button>
      {#each categorizedGroups as category (category.key)}
        {@const Icon = categoryIcons[category.key] ?? HelpCircle}
        <button
          type="button"
          onclick={() => onCategoryChange(category.key)}
          class="flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors {activeCategory === category.key
            ? 'bg-white text-indigo-600 shadow-sm dark:bg-zinc-700 dark:text-indigo-400'
            : 'text-zinc-600 hover:bg-zinc-200/50 dark:text-zinc-400 dark:hover:bg-zinc-800/50'}"
        >
          <Icon class="h-3 w-3" />
          {category.label}
        </button>
      {/each}
    </div>
  {/if}

  <div class="flex-1 overflow-y-auto pr-1">
    {#if displayCategories.length === 0}
      <div class="flex h-full flex-col items-center justify-center text-zinc-400 dark:text-zinc-600">
        <BarChart3 class="mb-2 h-8 w-8 opacity-50" />
        <p class="text-sm">No stats yet. Make sure your add-on is running.</p>
      </div>
    {:else}
      {#if showClientDropdown}
        <div class="mb-4 flex items-center gap-2">
          <span class="text-xs font-medium text-zinc-500 dark:text-zinc-400">Client</span>
          <select
            value={selectedClient}
            onchange={(e) => onClientChange(e.currentTarget.value as string | "all")}
            class="rounded-md border border-zinc-300 bg-white px-2 py-1 text-xs text-zinc-900 outline-none transition-colors focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
          >
            <option value="all">All clients</option>
            {#each clientIds as clientId}
              <option value={clientId}>{clientId}</option>
            {/each}
          </select>
        </div>
      {/if}

      <div class="grid grid-cols-1 gap-4 lg:grid-cols-2 2xl:grid-cols-3">
        {#each displayCategories as category (category.key)}
          {#each category.groups as group (group.name)}
            {#if group.name === "dynamic_property_values"}
              <div class="group/card rounded-xl border border-zinc-200 bg-white p-4 shadow-sm transition-all hover:border-indigo-200 hover:shadow-md dark:border-zinc-800 dark:bg-zinc-950 dark:hover:border-indigo-900/50">
                <div class="mb-3 flex items-start justify-between gap-3">
                  <div class="min-w-0">
                    <h3 class="truncate text-xs font-semibold uppercase tracking-wide text-zinc-700 dark:text-zinc-300">{group.name}</h3>
                    <p class="truncate text-[10px] text-zinc-500 dark:text-zinc-400">{group.series.length} properties</p>
                  </div>
                </div>
                <div class="overflow-hidden rounded-lg border border-zinc-200 dark:border-zinc-800">
                  <table class="w-full text-left text-xs">
                    <thead class="bg-zinc-50 text-zinc-500 dark:bg-zinc-900 dark:text-zinc-400">
                      <tr>
                        <th class="px-3 py-2 font-medium">Property</th>
                        <th class="px-3 py-2 text-right font-medium">Value</th>
                      </tr>
                    </thead>
                    <tbody class="divide-y divide-zinc-200 dark:divide-zinc-800">
                      {#each group.series as series (series.path)}
                        {@const rawVal = series.values[series.values.length - 1]}
                        <tr class="transition-colors hover:bg-zinc-50 dark:hover:bg-zinc-900">
                          <td class="px-3 py-2 font-medium text-zinc-700 dark:text-zinc-300">{shortName(series.path)}</td>
                          <td class="px-3 py-2 text-right font-mono text-zinc-600 dark:text-zinc-400">
                            {#if rawVal === undefined || rawVal === null}
                              —
                            {:else if typeof rawVal === "number"}
                              {formatStatValue(rawVal)}
                            {:else}
                              {String(rawVal)}
                            {/if}
                          </td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              </div>
            {:else}
              {@const activeSeries = group.series.filter((s) => !isEmptySeries(s))}
              {@const filteredSeries = category.key === "client" && selectedClient !== "all" ? activeSeries.filter((s) => getClientId(s.path) === selectedClient) : activeSeries}
              {@const displaySeries = filteredSeries.map((s) => scaleSeriesForDisplay(s, group.name))}
              {#if displaySeries.length > 0}
                {@const seriesNames = displaySeries.map((s) => shortName(s.path))}
                {@const groupData = buildGroupData(displaySeries)}
                {@const rawLastVal = filteredSeries[0]?.values[filteredSeries[0]?.values.length - 1]}
                <div class="group/card rounded-xl border border-zinc-200 bg-white p-4 shadow-sm transition-all hover:border-indigo-200 hover:shadow-md dark:border-zinc-800 dark:bg-zinc-950 dark:hover:border-indigo-900/50">
                  <div class="mb-3 flex items-start justify-between gap-3">
                    <div class="min-w-0">
                      <h3 class="truncate text-xs font-semibold uppercase tracking-wide text-zinc-700 dark:text-zinc-300">{group.name}</h3>
                      <p class="truncate text-[10px] text-zinc-500 dark:text-zinc-400">
                        {#if displaySeries.length === 1}
                          {shortName(displaySeries[0].path)}
                        {:else}
                          {displaySeries.length} series
                        {/if}
                      </p>
                    </div>
                    {#if displaySeries.length === 1}
                      <span class="shrink-0 rounded-full bg-indigo-50 px-2 py-0.5 font-mono text-xs font-semibold text-indigo-600 dark:bg-indigo-950/30 dark:text-indigo-400">
                        {formatGroupValue(group.name, rawLastVal)}
                      </span>
                    {:else}
                      <span class="shrink-0 rounded-full bg-zinc-100 px-2 py-0.5 font-mono text-xs font-semibold text-zinc-600 dark:bg-zinc-800 dark:text-zinc-400">
                        {displaySeries.length}
                      </span>
                    {/if}
                  </div>
                  <div class="relative overflow-hidden rounded-lg bg-zinc-50/50 text-zinc-500 dark:bg-zinc-900/50 dark:text-zinc-400">
                    <UPlotChart options={makeChartOptions(group.name, seriesNames)} data={groupData} formatValue={isMemoryGroup(group.name) ? formatMemoryValue : undefined} />
                  </div>
                </div>
              {/if}
            {/if}
          {/each}
        {/each}
      </div>
    {/if}
  </div>
</div>
