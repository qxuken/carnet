<script lang="ts">
  // The desktop's icons (exported with `carnet --icons`), in the text's
  // colour.
  const all = import.meta.glob('../icons/*.svg', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;
  let { name, size = 18 }: { name: string; size?: number } = $props();
  const svg = $derived(all[`../icons/${name}.svg`] ?? '');
</script>

<span class="icon" style:width="{size}px" style:height="{size}px" aria-hidden="true">{@html svg}</span>

<style>
  .icon { display: inline-flex; flex: none; }
  .icon :global(svg) { width: 100%; height: 100%; }
</style>
