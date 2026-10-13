<script lang="ts">
  // A menu at a button: a folder's or a note's ways, as the desktop's
  // context menus have them. An item with a list behind it ("Move to")
  // opens that list in its place, with a way back. The arrows walk it,
  // Escape steps back or closes it, and the focus goes back to the button
  // that opened it; a press outside closes it too.
  import { onMount, tick } from 'svelte';
  import Icon from './Icon.svelte';
  import { placeMenu, type MenuItem } from './menu';

  /// `heading`: its label said at its top too (a menu with no row beside
  /// it to say what it's for).
  let { anchor, label, items, close, heading = false }: { anchor: HTMLElement; label: string; items: MenuItem[]; close: () => void; heading?: boolean } = $props();
  let stack = $state<{ label: string; items: MenuItem[] }[]>([]);
  const shown = $derived(stack.at(-1) ?? { label, items });
  let box: HTMLElement | undefined = $state();
  let at = $state({ left: 0, top: 0, maxHeight: 400 });

  async function place(focus = true) {
    await tick();
    if (!box) return;
    at = placeMenu(anchor.getBoundingClientRect(), box.offsetWidth, box.scrollHeight, innerWidth, innerHeight);
    if (focus) box.querySelector<HTMLElement>('[role=menuitem]')?.focus({ preventScroll: true });
  }

  function done(back = true) {
    // Read before it closes: closed, the props are gone with it.
    const opener = anchor;
    close();
    if (back && opener.isConnected) opener.focus({ preventScroll: true });
  }

  function choose(item: MenuItem) {
    if (item.items) {
      stack = [...stack, { label: item.label, items: item.items }];
      void place();
      return;
    }
    done();
    item.run?.();
  }

  function back() {
    stack = stack.slice(0, -1);
    void place();
  }

  function key(e: KeyboardEvent) {
    const all = [...(box?.querySelectorAll<HTMLElement>('[role=menuitem]') ?? [])];
    const i = all.indexOf(document.activeElement as HTMLElement);
    const go = (j: number) => all[(j + all.length) % all.length]?.focus();
    if (e.key === 'ArrowDown') go(i + 1);
    else if (e.key === 'ArrowUp') go(i - 1);
    else if (e.key === 'Home') go(0);
    else if (e.key === 'End') go(all.length - 1);
    else if (e.key === 'Escape' || (e.key === 'ArrowLeft' && stack.length)) {
      if (stack.length) back();
      else done();
    } else if (e.key === 'Tab') {
      e.preventDefault();
      done();
      return;
    } else return;
    e.preventDefault();
    e.stopPropagation();
  }

  onMount(() => {
    void place();
    const outside = (e: PointerEvent) => {
      if (!box?.contains(e.target as Node) && !anchor.contains(e.target as Node)) done(false);
    };
    const moved = () => void place(false);
    document.addEventListener('pointerdown', outside, true);
    addEventListener('resize', moved);
    return () => {
      document.removeEventListener('pointerdown', outside, true);
      removeEventListener('resize', moved);
    };
  });
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="menu"
  role="menu"
  tabindex="-1"
  aria-label={shown.label}
  bind:this={box}
  style:left="{at.left}px"
  style:top="{at.top}px"
  style:max-height="{at.maxHeight}px"
  onkeydown={key}
>
  {#if stack.length}
    <button class="item head" role="menuitem" onclick={back}><Icon name="back" size={16} /> {shown.label}</button>
  {:else if heading}
    <p class="title" aria-hidden="true">{label}</p>
  {/if}
  {#each shown.items as item (item.label)}
    <button class="item" class:danger={item.danger} role={item.checked === undefined ? 'menuitem' : 'menuitemcheckbox'} aria-checked={item.checked} aria-haspopup={item.items ? 'menu' : undefined} onclick={() => choose(item)}>
      {#if item.icon}<Icon name={item.icon} size={16} />{:else}<span class="gap"></span>{/if}
      <span class="label">{item.label}</span>
      {#if item.items}<Icon name="chevron-right" size={14} />{/if}
    </button>
  {/each}
</div>

<style>
  .menu { position: fixed; z-index: 20; min-width: 200px; max-width: min(320px, calc(100vw - 16px)); overflow: hidden auto; padding: 6px; border-radius: var(--radius-card); background: var(--card); border: 1px solid var(--border-strong); box-shadow: 0 10px 32px #0000002e; }
  .menu:focus { outline: none; }
  .item { display: flex; align-items: center; gap: 10px; width: 100%; min-height: 36px; padding: 7px 10px; border: 0; border-radius: var(--radius); background: none; color: var(--ink); text-align: left; cursor: pointer; }
  .item:hover, .item:focus-visible { background: var(--hover); outline: none; }
  .item:focus-visible { box-shadow: inset 0 0 0 2px var(--focus); }
  .item.danger { color: var(--danger); }
  .item.head { color: var(--muted); font-size: 13.5px; border-bottom: 1px solid var(--border); border-radius: var(--radius) var(--radius) 0 0; margin-bottom: 4px; }
  .title { margin: 0 0 4px; padding: 6px 10px 8px; border-bottom: 1px solid var(--border); color: var(--muted); font-size: 13.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .gap { width: 16px; flex: none; }
</style>
