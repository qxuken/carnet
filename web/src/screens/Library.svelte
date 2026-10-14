<script lang="ts">
  // The notes: a folder (or all), searched, pinned first, newest next.
  //
  // Folders as the desktop has them: made (New folder, at the top, or
  // inside one from its menu), renamed, nested by a drag onto another
  // (onto All notes: to the top) or by its menu's "Move … to", deleted.
  // A note goes into a folder by a drag onto it, or its menu's "Move to";
  // the menus are the way on a touch screen and from the keyboard. Every
  // change goes through the vault (sync.svelte.ts), the rules with it.
  // A folder with folders inside folds them away by its chevron (Alt:
  // every folder inside too), or ← and → on its row; which are folded is
  // this browser's, kept in localStorage.
  import { tick } from 'svelte';
  import { sync, type FolderGone, type Note } from '../lib/sync.svelte';
  import Icon from '../lib/Icon.svelte';
  import Menu from '../lib/Menu.svelte';
  import type { MenuItem } from '../lib/menu';
  import { moveNote, noteMenu } from '../lib/noteMenu';
  import {
    afterDelete,
    arrow,
    deleteLabel,
    deletedWords,
    fold,
    foldsFollow,
    folderTargets,
    followed,
    hidden,
    keepFolded,
    leaf,
    mayDrop,
    movedWords,
    namingAfter,
    readFolded,
    shownAfterFold,
    unfoldTo,
    withSubfolders,
    type Drag,
  } from '../lib/folders';
  import { focusNow } from '../lib/focus';
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
  // The folder shown gone (on another device, say): every note.
  $effect(() => {
    if (folder !== null && !sync.folders.includes(folder)) folder = null;
  });

  // ---- folding

  let folded = $state<Set<string>>(readFolded());
  const parents = $derived(withSubfolders(sync.folders));
  $effect(() => keepFolded(folded));
  // The folder shown is never out of sight: however it came to be shown
  // (a new folder, a move, a rename), the folders it's in unfold.
  $effect(() => {
    if (folder !== null && !trash && hidden(folder, folded)) folded = unfoldTo(folded, folder);
  });

  /// `f` folded or unfolded (`deep`: every folder inside it too). A folder
  /// shown inside one folded gives way to it.
  function setFold(f: string, open: boolean, deep: boolean) {
    folded = fold(folded, sync.folders, f, open, deep);
    if (!open && !trash) folder = shownAfterFold(folder, f);
  }

  // What went wrong with a folder, said under the folders until the next
  // try.
  let problem = $state<string | null>(null);

  // ---- menus

  /// The menu open: at its button, for a row (`of`: a folder's path, ""
  /// for All notes, or a note's key), and what it says it is.
  let menu = $state<{ anchor: HTMLElement; of: string; label: string; items: MenuItem[] } | null>(null);

  function openMenu(anchor: HTMLElement, of: string, label: string, items: MenuItem[]) {
    if (menu?.anchor === anchor) {
      menu = null;
      return;
    }
    problem = null;
    menu = { anchor, of, label, items };
  }

  /// "All notes": what the desktop's menu for the notes folder has.
  function allMenu(): MenuItem[] {
    return [
      { label: 'New note', icon: 'plus', run: () => create(null) },
      { label: 'New folder', icon: 'folder-plus', run: () => startNaming('') },
    ];
  }

  function folderMenu(f: string): MenuItem[] {
    const bare = sync.folderIsBare(f);
    const targets = folderTargets(sync.folderTargets(f));
    const items: MenuItem[] = [
      { label: 'New note here', icon: 'plus', run: () => { folder = f; trash = false; create(f); } },
      { label: 'New folder inside', icon: 'folder-plus', run: () => startNaming(f) },
      { label: 'Rename…', icon: 'edit', run: () => startRename(f) },
    ];
    if (targets.length) {
      items.push({ label: `Move “${leaf(f)}” to`, icon: 'folder', items: targets.map((t) => ({ label: t.label, run: () => moveFolder(f, t.to) })) });
    }
    items.push({ label: deleteLabel(bare), icon: 'trash', danger: true, run: () => deleteFolder(f) });
    return items;
  }

  // ---- naming a folder: a new one, or one renamed, in a field in its row

  let naming = $state<{ parent: string; rename: string | null; name: string } | null>(null);
  const fieldAfter = $derived(naming && naming.rename === null ? namingAfter(sync.folders, naming.parent) : null);

  function startNaming(parent: string) {
    finishNaming(true, false);
    problem = null;
    // Its field goes under its parent's row: both in sight.
    if (parent) folded = fold(unfoldTo(folded, parent), sync.folders, parent, true);
    naming = { parent, rename: null, name: '' };
  }

  function startRename(f: string) {
    finishNaming(true, false);
    problem = null;
    naming = { parent: '', rename: f, name: leaf(f) };
  }

  /// Ends naming: the folder made or renamed from its name when `make`
  /// and there is one, as the desktop has it; a new one shown, when
  /// `show`.
  function finishNaming(make: boolean, show: boolean) {
    const n = naming;
    if (!n) return;
    naming = null;
    const name = n.name.trim();
    if (n.rename !== null) {
      if (make && name && name !== leaf(n.rename)) {
        const done = sync.renameFolder(n.rename, name);
        if ('why' in done) problem = `Couldn’t rename the folder: ${done.why}.`;
        else {
          folder = followed(folder, n.rename, done.to);
          folded = foldsFollow(folded, n.rename, done.to);
          focusFolder(done.to);
          return;
        }
      }
      focusFolder(n.rename);
      return;
    }
    if (!make || !name) return;
    const done = sync.makeFolder(n.parent, name);
    if ('why' in done) {
      problem = `Couldn’t make the folder: ${done.why}.`;
      return;
    }
    if (show) {
      folder = done.to;
      trash = false;
      focusFolder(done.to);
    }
  }

  function namingKey(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      finishNaming(true, true);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      finishNaming(false, false);
    }
  }

  let nav: HTMLElement | undefined = $state();
  /// The keyboard on a folder's row, once it's drawn.
  async function focusFolder(f: string) {
    await tick();
    nav?.querySelector<HTMLElement>(`[data-folder="${CSS.escape(f)}"]`)?.focus();
  }

  // ---- moving and deleting

  function moveFolder(f: string, to: string) {
    problem = null;
    const done = sync.moveFolder(f, to);
    if ('why' in done) {
      problem = `Couldn’t move the folder: ${done.why}.`;
      return;
    }
    folder = followed(folder, f, done.to);
    folded = foldsFollow(folded, f, done.to);
    sync.say(movedWords(leaf(f), to));
  }

  /// One with nothing in it is just deleted; any other goes to Deleted
  /// whole, Undo away, as on the desktop.
  function deleteFolder(f: string) {
    problem = null;
    const bare = sync.folderIsBare(f);
    const gone: FolderGone | null = sync.deleteFolder(f);
    if (!gone) return;
    folder = afterDelete(folder, f);
    sync.say(deletedWords(f, bare, gone.notes.length), bare ? null : () => sync.putBackFolder(gone));
  }

  /// F2 renames a folder, Delete or Backspace deletes it, ← and → fold
  /// and unfold it or go up and in: the desktop's keys on a folder's row.
  function folderKey(e: KeyboardEvent, f: string) {
    if (e.key === 'F2') startRename(f);
    else if (e.key === 'Delete' || e.key === 'Backspace') deleteFolder(f);
    else if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && !(e.shiftKey || e.ctrlKey || e.metaKey)) {
      const does = arrow(e.key, e.altKey, f, sync.folders, folded);
      if (does && 'go' in does) {
        folder = does.go;
        trash = false;
        focusFolder(does.go);
      } else if (does) setFold(f, !does.fold, does.deep);
    } else return;
    e.preventDefault();
  }

  // ---- dragging: a note or a folder onto a folder (All notes: the top)

  let drag = $state<Drag | null>(null);
  let over = $state<string | null>(null);
  // A folded folder a drag rests on unfolds after a moment, to go deeper.
  const SPRING = 700;
  let spring: { f: string; timer: ReturnType<typeof setTimeout> } | null = null;

  function springOver(f: string | null) {
    if (spring?.f === f) return;
    if (spring) clearTimeout(spring.timer);
    spring = null;
    if (f === null || !folded.has(f)) return;
    spring = {
      f,
      timer: setTimeout(() => {
        spring = null;
        if (drag) setFold(f, true, false);
      }, SPRING),
    };
  }

  function dragStart(e: DragEvent, d: Drag, words: string) {
    drag = d;
    menu = null;
    e.dataTransfer?.setData('text/plain', words);
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
  }

  function dragOver(e: DragEvent, to: string) {
    if (!mayDrop(drag, to)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
    over = to;
    springOver(to);
  }

  function dragLeave(e: DragEvent, to: string) {
    if (over === to && !(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node)) {
      over = null;
      springOver(null);
    }
  }

  function drop(e: DragEvent, to: string) {
    e.preventDefault();
    const d = drag;
    dragEnd();
    if (!d || !mayDrop(d, to)) return;
    if ('note' in d) {
      const n = sync.notes.find((x) => x.key === d.note);
      if (n) moveNote(n, to);
    } else moveFolder(d.folder, to);
  }

  function dragEnd() {
    drag = null;
    over = null;
    springOver(null);
  }
</script>

<aside>
  <header>
    <h1 title={folder ?? undefined}>{trash ? 'Deleted' : (folder?.split('/').at(-1) ?? 'Notes')}</h1>
    {#if !trash}
      <button class="new folder" onclick={() => startNaming('')} aria-label="New folder" title="New folder"><Icon name="folder-plus" size={20} /></button>
      <button class="new" onclick={() => create(folder)} aria-label="New note" title="New note"><Icon name="plus" size={20} /></button>
    {/if}
  </header>
  <label class="search">
    <Icon name="search" size={16} />
    <input type="search" placeholder="Search" aria-label="Search notes" bind:value={query} />
  </label>
  {#if !query}
    <nav bind:this={nav} aria-label="Folders">
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="frow" class:on={folder === null && !trash} class:drop={over === ''} ondragover={(e) => dragOver(e, '')} ondragleave={(e) => dragLeave(e, '')} ondrop={(e) => drop(e, '')}>
        <button
          class="fold"
          onclick={() => { folder = null; trash = false; }}
          oncontextmenu={(e) => { e.preventDefault(); openMenu(e.currentTarget.nextElementSibling as HTMLElement, '', 'All notes', allMenu()); }}
        >
          <Icon name="folder" size={16} /> <span class="name">All notes</span> <span class="count">{sync.notes.filter((n) => !n.trashed).length}</span>
        </button>
        <button class="more" aria-label="Menu for All notes" aria-haspopup="menu" aria-expanded={menu?.of === ''} title="More" onclick={(e) => openMenu(e.currentTarget, '', 'All notes', allMenu())}><Icon name="more" size={16} /></button>
      </div>
      {#if fieldAfter === -1}{@render field('', 1)}{/if}
      {#each sync.folders as f, i (f)}
        {@const depth = f.split('/').length}
        {@const open = !folded.has(f)}
        {#if hidden(f, folded)}
          <!-- Inside a folded folder: out of sight. -->
        {:else if naming?.rename === f}
          {@render field(f, depth)}
        {:else}
          <!-- Dragged by its row (a button alone isn't, in every browser). -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="frow"
            class:on={folder === f && !trash}
            class:drop={over === f}
            class:carried={drag && 'folder' in drag && drag.folder === f}
            draggable="true"
            ondragstart={(e) => dragStart(e, { folder: f, targets: sync.folderTargets(f) }, leaf(f))}
            ondragend={dragEnd}
            ondragover={(e) => dragOver(e, f)}
            ondragleave={(e) => dragLeave(e, f)}
            ondrop={(e) => drop(e, f)}
          >
            <!-- The chevron in the indent: the folder's icon where it would
                 be without one. -->
            {#if parents.has(f)}
              <button
                class="chevron"
                class:open
                style:margin-left="{14 * depth - 12}px"
                aria-expanded={open}
                aria-label="{open ? 'Hide' : 'Show'} the folders inside “{leaf(f)}”"
                title="{open ? 'Hide' : 'Show'} the folders inside (Alt: and all inside them)"
                onclick={(e) => setFold(f, !open, e.altKey)}
              ><Icon name="chevron-right" size={14} /></button>
            {/if}
            <button
              class="fold"
              data-folder={f}
              style:padding-left={parents.has(f) ? '4px' : `${14 + 14 * depth}px`}
              title={f}
              onclick={() => { folder = f; trash = false; }}
              onkeydown={(e) => folderKey(e, f)}
              oncontextmenu={(e) => { e.preventDefault(); openMenu(e.currentTarget.nextElementSibling as HTMLElement, f, leaf(f), folderMenu(f)); }}
            >
              <Icon name="folder" size={16} /> <span class="name">{leaf(f)}</span> <span class="count">{count(f)}</span>
            </button>
            <button class="more" aria-label="Menu for “{leaf(f)}”" aria-haspopup="menu" aria-expanded={menu?.of === f} title="More" onclick={(e) => openMenu(e.currentTarget, f, leaf(f), folderMenu(f))}><Icon name="more" size={16} /></button>
          </div>
        {/if}
        {#if fieldAfter === i}{@render field('', (naming?.parent ? naming.parent.split('/').length : 0) + 1)}{/if}
      {/each}
      {#if deleted}
        <div class="frow" class:on={trash}>
          <button class="fold" onclick={() => { trash = true; folder = null; }}>
            <Icon name="trash" size={16} /> <span class="name">Deleted</span> <span class="count">{deleted}</span>
          </button>
        </div>
      {/if}
    </nav>
    {#if problem}<p class="problem" role="alert"><Icon name="warning" size={14} /> {problem}</p>{/if}
  {/if}
  <ul bind:this={list} bind:clientHeight={height} onscroll={() => (top = list!.scrollTop)} aria-label="Notes">
    {#if shown.length}
      <li class="pad" style:height="{from * ROW}px" aria-hidden="true"></li>
      {#each shown.slice(from, to) as n, i (n.key)}
        <li
          style:height="{ROW}px"
          aria-setsize={shown.length}
          aria-posinset={from + i + 1}
          class:carried={drag && 'note' in drag && drag.note === n.key}
          draggable={!n.trashed}
          ondragstart={(e) => dragStart(e, { note: n.key, from: n.folder }, n.title)}
          ondragend={dragEnd}
        >
          <button
            class="row"
            class:on={current === n.key}
            onclick={() => open(n.key)}
            oncontextmenu={(e) => { if (n.trashed) return; e.preventDefault(); openMenu(e.currentTarget.nextElementSibling as HTMLElement, n.key, n.title, noteMenu(n)); }}
          >
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
          {#if !n.trashed}
            <button class="more note-more" aria-label="Menu for “{n.title}”" aria-haspopup="menu" aria-expanded={menu?.of === n.key} title="More" onclick={(e) => openMenu(e.currentTarget, n.key, n.title, noteMenu(n))}><Icon name="more" size={16} /></button>
          {/if}
        </li>
      {/each}
      <li class="pad" style:height="{(shown.length - to) * ROW}px" aria-hidden="true"></li>
    {:else}
      <li class="empty">{query ? 'No note has that in it.' : sync.status === 'Up to date' ? 'No notes here.' : 'Getting your notes…'}</li>
    {/if}
  </ul>
</aside>

{#if menu}
  <Menu anchor={menu.anchor} label={menu.label} items={menu.items} close={() => (menu = null)} />
{/if}

<!-- A folder's name being typed: a new one (after its parent's rows), or
     one renamed (in its row's place). Enter makes it; Escape lets it go;
     leaving the field makes it too, if it has a name. -->
{#snippet field(f: string, depth: number)}
  <div class="frow naming" style:padding-left="{14 + 14 * depth}px">
    <Icon name="folder" size={16} />
    <input
      bind:value={naming!.name}
      use:focusNow
      aria-label={naming?.rename ? `New name for “${leaf(f)}”` : 'New folder’s name'}
      placeholder="Folder name"
      onkeydown={namingKey}
      onblur={() => finishNaming(true, false)}
    />
  </div>
{/snippet}

<style>
  aside { display: flex; flex-direction: column; height: 100%; background: var(--side); }
  header { display: flex; align-items: center; gap: 8px; padding: 16px 18px 6px; padding-top: max(16px, env(safe-area-inset-top)); }
  h1 { font: 600 26px/1.2 var(--serif); margin: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0; }
  .new { display: inline-flex; padding: 6px; margin-right: -6px; border: 0; background: none; color: var(--accent-ink); border-radius: var(--radius); cursor: pointer; }
  .new:hover { background: var(--hover); }
  .search { display: flex; align-items: center; gap: 8px; margin: 8px 14px; padding: 8px 12px; border-radius: var(--radius); background: var(--sunken); color: var(--muted); }
  .search input { border: 0; background: none; color: var(--ink); font: inherit; width: 100%; outline: none; }
  .new.folder { color: var(--muted); margin-right: 0; }
  .new.folder:hover { color: var(--ink); }
  /* Many folders scroll in their own space: the notes stay in sight. */
  /* Never sideways: a long name is cut short, whole on hover. */
  nav { display: flex; flex-direction: column; padding: 4px 8px; flex: none; max-height: 34%; overflow: hidden auto; border-bottom: 1px solid transparent; }
  nav:has(.frow:nth-child(9)) { border-bottom-color: var(--border); }
  .frow { display: flex; align-items: center; min-width: 0; flex: none; border-radius: var(--radius); }
  .frow:hover { background: var(--hover); }
  .frow.on { background: var(--pressed); }
  /* What a drag would go into, lit; what is carried, faint. */
  .frow.drop { background: var(--accent-soft); box-shadow: inset 0 0 0 2px var(--accent); }
  .carried { opacity: 0.5; }
  /* A folder's chevron: turned a quarter, its folders shown. */
  .chevron { flex: none; display: inline-flex; width: 22px; height: 26px; margin-right: 0; padding: 0; align-items: center; justify-content: center; border: 0; background: none; color: var(--muted); border-radius: var(--radius); cursor: pointer; }
  .chevron:hover { background: var(--pressed); color: var(--ink); }
  .chevron :global(svg) { transition: transform 0.12s ease; }
  .chevron.open :global(svg) { transform: rotate(90deg); }
  @media (prefers-reduced-motion: reduce) { .chevron :global(svg) { transition: none; } }
  /* On a touch screen, or a phone's width: a row a finger's height, and
     the chevron a 44px target, its reach (a box of no paint) past its
     drawn 22×26 into the indent and the row's height, over no folder's
     name, so nothing moves. */
  @media (pointer: coarse), (max-width: 600px) {
    .frow { min-height: 44px; }
    .chevron { position: relative; }
    .chevron::before { content: ''; position: absolute; inset: -9px -11px; }
  }
  .fold { flex: 1; display: flex; align-items: center; gap: 10px; min-width: 0; padding: 8px 14px; border: 0; background: none; border-radius: var(--radius); cursor: pointer; text-align: left; color: var(--ink); }
  nav .name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  nav .count { flex: none; color: var(--faint); font-size: 13px; }
  /* A row's menu: shown on the row the pointer or the keyboard is on, the
     one chosen, and always on a touch screen, which has no hover. */
  .more { flex: none; display: inline-flex; padding: 6px; margin-right: 4px; border: 0; background: none; color: var(--muted); border-radius: var(--radius); cursor: pointer; opacity: 0; }
  .more:hover { background: var(--pressed); color: var(--ink); }
  .frow:hover .more, .frow:focus-within .more, .frow.on .more, li:hover .more, li:focus-within .more, .row.on + .more, .more[aria-expanded='true'], .more:focus-visible { opacity: 1; }
  @media (hover: none) { .more { opacity: 1; } }
  .naming { gap: 10px; padding: 4px 8px 4px 14px; color: var(--muted); background: var(--card); box-shadow: inset 0 0 0 1px var(--border-strong); }
  .naming input { flex: 1; min-width: 0; padding: 4px 0; border: 0; background: none; color: var(--ink); font: inherit; outline: none; }
  .problem { display: flex; align-items: flex-start; gap: 6px; margin: 0; padding: 4px 22px 8px; color: var(--danger); font-size: 13px; line-height: 1.4; }
  .problem :global(.icon) { margin-top: 2px; }
  ul { list-style: none; margin: 0; padding: 6px 8px 24px; overflow: hidden auto; flex: 1; min-height: 0; }
  li { list-style: none; position: relative; }
  .note-more { position: absolute; right: 6px; top: 50%; transform: translateY(-50%); margin: 0; }
  .pad { pointer-events: none; }
  .row { display: flex; flex-direction: column; justify-content: center; gap: 3px; width: 100%; height: 64px; padding: 0 42px 0 14px; border: 0; border-radius: var(--radius-card); background: none; text-align: left; cursor: pointer; }
  .row:hover { background: var(--hover); }
  .row.on { background: var(--card); box-shadow: 0 1px 3px #0000000f; }
  .top { display: flex; align-items: center; gap: 6px; min-width: 0; color: var(--accent-ink); }
  /* Pinned: the star filled, as the desktop draws it. */
  .top :global(svg path) { fill: currentColor; }
  .title { min-width: 0; font-weight: 600; color: var(--ink); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .when { margin-left: auto; color: var(--faint); font-size: 12.5px; flex: none; }
  .under { display: flex; gap: 8px; color: var(--muted); font-size: 13.5px; min-width: 0; }
  .folder { flex: 0 1 auto; min-width: 0; max-width: 50%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--accent-ink); }
  .todo { flex: none; }
  .snippet { flex: 1 1 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .empty { color: var(--muted); padding: 24px 14px; }
</style>
