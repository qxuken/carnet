<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { is_note_link, note_href, typed_link_address } from '../wasm/carnet_web.js';

  type Note = { id: string; title: string; path: string };
  type Section = { name: string; fragment: string | null };
  type Props = {
    source: string; folder: string; initial: string | null; revision: number;
    notes: (query: string) => Note[];
    sections: (id: string) => { name: string; fragment: string }[];
    resolve: (href: string) => string | undefined;
    choose: (url: string, title?: string) => void;
    remove: () => void; cancel: () => void; open?: () => void;
  };
  let { source, folder, initial, revision, notes, sections, resolve, choose, remove, cancel, open }: Props = $props();
  const uid = $props.id();
  const original = untrack(() => initial ?? '');
  const originalId = untrack(() => original.startsWith('#') ? source : resolve(original));
  let mode = $state<'note' | 'address'>(!original || is_note_link(original) || original.startsWith('#') ? 'note' : 'address');
  let target = $state<Note | null>(untrack(() => notes('').find((n) => n.id === originalId) ?? null));
  let fragment = $state<string | null>(original.includes('#') ? original.slice(original.indexOf('#')) : null);
  let query = $state('');
  let address = $state(original);
  let active = $state(0);
  let input = $state<HTMLInputElement>();
  const found = $derived.by(() => { revision; return notes(query).sort((a, b) => Number(b.id === source) - Number(a.id === source)).slice(0, 8); });
  const places = $derived.by(() => {
    revision;
    const all: Section[] = [...(target?.id === source ? [] : [{ name: 'Whole note', fragment: null }]), ...(target ? sections(target.id) : [])];
    return all.filter((s) => s.name.toLowerCase().includes(query.toLowerCase()) || s.fragment?.toLowerCase().includes(query.toLowerCase()));
  });
  const count = $derived(target ? places.length : found.length);
  const safeActive = $derived(Math.min(active, Math.max(0, count - 1)));

  onMount(() => { if (target && fragment) active = Math.max(0, places.findIndex((s) => s.fragment === fragment)); input?.focus(); });
  $effect(() => { const row = safeActive; count; void tick().then(() => document.getElementById(`${uid}-${row}`)?.scrollIntoView({ block: 'nearest' })); });
  function focus() { void tick().then(() => input?.focus()); }
  function switchMode(to: 'note' | 'address') { mode = to; query = ''; active = 0; focus(); }
  function pick(note: Note) { target = note; fragment = null; query = ''; active = 0; focus(); }
  function back() { target = null; query = ''; active = 0; focus(); }
  function searchChanged(text: string) {
    query = text;
    active = 0;
    if (!target && typed_link_address(query) && !notes(query).length) { address = query; switchMode('address'); }
  }
  function insert(section = fragment) {
    if (mode === 'address') { if (address.trim()) choose(address.trim()); return; }
    if (!target || (target.id === source && !section)) return;
    const to = notes('').find((n) => n.id === target!.id);
    if (!to) { back(); return; }
    const url = to.id === source && section ? section : note_href(folder, to.path, section ?? undefined);
    choose(url, to.id === source ? sections(to.id).find((s) => s.fragment === section)?.name ?? to.title : to.title);
  }
  function keys(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault(); e.stopPropagation();
      if (mode === 'note' && target) back(); else cancel();
    } else if (mode === 'note' && ['ArrowDown', 'ArrowUp', 'Enter'].includes(e.key)) {
      e.preventDefault(); e.stopPropagation();
      if (e.key === 'Enter') {
        if (target) insert(places[safeActive]?.fragment ?? null);
        else if (found[safeActive]) pick(found[safeActive]);
      } else active = count ? (safeActive + (e.key === 'ArrowDown' ? 1 : -1) + count) % count : 0;
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="link-dialog" role="dialog" aria-label={initial ? 'Edit link' : 'Make a link'} tabindex="-1" onpointerdown={(e) => e.stopPropagation()}>
  <form onsubmit={(e) => { e.preventDefault(); insert(target ? places[safeActive]?.fragment ?? null : fragment); }}>
    <div class="tabs" role="group" aria-label="Link destination">
      <button type="button" class:on={mode === 'note'} aria-pressed={mode === 'note'} onclick={() => switchMode('note')}>Note</button>
      <button type="button" class:on={mode === 'address'} aria-pressed={mode === 'address'} onclick={() => switchMode('address')}>Web address</button>
    </div>
    {#if mode === 'note'}
      {#if target}
        <button type="button" class="chosen" onclick={back} aria-label="Choose another note"><span aria-hidden="true">←</span><span><b>{target.id === source ? 'This note' : target.title}</b>{#if target.id !== source}<small>{target.path}</small>{/if}</span></button>
      {/if}
      <input aria-label={target ? 'Find a section' : 'Search notes'} bind:this={input} bind:value={query} oninput={(e) => searchChanged(e.currentTarget.value)} onkeydown={keys} type="search" placeholder={target?.id === source ? 'Find a section' : target ? 'Find a section (optional)' : 'Search notes by title'} role="combobox" aria-autocomplete="list" aria-expanded="true" aria-controls={`${uid}-results`} aria-activedescendant={count ? `${uid}-${safeActive}` : undefined} autocomplete="off" />
      <div id={`${uid}-results`} class="results" role="listbox" aria-label={target ? 'Sections' : 'Notes'}>
        {#if target}
          {#each places as section, i}
            <button id={`${uid}-${i}`} type="button" role="option" aria-selected={i === safeActive} class:active={i === safeActive} onclick={() => { fragment = section.fragment; active = i; }}><span>{section.name}</span>{#if section.fragment}<small>{section.fragment}</small>{/if}</button>
          {/each}
          {#if !places.length}<p>{target.id === source ? 'No sections match. Add a heading or /anchor to this note.' : 'No sections match. Clear the search to link to the whole note.'}</p>{/if}
        {:else}
          {#each found as note, i (note.id)}
            <button id={`${uid}-${i}`} type="button" role="option" aria-selected={i === safeActive} class:active={i === safeActive} onclick={() => pick(note)}><b>{note.title}</b><small>{#if note.id === source}This note · {/if}{note.path}</small></button>
          {/each}
          {#if !found.length}<p>No notes match. Try a different title or folder.</p>{/if}
        {/if}
      </div>
    {:else}
      <input bind:this={input} bind:value={address} onkeydown={keys} placeholder="https://example.com" aria-label="Link address" autocomplete="off" spellcheck="false" autocapitalize="off" />
    {/if}
    <div class="actions">
      {#if initial}<button type="button" onclick={remove}>Remove link</button>{/if}
      {#if open}<button type="button" onclick={open}>Open</button>{/if}
      <span></span>
      <button type="button" onclick={cancel}>Cancel</button>
      <button class="primary" disabled={mode === 'note' ? !target || !places.length : !address.trim()}>{initial ? 'Change link' : 'Insert link'}</button>
    </div>
  </form>
</div>

<style>
  .link-dialog { box-sizing: border-box; width: min(440px, calc(100vw - 32px)); background: var(--card); color: var(--ink); border: 1px solid var(--border); border-radius: 12px; padding: 12px; box-shadow: 0 8px 28px #0002; }
  form { display: grid; gap: 10px; }
  button, input { font: inherit; }
  button { border: 1px solid transparent; border-radius: 6px; background: transparent; color: inherit; padding: 7px 10px; text-align: left; cursor: pointer; }
  button:hover, button.active { background: var(--sunken); }
  button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .tabs { display: flex; gap: 4px; }
  .tabs button { color: var(--muted); }
  .tabs button.on { color: var(--ink); background: var(--sunken); }
  input { width: 100%; min-width: 0; box-sizing: border-box; border: 1px solid var(--border); border-radius: 6px; padding: 10px; background: var(--sunken); color: var(--ink); }
  .results { max-height: min(290px, 35vh); overflow: auto; display: grid; gap: 2px; }
  .results button { display: grid; gap: 3px; width: 100%; overflow-wrap: anywhere; }
  .results button[aria-selected="true"] { box-shadow: inset 2px 0 var(--accent); }
  small { display: block; color: var(--muted); font-size: 12px; overflow-wrap: anywhere; }
  .results p { color: var(--muted); font-size: 13px; padding: 8px; margin: 0; }
  .chosen { display: flex; align-items: center; gap: 10px; }
  .actions { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; border-top: 1px solid var(--border); padding-top: 8px; font-size: 13px; }
  .actions span { flex: 1; }
  .primary { background: var(--primary); color: var(--on-accent, white); }
  .primary:hover { background: var(--primary); filter: brightness(.95); }
  button:disabled { opacity: .4; cursor: default; }
</style>
