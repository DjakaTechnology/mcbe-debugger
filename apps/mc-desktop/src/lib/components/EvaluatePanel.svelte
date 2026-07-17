<script lang="ts">
  import { slide, fade } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import Terminal from "@lucide/svelte/icons/terminal";
  import Send from "@lucide/svelte/icons/send";
  import type { ResponsePayload } from "$lib/types.js";

  let {
    evalExpression,
    evalHistory,
    busy,
    onEvaluate,
    onExpressionChange,
  }: {
    evalExpression: string;
    evalHistory: { expression: string; result: ResponsePayload }[];
    busy: boolean;
    onEvaluate: () => void;
    onExpressionChange: (expr: string) => void;
  } = $props();
</script>

<div transition:slide={{ duration: 200, easing: cubicOut }} class="border-t border-zinc-200 bg-white p-3 dark:border-zinc-800 dark:bg-zinc-950">
  <div class="flex items-start gap-2">
    <div class="relative flex-1">
      <Terminal class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400 dark:text-zinc-500" />
      <input
        type="text"
        value={evalExpression}
        oninput={(e) => onExpressionChange(e.currentTarget.value)}
        onkeydown={(e) => e.key === "Enter" && onEvaluate()}
        placeholder="Evaluate expression…"
        disabled={busy}
        class="w-full rounded-md border border-zinc-300 bg-white py-2 pl-8 pr-3 text-sm text-zinc-900 outline-none transition-colors placeholder:text-zinc-400 focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/20 disabled:cursor-not-allowed disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-100"
      />
    </div>
    <button
      type="button"
      onclick={onEvaluate}
      disabled={busy || !evalExpression.trim()}
      class="flex items-center gap-1.5 rounded-md bg-indigo-600 px-3 py-2 text-sm font-medium text-white shadow-sm transition-colors hover:bg-indigo-700 disabled:cursor-not-allowed disabled:bg-indigo-600 disabled:opacity-60 dark:bg-indigo-600 dark:hover:bg-indigo-700"
    >
      <Send class="h-4 w-4" />
      Eval
    </button>
  </div>

  {#if evalHistory.length > 0}
    <div transition:fade={{ duration: 150 }} class="mt-3 space-y-2">
      {#each evalHistory as item, i (i)}
        <div class="rounded-md border border-zinc-200 bg-zinc-50 p-2 text-xs dark:border-zinc-800 dark:bg-zinc-900">
          <div class="mb-1 flex items-center justify-between">
            <code class="font-mono font-medium text-zinc-700 dark:text-zinc-300">{item.expression}</code>
            {#if item.result.success}
              <span class="rounded-full bg-emerald-100 px-1.5 py-0.5 text-[10px] font-medium text-emerald-700 dark:bg-emerald-950/30 dark:text-emerald-300">OK</span>
            {:else}
              <span class="rounded-full bg-rose-100 px-1.5 py-0.5 text-[10px] font-medium text-rose-700 dark:bg-rose-950/30 dark:text-rose-300">ERR</span>
            {/if}
          </div>
          {#if item.result.success}
            <pre class="overflow-x-auto rounded bg-white p-1.5 font-mono text-[10px] text-zinc-600 dark:bg-zinc-950 dark:text-zinc-400">{JSON.stringify(item.result.args, null, 2)}</pre>
          {:else}
            <p class="text-rose-600 dark:text-rose-400">{item.result.message ?? "Evaluation failed"}</p>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
