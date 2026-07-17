<script lang="ts">
  import uPlot from "uplot";
  import "uplot/dist/uPlot.min.css";

  let { options, data, formatValue }: { options: any; data: any; formatValue?: (val: number) => string } = $props();
  let el = $state<HTMLDivElement>();
  let tooltipEl = $state<HTMLDivElement>();
  let chart: uPlot | null = null;
  let lastCursorLeft = $state<number | null>(null);
  let mouseInside = $state(false);

  $effect(() => {
    if (!el) return;

    const handleSetCursor = (self: uPlot) => {
      if (!tooltipEl) return;
      const idx = self.cursor.idx;
      if (idx === null || idx === undefined) {
        tooltipEl.style.display = "none";
        return;
      }
      lastCursorLeft = self.cursor.left ?? null;
      const xVal = self.data[0]?.[idx];
      let html = `<div style="opacity:0.5">tick ${xVal ?? ""}</div>`;
      for (let i = 1; i < self.data.length; i++) {
        const s = self.series[i];
        if (!s?.show) continue;
        const val = self.data[i]?.[idx];
        const label = s.label || `s${i}`;
        const stroke = typeof s.stroke === "string" ? s.stroke : "#999";
        if (val !== null && val !== undefined) {
          const formatted =
            typeof val === "number"
              ? formatValue
                ? formatValue(val)
                : Math.abs(val) >= 1000
                  ? val.toFixed(0)
                  : val.toFixed(2)
              : String(val);
          html += `<div style="color:${stroke}">${label}: ${formatted}</div>`;
        }
      }
      tooltipEl.innerHTML = html;
      tooltipEl.style.display = "block";
      const left = self.cursor.left || 0;
      const cw = self.width || 340;
      tooltipEl.style.left = `${left + 12 > cw - 140 ? left - 140 : left + 12}px`;
      tooltipEl.style.top = "4px";
    };

    const merged: any = {
      ...options,
      width: el.clientWidth || 340,
      hooks: {
        ...(options.hooks || {}),
        setCursor: [...(options.hooks?.setCursor || []), handleSetCursor],
      },
    };

    chart = new uPlot(merged, data, el);

    const ro = new ResizeObserver(() => {
      if (chart && el) {
        chart.setSize({ width: el.clientWidth, height: chart.height });
      }
    });
    ro.observe(el);

    return () => {
      ro.disconnect();
      chart?.destroy();
      chart = null;
    };
  });

  $effect(() => {
    if (chart && data) {
      chart.setData(data);
      if (lastCursorLeft !== null && mouseInside) {
        chart.setCursor({ left: lastCursorLeft, top: 0 });
      }
    }
  });
</script>

<div
  role="img"
  class="relative w-full"
  onmouseenter={() => (mouseInside = true)}
  onmouseleave={() => {
    mouseInside = false;
    lastCursorLeft = null;
    if (tooltipEl) tooltipEl.style.display = "none";
  }}
>
  <div bind:this={el}></div>
  <div
    bind:this={tooltipEl}
    class="pointer-events-none absolute z-10 hidden rounded-md bg-zinc-900/90 px-2 py-1.5 text-[10px] font-mono leading-relaxed text-zinc-100 shadow-lg ring-1 ring-white/10 dark:bg-zinc-800/95"
  ></div>
</div>
