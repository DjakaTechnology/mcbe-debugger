<script lang="ts">
  import uPlot from "uplot";
  import "uplot/dist/uPlot.min.css";

  let { options, data }: { options: any; data: any } = $props();
  let el = $state<HTMLDivElement>();
  let chart: uPlot | null = null;

  $effect(() => {
    if (!el) return;
    chart = new uPlot(options, data, el);
    return () => {
      chart?.destroy();
      chart = null;
    };
  });

  $effect(() => {
    if (chart && data) {
      chart.setData(data);
    }
  });
</script>

<div bind:this={el} class="w-full"></div>
