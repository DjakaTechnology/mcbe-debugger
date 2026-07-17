<script lang="ts">
  import { onMount } from "svelte";
  import X from "@lucide/svelte/icons/x";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import AlertCircle from "@lucide/svelte/icons/alert-circle";
  import type { PluginInfo } from "$lib/types.js";

  let {
    plugins,
    selecting,
    selectionError,
    onSelect,
    onCancel,
  }: {
    plugins: PluginInfo[];
    selecting: boolean;
    selectionError: string | null;
    onSelect: (plugin: PluginInfo) => void;
    onCancel: () => void;
  } = $props();

  let selectedUuid = $state<string | null>(null);
  let modalRef = $state<HTMLDivElement | null>(null);

  // Reset selection whenever the plugin list changes
  $effect(() => {
    plugins;
    selectedUuid = null;
  });

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      onCancel();
    }
  }

  function handleConfirm() {
    const plugin = plugins.find((p) => p.module_uuid === selectedUuid);
    if (plugin) onSelect(plugin);
  }

  onMount(() => {
    const firstRadio = modalRef?.querySelector<HTMLInputElement>('input[type="radio"]');
    if (firstRadio) {
      firstRadio.focus();
    } else {
      modalRef?.focus();
    }
    window.addEventListener("keydown", handleKeydown);
    return () => window.removeEventListener("keydown", handleKeydown);
  });
</script>

<!-- Backdrop: not clickable, so backend handshake is not stranded -->
<div class="fixed inset-0 z-40 bg-zinc-900/40 backdrop-blur-sm" aria-hidden="true"></div>

<div
  bind:this={modalRef}
  tabindex="-1"
  role="dialog"
  aria-modal="true"
  aria-labelledby="target-selection-title"
  class="fixed inset-0 z-50 flex items-center justify-center p-4 outline-none"
>
  <div class="flex w-full max-w-lg flex-col overflow-hidden rounded-xl border border-zinc-200 bg-white shadow-xl dark:border-zinc-800 dark:bg-zinc-950">
    <div class="flex items-center justify-between border-b border-zinc-200 px-4 py-3 dark:border-zinc-800">
      <h2 id="target-selection-title" class="text-sm font-semibold text-zinc-900 dark:text-zinc-100">Select a script module</h2>
      <button
        type="button"
        onclick={onCancel}
        disabled={selecting}
        aria-label="Cancel module selection"
        class="flex h-6 w-6 items-center justify-center rounded-md text-zinc-400 transition-colors hover:bg-zinc-100 hover:text-zinc-600 disabled:cursor-not-allowed disabled:opacity-40 dark:text-zinc-500 dark:hover:bg-zinc-800 dark:hover:text-zinc-300"
      >
        <X class="h-4 w-4" />
      </button>
    </div>

    <div class="px-4 py-3">
      <p class="text-xs text-zinc-500 dark:text-zinc-400">Minecraft reported multiple modules. Choose which one to debug.</p>
    </div>

    <div class="max-h-[300px] overflow-y-auto px-4 pb-2">
      <div class="space-y-1.5" role="radiogroup" aria-label="Available modules">
        {#each plugins as plugin (plugin.module_uuid)}
          {@const checked = selectedUuid === plugin.module_uuid}
          <label
            class="flex cursor-pointer items-center gap-3 rounded-lg border px-3 py-2.5 transition-colors {checked
              ? 'border-indigo-500 bg-indigo-50 dark:border-indigo-400 dark:bg-indigo-950/30'
              : 'border-zinc-200 hover:bg-zinc-50 dark:border-zinc-800 dark:hover:bg-zinc-900'}"
          >
            <input
              type="radio"
              name="target-module"
              value={plugin.module_uuid}
              bind:group={selectedUuid}
              disabled={selecting}
              class="h-4 w-4 cursor-pointer border-zinc-300 text-indigo-600 focus:ring-indigo-500 focus:ring-offset-0 disabled:cursor-not-allowed dark:border-zinc-600 dark:bg-zinc-800 dark:text-indigo-400"
            />
            <div class="min-w-0 flex-1">
              <div class="text-xs font-semibold text-zinc-900 dark:text-zinc-100">{plugin.name}</div>
              <div class="break-all font-mono text-[10px] leading-snug text-zinc-500 dark:text-zinc-400">{plugin.module_uuid}</div>
            </div>
          </label>
        {/each}
      </div>
    </div>

    {#if selectionError}
      <div class="flex items-start gap-2 px-4 py-2 text-xs text-rose-700 dark:text-rose-300">
        <AlertCircle class="mt-0.5 h-4 w-4 shrink-0" />
        <span class="break-words">{selectionError}</span>
      </div>
    {/if}

    <div class="flex items-center justify-end gap-2 border-t border-zinc-200 px-4 py-3 dark:border-zinc-800">
      <button
        type="button"
        onclick={onCancel}
        disabled={selecting}
        class="rounded-md px-3 py-1.5 text-xs font-medium text-zinc-600 transition-colors hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-50 dark:text-zinc-400 dark:hover:bg-zinc-800"
      >
        Cancel
      </button>
      <button
        type="button"
        onclick={handleConfirm}
        disabled={!selectedUuid || selecting}
        class="flex min-w-[8rem] items-center justify-center gap-1.5 rounded-md bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-indigo-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-indigo-600 dark:hover:bg-indigo-700"
      >
        {#if selecting}
          <LoaderCircle class="h-3.5 w-3.5 animate-spin" />
          Selecting...
        {:else}
          Use selected module
        {/if}
      </button>
    </div>
  </div>
</div>
