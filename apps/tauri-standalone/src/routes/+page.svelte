<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

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

  let connecting = $state(false);
  let connected = $state(false);
  let handshake = $state<HandshakeInfo | null>(null);
  let error = $state<string | null>(null);
  let events = $state<McEvent[]>([]);
  let logElement = $state<HTMLDivElement | null>(null);

  onMount(() => {
    let unlistens: Array<() => void> = [];
    let cancelled = false;

    Promise.all([
      listen<McEvent>("mc-event", (e) => {
        events = [...events, e.payload].slice(-500);
      }),
      listen("mc-disconnected", () => {
        connected = false;
        handshake = null;
      }),
      listen("mc-terminated", () => {
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

  function logLevelName(level: number): string {
    return level === 0 ? "LOG" : level === 1 ? "WARN" : "ERROR";
  }
</script>

<main>
  <header>
    <h1>Minecraft Debugger</h1>
    <p class="subtitle">Standalone</p>
  </header>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  <section class="panel">
    <h2>Connection</h2>
    <div class="mode-toggle">
      <label>
        <input type="radio" bind:group={mode} value="listen" disabled={connecting || connected} />
        Listen (MC client connects to us)
      </label>
      <label>
        <input type="radio" bind:group={mode} value="connect" disabled={connecting || connected} />
        Connect (we connect to BDS)
      </label>
    </div>

    <form onsubmit={(e) => e.preventDefault()}>
      {#if mode === "connect"}
        <label>
          Host
          <input type="text" bind:value={host} disabled={connecting || connected} />
        </label>
      {/if}
      <label>
        Port
        <input type="number" bind:value={port} disabled={connecting || connected} />
      </label>
      <details>
        <summary>Advanced</summary>
        <label>
          Target module UUID (required if multiple plugins)
          <input type="text" bind:value={targetModuleUuid} disabled={connecting || connected} />
        </label>
        <label>
          Passcode (only if MC requires one)
          <input type="text" bind:value={passcode} disabled={connecting || connected} />
        </label>
      </details>

      <div class="actions">
        {#if !connected}
          <button type="submit" onclick={handleConnect} disabled={connecting}>
            {connecting ? (mode === "listen" ? "Waiting for MC..." : "Connecting...") : mode === "listen" ? "Listen" : "Connect"}
          </button>
        {:else}
          <button type="button" onclick={handleDisconnect}>Disconnect</button>
        {/if}
      </div>
    </form>
  </section>

  {#if handshake}
    <section class="panel">
      <h2>Handshake</h2>
      <div class="handshake-grid">
        <span>Protocol version:</span><code>v{handshake.version}</code>
        <span>Plugins:</span>
        <span>
          {#if handshake.plugins.length === 0}
            <em>none</em>
          {:else}
            {#each handshake.plugins as p}
              <div><code>{p.module_uuid}</code> ({p.name})</div>
            {/each}
          {/if}
        </span>
        <span>Require passcode:</span><code>{handshake.requirePasscode}</code>
      </div>
    </section>
  {/if}

  <section class="panel">
    <div class="log-header">
      <h2>Event Log</h2>
      <button type="button" onclick={clearLog} disabled={events.length === 0}>Clear</button>
    </div>
    <div class="log" bind:this={logElement}>
      {#if events.length === 0}
        <p class="empty">No events yet.</p>
      {:else}
        {#each events as event}
          <pre>{formatEvent(event)}</pre>
        {/each}
      {/if}
    </div>
  </section>
</main>

<style>
  :root {
    font-family: Inter, system-ui, sans-serif;
    color-scheme: light dark;
  }

  :global(body) {
    margin: 0;
    background: #f6f6f6;
    color: #0f0f0f;
  }

  @media (prefers-color-scheme: dark) {
    :global(body) {
      background: #1e1e1e;
      color: #f6f6f6;
    }
  }

  main {
    max-width: 820px;
    margin: 0 auto;
    padding: 1.5rem;
  }

  header h1 {
    margin: 0;
    font-size: 1.6rem;
  }

  .subtitle {
    margin: 0.2rem 0 0;
    color: #888;
    font-size: 0.9rem;
  }

  .error {
    color: #c33;
    background: rgba(204, 51, 51, 0.1);
    padding: 0.5rem 0.75rem;
    border-radius: 6px;
    margin: 1rem 0;
    font-family: monospace;
  }

  .panel {
    margin-top: 1.25rem;
    padding: 1rem 1.25rem;
    border: 1px solid #ddd;
    border-radius: 8px;
    background: rgba(127, 127, 127, 0.05);
  }

  .panel h2 {
    margin: 0 0 0.75rem;
    font-size: 1.05rem;
  }

  .mode-toggle {
    display: flex;
    gap: 1.5rem;
    margin-bottom: 0.75rem;
    font-size: 0.9rem;
  }

  .mode-toggle label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  form {
    display: grid;
    gap: 0.6rem;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.85rem;
  }

  input[type="text"],
  input[type="number"] {
    padding: 0.45rem 0.65rem;
    border: 1px solid #ccc;
    border-radius: 6px;
    font: inherit;
    background: inherit;
    color: inherit;
  }

  input:disabled {
    opacity: 0.5;
  }

  details {
    margin-top: 0.25rem;
  }

  summary {
    cursor: pointer;
    font-size: 0.85rem;
    color: #666;
  }

  .actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }

  button {
    padding: 0.5rem 1rem;
    border: 1px solid #396cd8;
    border-radius: 6px;
    background: #396cd8;
    color: white;
    cursor: pointer;
    font: inherit;
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .handshake-grid {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.4rem 1rem;
    font-size: 0.9rem;
  }

  code {
    font-family: ui-monospace, "SF Mono", Consolas, monospace;
    background: rgba(127, 127, 127, 0.1);
    padding: 0.1rem 0.35rem;
    border-radius: 3px;
    font-size: 0.85em;
  }

  .log-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 0.5rem;
  }

  .log-header h2 {
    margin: 0;
  }

  .log-header button {
    background: transparent;
    color: inherit;
    border-color: #888;
    padding: 0.25rem 0.6rem;
    font-size: 0.8rem;
  }

  .log {
    height: 320px;
    overflow-y: auto;
    border: 1px solid #ddd;
    border-radius: 6px;
    padding: 0.5rem;
    background: rgba(0, 0, 0, 0.04);
    font-family: ui-monospace, "SF Mono", Consolas, monospace;
    font-size: 0.82rem;
  }

  @media (prefers-color-scheme: dark) {
    .log {
      background: rgba(0, 0, 0, 0.3);
    }
  }

  .log pre {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .empty {
    color: #888;
    font-style: italic;
  }
</style>
