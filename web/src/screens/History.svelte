<script lang="ts">
  // A note's history, as the desktop's: its versions, newest first, what
  // made each and how many words it changed; one shown with what changed
  // marked in the note's own styles; and brought back, if wanted.
  import { sync } from '../lib/sync.svelte';
  import { ago, shortAgo } from '../lib/time';
  import Icon from '../lib/Icon.svelte';
  import { pictures } from '../lib/pictures';
  import { tints } from '../lib/colors';
  import { dropLetters } from '../lib/initial';
  import { onDestroy, onMount } from 'svelte';

  let { key, back, wide }: { key: string; back: () => void; wide: boolean } = $props();
  const list = $derived((sync.version, sync.history(key)));
  let shown = $state(0);
  /// On a phone: a version open, or the list.
  let reading = $state(false);
  let restoring = $state(false);
  let box: HTMLElement | undefined = $state();
  const past = $derived(list[shown]);
  const html = $derived(past ? sync.pastHtml(key, past.version) : '');

  // The devices, to name who made each version.
  onMount(() => sync.listDevices());
  onDestroy(() => sync.doneWithDevices());

  /// What made a version, and where: "Edited here", "Edited on Laptop".
  function made(p: { what: string; by: string | null }) {
    if (!p.by) return p.what;
    if (p.by === 'here') return `${p.what} here`;
    const name = sync.devices?.find((d) => d.id === p.by)?.name;
    return `${p.what} on ${name ?? 'another device'}`;
  }

  $effect(() => {
    void html;
    if (!box) return;
    pictures(box);
    tints(box);
    dropLetters(box.querySelectorAll<HTMLElement>('p.initial'));
    // A version is for reading: its boxes don't tick.
    for (const i of box.querySelectorAll('input')) i.disabled = true;
  });

  const words = (p: { added: number; removed: number }) =>
    p.added && p.removed ? `+${p.added} −${p.removed} words` : p.added ? `+${p.added} words` : p.removed ? `−${p.removed} words` : 'no words changed';
  const detail = (i: number) => (i === 0 ? `The note as it is · ${words(list[i])}` : `${ago(list[i].time)} · ${words(list[i])}`);

  function pick(i: number) {
    shown = i;
    restoring = false;
    reading = true;
  }

  function restore() {
    const when = ago(list[shown].time);
    if (sync.restore(key, list[shown].version)) {
      sync.say(`Restored the version from ${when}`);
      back();
    }
  }
</script>

<div class="history" class:wide>
  {#if wide || !reading}
    <aside>
      <header>
        <button class="back" onclick={back} aria-label="Back to the note"><Icon name="back" size={22} /></button>
        <h1>History</h1>
      </header>
      <p class="hint">Tap one to read it. What changed is marked: added words in green, removed ones struck through in red.</p>
      <ul>
        {#each list as p, i (p.version)}
          <li>
            <button class="row" class:on={wide && i === shown} onclick={() => pick(i)}>
              <span class="top"><b>{made(p)}</b><span class="when">{i === 0 ? 'now' : shortAgo(p.time)}</span></span>
              <span class="under">{detail(i)}</span>
            </button>
          </li>
        {/each}
      </ul>
    </aside>
  {/if}
  {#if past && (wide || reading)}
    <main>
      <header>
        {#if !wide}
          <button class="back" onclick={() => (reading = false)} aria-label="All versions"><Icon name="back" size={22} /></button>
        {/if}
        <span class="meta">{shown === 0 ? 'The note as it is' : `The version from ${ago(past.time)}`} · {made(past).replace(/^./, (c) => c.toLowerCase())}</span>
        {#if shown !== 0 && !restoring}
          <button class="pill" onclick={() => (restoring = true)}><Icon name="undo" size={16} /> Restore</button>
        {/if}
      </header>
      {#if restoring}
        <div class="confirm">
          <p>The note becomes this version again, on every device. What it is now stays in its history, to restore in turn.</p>
          <div class="buttons">
            <button class="pill" onclick={() => (restoring = false)}>Cancel</button>
            <button class="pill primary" onclick={restore}>Yes, restore</button>
          </div>
        </div>
      {/if}
      <div class="note" bind:this={box}>{@html html}</div>
    </main>
  {/if}
</div>

<style>
  .history { height: 100%; display: flex; background: var(--page); }
  aside { width: 100%; height: 100%; display: flex; flex-direction: column; background: var(--side); }
  .wide aside { width: 300px; flex: none; border-right: 1px solid var(--border); }
  header { display: flex; align-items: center; gap: 8px; min-height: 52px; padding: 8px 16px; padding-top: max(8px, env(safe-area-inset-top)); }
  h1 { font: 600 22px/1.2 var(--serif); margin: 0; }
  .back { display: inline-flex; padding: 6px; margin-left: -6px; border: 0; background: none; color: var(--ink); border-radius: var(--radius); cursor: pointer; }
  .hint { margin: 0 18px 8px; font-size: 13px; color: var(--muted); }
  ul { list-style: none; margin: 0; padding: 4px 8px 24px; overflow-y: auto; flex: 1; }
  .row { display: flex; flex-direction: column; gap: 3px; width: 100%; padding: 10px 14px; border: 0; border-radius: var(--radius-card); background: none; text-align: left; cursor: pointer; }
  .row:hover { background: var(--hover); }
  .row.on { background: var(--card); box-shadow: 0 1px 3px #0000000f; }
  .top { display: flex; gap: 8px; align-items: baseline; }
  .top b { font-weight: 600; color: var(--ink); }
  .when { margin-left: auto; font-size: 12.5px; color: var(--faint); }
  .under { font-size: 13px; color: var(--muted); }
  main { flex: 1; min-width: 0; height: 100%; overflow-y: auto; }
  main header { position: sticky; top: 0; background: color-mix(in srgb, var(--page) 88%, transparent); backdrop-filter: blur(10px); z-index: 1; }
  .meta { color: var(--muted); font-size: 13.5px; flex: 1; min-width: 0; }
  .pill { display: inline-flex; align-items: center; gap: 6px; flex: none; padding: 7px 14px; border: 0; border-radius: 999px; background: var(--sunken); color: var(--ink); font-weight: 500; cursor: pointer; }
  .pill.primary { background: var(--primary); color: var(--on-accent); }
  .confirm { max-width: 648px; margin: 0 auto; padding: 4px 20px 8px; color: var(--muted); font-size: 13.5px; }
  .confirm p { margin: 0 0 10px; }
  .buttons { display: flex; gap: 8px; justify-content: flex-end; }
  /* As wide as the reader's (Reader.svelte `.note`), the desktop's. */
  .note { max-width: 648px; margin: 0 auto; padding: 8px 20px calc(48px + env(safe-area-inset-bottom)); }
</style>
