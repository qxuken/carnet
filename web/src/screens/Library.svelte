<script lang="ts">
  // The notes: a folder (or all), searched, pinned first, newest next.
  import { sync, type Note } from '../lib/sync.svelte';
  import Icon from '../lib/Icon.svelte';
  import { shortAgo } from '../lib/time';

  let { open, current, create }: { open: (key: string) => void; current: string | null; create: (folder: string | null) => void } = $props();
  let query = $state('');
  let folder = $state<string | null>(null);
  let trash = $state(false);

  const shown = $derived.by(() => {
    const hits = query.trim() ? sync.search(query.trim()) : null;
    const list = sync.notes.filter(
      (n: Note) =>
        n.trashed === trash &&
        (hits ? hits.has(n.key) : true) &&
        // A folder's own notes, as the desktop lists them; its subfolders'
        // are theirs.
        (hits || folder === null || n.folder === folder),
    );
    return list.sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.modified - a.modified);
  });
  // Only the rows in sight (and a few either side) are in the page: ten
  // thousand notes are ten thousand rows otherwise. Rows are one height.
  const ROW = 66;
  let list: HTMLElement | undefined = $state();
  let top = $state(0);
  let height = $state(800);
  const from = $derived(Math.max(0, Math.floor(top / ROW) - 8));
  const to = $derived(Math.min(shown.length, Math.ceil((top + height) / ROW) + 8));
  // A new folder or search starts at its top.
  $effect(() => {
    void [query, folder, trash];
    if (list) list.scrollTop = top = 0;
  });

  const count = (f: string) => sync.notes.filter((n) => !n.trashed && n.folder === f).length;
  const deleted = $derived(sync.notes.filter((n) => n.trashed).length);
  // The last deleted note restored or deleted for good: the Deleted list
  // is gone from the side, so back to all the notes.
  $effect(() => {
    if (trash && !deleted) trash = false;
  });
</script>

<aside>
  <header>
    <h1 title={folder ?? undefined}>{trash ? 'Deleted' : (folder?.split('/').at(-1) ?? 'Notes')}</h1>
    {#if !trash}
      <button class="new" onclick={() => create(folder)} aria-label="New note" title="New note"><Icon name="plus" size={20} /></button>
    {/if}
  </header>
  <label class="search">
    <Icon name="search" size={16} />
    <input type="search" placeholder="Search" bind:value={query} />
  </label>
  {#if !query}
    <nav>
      <button class:on={folder === null && !trash} onclick={() => { folder = null; trash = false; }}>
        <Icon name="folder" size={16} /> <span class="name">All notes</span> <span class="count">{sync.notes.filter((n) => !n.trashed).length}</span>
      </button>
      {#each sync.folders as f (f)}
        <button class:on={folder === f && !trash} class="sub" style:padding-left="{14 + 14 * f.split('/').length}px" title={f} onclick={() => { folder = f; trash = false; }}>
          <Icon name="folder" size={16} /> <span class="name">{f.split('/').at(-1)}</span> <span class="count">{count(f)}</span>
        </button>
      {/each}
      {#if deleted}
        <button class:on={trash} onclick={() => { trash = true; folder = null; }}>
          <Icon name="trash" size={16} /> <span class="name">Deleted</span> <span class="count">{deleted}</span>
        </button>
      {/if}
    </nav>
  {/if}
  <ul bind:this={list} bind:clientHeight={height} onscroll={() => (top = list!.scrollTop)} aria-label="Notes">
    {#if shown.length}
      <li class="pad" style:height="{from * ROW}px" aria-hidden="true"></li>
      {#each shown.slice(from, to) as n, i (n.key)}
        <li style:height="{ROW}px" aria-setsize={shown.length} aria-posinset={from + i + 1}>
          <button class="row" class:on={current === n.key} onclick={() => open(n.key)}>
            <span class="top">
              <span class="title">{n.title}</span>
              {#if n.pinned}<Icon name="star" size={13} />{/if}
              <span class="when">{shortAgo(n.modified)}</span>
            </span>
            <span class="under">
              {#if n.folder && folder === null}<span class="folder" title={n.folder}>{n.folder}</span>{/if}
              {#if n.todos}<span class="todo">{n.done} of {n.todos} done</span>{/if}
              <span class="snippet">{n.snippet || 'No text'}</span>
            </span>
          </button>
        </li>
      {/each}
      <li class="pad" style:height="{(shown.length - to) * ROW}px" aria-hidden="true"></li>
    {:else}
      <li class="empty">{query ? 'No note has that in it.' : sync.status === 'Up to date' ? 'No notes here.' : 'Getting your notes…'}</li>
    {/if}
  </ul>
</aside>

<style>
  aside { display: flex; flex-direction: column; height: 100%; background: var(--side); }
  header { display: flex; align-items: center; gap: 8px; padding: 16px 18px 6px; padding-top: max(16px, env(safe-area-inset-top)); }
  h1 { font: 600 26px/1.2 var(--serif); margin: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0; }
  .new { display: inline-flex; padding: 6px; margin-right: -6px; border: 0; background: none; color: var(--accent-ink); border-radius: var(--radius); cursor: pointer; }
  .new:hover { background: var(--hover); }
  .search { display: flex; align-items: center; gap: 8px; margin: 8px 14px; padding: 8px 12px; border-radius: var(--radius); background: var(--sunken); color: var(--muted); }
  .search input { border: 0; background: none; color: var(--ink); font: inherit; width: 100%; outline: none; }
  /* Many folders scroll in their own space: the notes stay in sight. */
  /* Never sideways: a long name is cut short, whole on hover. */
  nav { display: flex; flex-direction: column; padding: 4px 8px; flex: none; max-height: 34%; overflow: hidden auto; border-bottom: 1px solid transparent; }
  nav:has(button:nth-child(9)) { border-bottom-color: var(--border); }
  nav button { display: flex; align-items: center; gap: 10px; min-width: 0; padding: 8px 14px; border: 0; background: none; border-radius: var(--radius); cursor: pointer; text-align: left; color: var(--ink); }
  nav .name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  nav .count { flex: none; color: var(--faint); font-size: 13px; }
  nav button:hover { background: var(--hover); }
  nav button.on { background: var(--pressed); }
  ul { list-style: none; margin: 0; padding: 6px 8px 24px; overflow: hidden auto; flex: 1; min-height: 0; }
  li { list-style: none; }
  .pad { pointer-events: none; }
  .row { display: flex; flex-direction: column; justify-content: center; gap: 3px; width: 100%; height: 64px; padding: 0 14px; border: 0; border-radius: var(--radius-card); background: none; text-align: left; cursor: pointer; }
  .row:hover { background: var(--hover); }
  .row.on { background: var(--card); box-shadow: 0 1px 3px #0000000f; }
  .top { display: flex; align-items: center; gap: 6px; min-width: 0; color: var(--accent-ink); }
  .title { min-width: 0; font-weight: 600; color: var(--ink); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .when { margin-left: auto; color: var(--faint); font-size: 12.5px; flex: none; }
  .under { display: flex; gap: 8px; color: var(--muted); font-size: 13.5px; min-width: 0; }
  .folder { flex: 0 1 auto; min-width: 0; max-width: 50%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--accent-ink); }
  .todo { flex: none; }
  .snippet { flex: 1 1 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .empty { color: var(--muted); padding: 24px 14px; }
</style>
