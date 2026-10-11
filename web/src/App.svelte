<script lang="ts">
  // The viewer: set up once, then the library and a note, side by side on
  // a wide screen and one at a time on a phone. Where it is lives in the
  // address (`#note/<key>`, `#note/<key>/history`, `#settings`), so Back
  // goes back. Writing in a note is `#note/<key>/edit` (`/new` for one
  // made just now).
  import { onMount } from 'svelte';
  import { sync } from './lib/sync.svelte';
  import Pair from './screens/Pair.svelte';
  import Library from './screens/Library.svelte';
  import Reader from './screens/Reader.svelte';
  import History from './screens/History.svelte';
  import Settings from './screens/Settings.svelte';
  import Icon from './lib/Icon.svelte';

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
{:else}
  <div class="shell" class:wide>
    {#if wide || route.kind === 'list'}
      <div class="side">
        <Library open={(key) => go(`note/${key}`)} {current} {create} />
        <footer>
          <span class="status" class:ok={sync.status === 'Up to date'}>
            <Icon name={sync.status === 'Up to date' ? 'check' : 'clock'} size={14} /> {sync.status}
          </span>
          <button class:on={route.kind === 'settings'} onclick={() => go('settings')} aria-label="Settings" title="Settings"><Icon name="settings" size={18} /></button>
        </footer>
        {#if sync.error && route.kind !== 'settings'}<p class="error" role="alert">{sync.error}</p>{/if}
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
  /* Only if it takes a moment: a quick start shouldn't flash. */
  .wait { opacity: 0; animation: show 0.2s ease 0.4s forwards; }
  @keyframes show { to { opacity: 1; } }
  .shell { height: 100%; display: flex; }
  .side { width: 100%; height: 100%; display: flex; flex-direction: column; background: var(--side); position: relative; }
  .shell.wide .side { width: 320px; flex: none; border-right: 1px solid var(--border); }
  .side :global(aside) { flex: 1; min-height: 0; }
  main { flex: 1; min-width: 0; height: 100%; }
  main.none { display: grid; place-items: center; color: var(--faint); }
  footer { display: flex; align-items: center; justify-content: space-between; padding: 10px 18px; padding-bottom: max(10px, env(safe-area-inset-bottom)); border-top: 1px solid var(--border); }
  footer button { display: inline-flex; padding: 6px; border: 0; background: none; color: var(--muted); border-radius: var(--radius); cursor: pointer; }
  footer button:hover, footer button.on { background: var(--pressed); color: var(--ink); }
  .status { display: inline-flex; align-items: center; gap: 6px; font-size: 13px; color: var(--muted); }
  .status.ok { color: var(--success); }
  .error { color: var(--danger); font-size: 13px; padding: 0 18px 10px; margin: 0; }
  .notice {
    position: fixed; left: 50%; bottom: max(20px, env(safe-area-inset-bottom)); transform: translateX(-50%); margin: 0;
    display: inline-flex; align-items: center; gap: 8px; padding: 10px 16px; border-radius: 999px;
    background: var(--ink); color: var(--page); font-size: 14px; box-shadow: 0 6px 24px #00000026; z-index: 10; max-width: calc(100% - 32px);
  }
  .notice button { margin-left: 6px; padding: 2px 4px; border: 0; background: none; color: var(--accent); font-weight: 600; cursor: pointer; }
</style>
