<script lang="ts">
  // The viewer: set up once, then the library and a note, side by side on
  // a wide screen and one at a time on a phone. Where it is lives in the
  // address (`#note/<key>`, `#note/<key>/history`, `#settings`), so Back
  // goes back. Writing in a note is `#note/<key>/edit` (`/new` for one
  // made just now).
  import { onMount, untrack } from 'svelte';
  import { sync } from './lib/sync.svelte';
  import Pair from './screens/Pair.svelte';
  import Library from './screens/Library.svelte';
  import Reader from './screens/Reader.svelte';
  import History from './screens/History.svelte';
  import Settings from './screens/Settings.svelte';
  import Icon from './lib/Icon.svelte';
  import { footer } from './lib/words';

  type Route =
    | { kind: 'list' }
    | { kind: 'note'; key: string; editing: boolean; fresh: boolean }
    | { kind: 'history'; key: string }
    | { kind: 'settings' };
  let route = $state<Route>({ kind: 'list' });
  let wide = $state(true);
  const current = $derived(route.kind === 'note' || route.kind === 'history' ? route.key : null);

  function read(): Route {
    const h = location.hash;
    if (h === '#settings') return { kind: 'settings' };
    const m = /^#note\/([0-9a-f]+)(?:\/(history|edit|new))?$/.exec(h);
    if (m?.[2] === 'history') return { kind: 'history', key: m[1] };
    if (m) return { kind: 'note', key: m[1], editing: m[2] === 'edit' || m[2] === 'new', fresh: m[2] === 'new' };
    return { kind: 'list' };
  }

  onMount(() => {
    void sync.start();
    const mq = matchMedia('(min-width: 760px)');
    wide = mq.matches;
    mq.onchange = () => (wide = mq.matches);
    route = read();
    addEventListener('hashchange', () => (route = read()));
  });

  // The library's width on a wide screen: its edge dragged (or moved
  // with the arrow keys) between the desktop's bounds, and kept in this
  // browser; a double-click puts back the usual width.
  const SIDE = 320;
  const SIDE_MIN = 200;
  const SIDE_MAX = 480;
  /// The least room the note keeps beside it.
  const PAGE_MIN = 440;
  const SIDE_KEY = 'carnet.libraryWidth';
  let sideW = $state(readSide());
  let dragFrom: { x: number; w: number } | null = $state(null);

  function readSide(): number {
    try {
      const v = Number(localStorage.getItem(SIDE_KEY));
      return v ? Math.min(SIDE_MAX, Math.max(SIDE_MIN, Math.round(v))) : SIDE;
    } catch {
      return SIDE;
    }
  }

  function keepSide() {
    try {
      if (sideW === SIDE) localStorage.removeItem(SIDE_KEY);
      else localStorage.setItem(SIDE_KEY, String(sideW));
    } catch {
      // Not kept: a private window, storage refused. Only a convenience.
    }
  }

  /// `w` within the bounds, and within what the window leaves the note.
  const fit = (w: number) => Math.round(Math.max(SIDE_MIN, Math.min(SIDE_MAX, innerWidth - PAGE_MIN, w)));

  function edgeDown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    dragFrom = { x: e.clientX, w: sideW };
  }

  function edgeMove(e: PointerEvent) {
    if (dragFrom) sideW = fit(dragFrom.w + e.clientX - dragFrom.x);
  }

  function edgeUp() {
    if (!dragFrom) return;
    dragFrom = null;
    keepSide();
  }

  function edgeReset() {
    sideW = fit(SIDE);
    keepSide();
  }

  function edgeKey(e: KeyboardEvent) {
    const step = e.shiftKey ? 48 : 16;
    const to =
      e.key === 'ArrowLeft' ? sideW - step
      : e.key === 'ArrowRight' ? sideW + step
      : e.key === 'Home' ? SIDE_MIN
      : e.key === 'End' ? SIDE_MAX
      : e.key === 'Enter' ? SIDE
      : null;
    if (to === null) return;
    e.preventDefault();
    sideW = fit(to);
    keepSide();
  }

  const go = (hash: string) => (location.hash = hash);
  /// The address changed without a step for Back: into or out of writing.
  const swap = (hash: string) => location.replace(`#${hash}`);

  function create(folder: string | null) {
    const key = sync.create(folder ?? '');
    if (key) go(`note/${key}/new`);
  }

  function finished(key: string) {
    swap(`note/${key}`);
    // A new note left blank was dropped: nothing to show.
    setTimeout(() => {
      if (!sync.notes.some((n) => n.key === key)) swap('');
    });
  }
  // Back where there's a page to go back to; else to the list.
  const back = () => (history.length > 1 ? history.back() : go(''));

  /// Removed with notes or pictures not sent: what they are, said plainly.
  const unsentWords = $derived.by(() => {
    const { notes, pictures } = sync.unsentCounts;
    const count = (n: number, one: string) => (n === 1 ? `1 ${one}` : `${n} ${one}s`);
    const what = [notes ? count(notes, 'note') : '', pictures ? count(pictures, 'picture') : ''].filter(Boolean).join(' and ');
    const one = notes + pictures === 1;
    return `${what} here ${one ? 'wasn’t' : 'weren’t'} sent before it was. Download ${one ? 'it' : 'them'} before this browser forgets ${one ? 'it' : 'them'}.`;
  });

  // What waits to be sent is said only once it has waited a moment: a
  // wait a fast server ends at once says nothing. Once said, when all of
  // it has gone, "All sent" for a moment, then the status again. (The
  // desktop's Settings → Sync times its line alike.)
  const WAIT_SHOWN_AFTER = 300;
  const SENT_SHOWN_FOR = 1500;
  let waitShown = $state(false);
  let allSent = $state(false);
  let waitTimer: ReturnType<typeof setTimeout> | undefined;
  let sentTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const waiting = sync.waiting + sync.picturesWaiting > 0;
    untrack(() => {
      if (waiting) {
        if (waitShown || waitTimer !== undefined) return;
        waitTimer = setTimeout(() => {
          waitTimer = undefined;
          clearTimeout(sentTimer);
          waitShown = true;
          allSent = false;
        }, WAIT_SHOWN_AFTER);
        return;
      }
      clearTimeout(waitTimer);
      waitTimer = undefined;
      if (!waitShown) return;
      waitShown = false;
      allSent = true;
      clearTimeout(sentTimer);
      sentTimer = setTimeout(() => (allSent = false), SENT_SHOWN_FOR);
    });
  });

  /// The footer's line (words.ts `footer`).
  const status = $derived(
    footer({
      status: sync.status,
      changes: sync.waiting,
      pictures: sync.picturesWaiting,
      waitShown,
      allSent,
      // Read only: said here (and in Settings), not again as an error.
      readOnly: sync.readOnly || (sync.usage !== null && !sync.usage.writable),
      held: sync.picturesHeld,
    }),
  );

  // Back at the start (this browser forgotten, or turned away): no page
  // of the account's left in the address, so the next account starts at
  // its notes.
  $effect(() => {
    if (sync.phase !== 'setup') return;
    untrack(() => {
      if (location.hash) history.replaceState(null, '', location.pathname + location.search);
      route = { kind: 'list' };
    });
  });
