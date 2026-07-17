<script lang="ts">
  import Pause from "@lucide/svelte/icons/pause";
  import Play from "@lucide/svelte/icons/play";
  import SkipForward from "@lucide/svelte/icons/skip-forward";
  import CornerDownRight from "@lucide/svelte/icons/corner-down-right";
  import CornerUpRight from "@lucide/svelte/icons/corner-up-right";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  let {
    connected,
    stopped,
    stopReason,
    busy,
    onPause,
    onContinue,
    onStepNext,
    onStepIn,
    onStepOut,
  }: {
    connected: boolean;
    stopped: boolean;
    stopReason: string;
    busy: boolean;
    onPause: () => void;
    onContinue: () => void;
    onStepNext: () => void;
    onStepIn: () => void;
    onStepOut: () => void;
  } = $props();
</script>

{#if connected}
  <div class="flex items-center gap-1 border-b border-zinc-200 bg-white px-3 py-2 dark:border-zinc-800 dark:bg-zinc-950">
    {#if stopped}
      <span class="mr-2 flex items-center gap-1.5 rounded-md bg-amber-50 px-2 py-1 text-xs font-medium text-amber-700 dark:bg-amber-950/30 dark:text-amber-300">
        <Pause class="h-3.5 w-3.5" />
        Paused: {stopReason}
      </span>
    {/if}

    {#if !stopped && !busy}
      <button
        type="button"
        onclick={onPause}
        class="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-zinc-600 transition-colors hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
      >
        <Pause class="h-4 w-4" />
        Pause
      </button>
    {/if}

    {#if stopped && !busy}
      <button
        type="button"
        onclick={onContinue}
        class="flex items-center gap-1.5 rounded-md bg-indigo-600 px-2.5 py-1.5 text-xs font-medium text-white shadow-sm transition-colors hover:bg-indigo-700 dark:bg-indigo-600 dark:hover:bg-indigo-700"
      >
        <Play class="h-4 w-4" />
        Continue
      </button>
      <button
        type="button"
        onclick={onStepNext}
        class="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-zinc-600 transition-colors hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
      >
        <SkipForward class="h-4 w-4" />
        Next
      </button>
      <button
        type="button"
        onclick={onStepIn}
        class="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-zinc-600 transition-colors hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
      >
        <CornerDownRight class="h-4 w-4" />
        In
      </button>
      <button
        type="button"
        onclick={onStepOut}
        class="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-zinc-600 transition-colors hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
      >
        <CornerUpRight class="h-4 w-4" />
        Out
      </button>
    {/if}

    {#if busy}
      <LoaderCircle class="h-4 w-4 animate-spin text-zinc-500 dark:text-zinc-400" />
    {/if}
  </div>
{/if}
