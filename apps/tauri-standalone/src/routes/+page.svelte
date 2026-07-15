<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  interface AdapterInfo {
    protocol_version: number;
    default_port: number;
  }

  let info = $state<AdapterInfo | null>(null);
  let error = $state<string | null>(null);

  let host = $state("127.0.0.1");
  let port = $state(19144);

  onMount(async () => {
    try {
      info = await invoke<AdapterInfo>("adapter_info");
      port = info.default_port;
    } catch (e) {
      error = String(e);
    }
  });
</script>

<main>
  <header>
    <h1>Minecraft Debugger</h1>
    <p class="subtitle">Standalone</p>
  </header>

  <section class="status">
    {#if error}
      <p class="error">Failed to load adapter: {error}</p>
    {:else if info}
      <p>Protocol v{info.protocol_version} · Default port {info.default_port}</p>
    {:else}
      <p>Loading adapter...</p>
    {/if}
  </section>

  <section class="connection">
    <h2>Connection</h2>
    <form onsubmit={(e) => e.preventDefault()}>
      <label>
        Host
        <input type="text" bind:value={host} />
      </label>
      <label>
        Port
        <input type="number" bind:value={port} />
      </label>
      <button type="submit" disabled>Connect (wiring pending)</button>
    </form>
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
    max-width: 720px;
    margin: 0 auto;
    padding: 2rem;
  }

  header h1 {
    margin: 0;
    font-size: 1.75rem;
  }

  .subtitle {
    margin: 0.25rem 0 0;
    color: #888;
    font-size: 0.95rem;
  }

  section {
    margin-top: 2rem;
    padding: 1rem 1.25rem;
    border: 1px solid #ddd;
    border-radius: 8px;
    background: rgba(127, 127, 127, 0.05);
  }

  .error {
    color: #c33;
  }

  form {
    display: grid;
    gap: 0.75rem;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.9rem;
  }

  input {
    padding: 0.5rem 0.75rem;
    border: 1px solid #ccc;
    border-radius: 6px;
    font: inherit;
    background: inherit;
    color: inherit;
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
</style>