</script>

{#if sync.phase === 'loading'}
  <div class="loading">
    {#if sync.error}
      <div class="stopped">
        <p class="error" role="alert"><Icon name="warning" size={16} /> {sync.error}</p>
        <button onclick={() => location.reload()}>Reload</button>
      </div>
    {:else}<p class="wait">Opening your notes…</p>{/if}
  </div>
{:else if sync.phase === 'setup'}
  <Pair />
{:else if sync.removed}
  <!-- Removed, as the server says, with notes here it may not have: kept
       until they're downloaded, or this browser is forgotten. -->
  <div class="loading">
    <div class="stopped">
      <p class="error" role="alert"><Icon name="warning" size={16} /> {sync.removed}</p>
      <p class="said">{unsentWords}</p>
      <button onclick={() => void sync.downloadUnsent()}>Download what wasn’t sent</button>
      <button class="quiet" onclick={() => sync.forgetRemoved()}>Forget this browser</button>
    </div>
  </div>
{:else}
  <div class="shell" class:wide class:resizing={dragFrom !== null}>
    {#if wide || route.kind === 'list'}
      <div class="side" style:--side-w={wide ? `${sideW}px` : null}>
        <Library open={(key) => go(`note/${key}`)} {current} {create} />
        <footer>
          <span class="status" class:ok={status.ok} role="status">
            <Icon name={status.ok ? 'check' : 'clock'} size={14} /> {status.text}
          </span>
          <button class:on={route.kind === 'settings'} onclick={() => go('settings')} aria-label="Settings" title="Settings"><Icon name="settings" size={18} /></button>
        </footer>
        {#if status.why}<p class="held" role="status"><Icon name="warning" size={14} /> {status.why}</p>{/if}
        {#if sync.error && route.kind !== 'settings'}<p class="error" role="alert">{sync.error}</p>{/if}
        {#if wide}
          <!-- Its edge: dragged, the library wider or narrower. A focusable
               separator is a control (WAI-ARIA), which Svelte doesn't know. -->
          <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
          <div
            class="edge"
            class:on={dragFrom !== null}
            role="separator"
            aria-orientation="vertical"
            aria-label="Library width"
            aria-valuenow={sideW}
            aria-valuemin={SIDE_MIN}
            aria-valuemax={SIDE_MAX}
            tabindex="0"
            title="Drag to make the notes wider or narrower · double-click for the usual width"
            onpointerdown={edgeDown}
            onpointermove={edgeMove}
            onpointerup={edgeUp}
            onpointercancel={edgeUp}
            ondblclick={edgeReset}
            onkeydown={edgeKey}
          ></div>
        {/if}
      </div>
    {/if}
    {#if route.kind === 'note'}
      {@const key = route.key}
      <main>
        {#key key}
          <Reader
            {key}
            back={wide ? null : back}
            history={() => go(`note/${key}/history`)}
            editing={route.editing}
            fresh={route.fresh}
            edit={() => swap(`note/${key}/edit`)}
            done={() => finished(key)}
            deleted={() => (wide ? swap('') : back())}
          />
        {/key}
      </main>
    {:else if route.kind === 'history'}
      <main><History key={route.key} back={() => go(`note/${current}`)} {wide} /></main>
    {:else if route.kind === 'settings'}
      <main><Settings back={wide ? null : back} /></main>
    {:else if wide}
      <main class="none"><p>Choose a note.</p></main>
    {/if}
  </div>
{/if}

{#if sync.notice}
  <p class="notice" role="status">
    <Icon name="check" size={16} /> {sync.notice}
    {#if sync.noticeUndo}
      <button onclick={() => { sync.noticeUndo?.(); sync.notice = null; sync.noticeUndo = null; }}>Undo</button>
    {/if}
  </p>
{/if}

<style>
  .loading { height: 100%; display: grid; place-items: center; padding: 24px; background: var(--page); color: var(--muted); font-size: 15px; text-align: center; }
  .stopped { display: grid; gap: 16px; justify-items: center; max-width: 360px; }
  .stopped .error { padding: 0; font-size: 15px; }
  .stopped button { padding: 10px 22px; border: 0; border-radius: var(--radius); background: var(--primary); color: var(--on-accent); font: inherit; font-weight: 600; cursor: pointer; }
  .stopped .said { margin: 0; color: var(--ink); line-height: 1.5; }
  .stopped button.quiet { background: none; color: var(--ink); border: 1px solid var(--border-strong); font-weight: 500; }
  /* Only if it takes a moment: a quick start shouldn't flash. */
  .wait { opacity: 0; animation: show 0.2s ease 0.4s forwards; }
  @keyframes show { to { opacity: 1; } }
  .shell { height: 100%; display: flex; }
  .side { width: 100%; min-width: 0; height: 100%; display: flex; flex-direction: column; background: var(--side); position: relative; }
  .shell.wide .side { width: var(--side-w); max-width: max(200px, calc(100% - 440px)); flex: none; border-right: 1px solid var(--border); }
  /* Its edge: from the border out over the note's margin, never over the
     list's scrollbar, which runs down the library's own right side; its
     line drawn on the border. */
  .edge { position: absolute; top: 0; right: -8px; bottom: 0; width: 8px; cursor: ew-resize; z-index: 2; touch-action: none; }
  .edge::after { content: ''; position: absolute; top: 0; bottom: 0; left: 0; width: 3px; background: transparent; transition: background 0.15s; }
  .edge:hover::after { background: var(--border-strong); }
  .edge.on::after, .edge:focus-visible::after { background: var(--focus); }
  .edge:focus-visible { outline: none; }
  /* While its edge is dragged: nothing selected, the cursor kept. */
  .shell.resizing { cursor: ew-resize; user-select: none; }
  .side :global(aside) { flex: 1; min-height: 0; }
  main { flex: 1; min-width: 0; height: 100%; }
  main.none { display: grid; place-items: center; color: var(--faint); }
  footer { display: flex; align-items: center; justify-content: space-between; padding: 10px 18px; padding-bottom: max(10px, env(safe-area-inset-bottom)); border-top: 1px solid var(--border); }
  footer button { display: inline-flex; padding: 6px; border: 0; background: none; color: var(--muted); border-radius: var(--radius); cursor: pointer; }
  footer button:hover, footer button.on { background: var(--pressed); color: var(--ink); }
  .status { display: inline-flex; align-items: center; gap: 6px; font-size: 13px; color: var(--muted); }
  .status.ok { color: var(--success); }
  .error { color: var(--danger); font-size: 13px; padding: 0 18px 10px; margin: 0; }
  .held { display: flex; align-items: flex-start; gap: 6px; color: var(--muted); font-size: 13px; line-height: 1.4; padding: 0 18px 10px; margin: 0; }
  .held :global(.icon) { margin-top: 2px; }
  .notice {
    position: fixed; left: 50%; bottom: max(20px, env(safe-area-inset-bottom)); transform: translateX(-50%); margin: 0;
    display: inline-flex; align-items: center; gap: 8px; padding: 10px 16px; border-radius: 999px;
    background: var(--ink); color: var(--page); font-size: 14px; box-shadow: 0 6px 24px #00000026; z-index: 10; max-width: calc(100% - 32px);
  }
  .notice button { margin-left: 6px; padding: 2px 4px; border: 0; background: none; color: var(--accent); font-weight: 600; cursor: pointer; }
</style>
