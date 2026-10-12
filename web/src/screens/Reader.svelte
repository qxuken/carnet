<script lang="ts">
  // A note, as the desktop shows it: its checkboxes live, its pictures
  // fetched as they're needed. Edit opens it to write in (the editor's
  // code comes only then). One in Deleted is read only, with the ways
  // out: Restore, or Delete forever (asked to be sure first).
  import { sync } from '../lib/sync.svelte';
  import Icon from '../lib/Icon.svelte';
  import { pictures } from '../lib/pictures';
  import { tints } from '../lib/colors';
  import { dropLetters } from '../lib/initial';

  type Props = {
    key: string;
    back: (() => void) | null;
    history: () => void;
    editing: boolean;
    /// Made just now, here: it opens to be written in, and goes if left blank.
    fresh: boolean;
    edit: () => void;
    done: () => void;
    deleted: () => void;
  };
  let { key, back, history, editing, fresh, edit, done, deleted }: Props = $props();
  let box: HTMLElement | undefined = $state();
  const note = $derived(sync.notes.find((n) => n.key === key));
  const html = $derived((sync.version, sync.html(key)));
  const meta = $derived(
    note
      ? [note.folder, note.todos ? `${note.done} of ${note.todos} done` : '', note.trashed ? 'in Deleted' : '']
          .filter(Boolean)
          .join(' · ')
      : '',
  );

  // Pictures: `data-picture="<path>"` until they've come; and the
  // colours. Done each time the note is drawn anew, as it is drawn: a
  // note edited, then Done at once, is drawn again after this view is
  // back (the edit kept as the editor goes), with nothing else to say so.
  $effect(() => {
    if (!box) return;
    const el = box;
    const drop = () => dropLetters(el.querySelectorAll<HTMLElement>('p.initial'));
    const touch = () => {
      pictures(el);
      tints(el);
      drop();
    };
    touch();
    const drawn = new MutationObserver(touch);
    drawn.observe(el, { childList: true });
    // A narrower or wider page: paragraphs take more lines, or fewer.
    let width = el.clientWidth;
    const resized = new ResizeObserver(() => {
      if (el.clientWidth !== width) {
        width = el.clientWidth;
        drop();
      }
    });
    resized.observe(el);
    return () => {
      drawn.disconnect();
      resized.disconnect();
    };
  });

  let sure = $state(false);
  $effect(() => {
    void key;
    sure = false;
  });

  function restore() {
    if (sync.setTrashed(key, false)) sync.say('Restored');
  }

  function forever() {
    const title = note?.title ?? 'The note';
    if (!sync.deleteForever(key)) return;
    sync.say(`“${title}” deleted forever`);
    deleted();
  }

  function remove() {
    if (!sync.setTrashed(key, true)) return;
    sync.say('Moved to Deleted', () => sync.setTrashed(key, false));
    deleted();
  }

  function click(e: MouseEvent) {
    const t = e.target as HTMLElement;
    if (t instanceof HTMLInputElement && t.dataset.task !== undefined) {
      e.preventDefault();
      if (!note?.trashed) sync.toggle(key, Number(t.dataset.task));
    }
  }
</script>

<article>
  <header>
    {#if back}
      <button class="back" onclick={back} aria-label="All notes"><Icon name="back" size={22} /></button>
    {/if}
    <span class="meta">{meta}</span>
    {#if editing}
      <button class="done" onclick={done}><Icon name="check" size={17} /> Done</button>
    {:else if note && !note.trashed}
      <button class="tool" onclick={remove} aria-label="Move to Deleted" title="Move to Deleted"><Icon name="trash" size={19} /></button>
      <button class="tool" onclick={history} aria-label="History" title="History"><Icon name="clock" size={19} /></button>
      <button class="tool" onclick={edit} aria-label="Edit" title="Edit"><Icon name="edit" size={19} /></button>
    {/if}
  </header>
  {#if note?.trashed && !editing}
    <div class="deleted" role="region" aria-label="Deleted note">
      {#if sure}
        <p><Icon name="trash" size={16} /> Delete this note and its history on every device? This can’t be undone.</p>
        <div class="ways">
          <button class="pill" onclick={() => (sure = false)}>Cancel</button>
          <button class="pill danger" onclick={forever}>Yes, delete forever</button>
        </div>
      {:else}
        <p><Icon name="trash" size={16} /> In Deleted. Read only.</p>
        <div class="ways">
          <button class="pill" onclick={restore}><Icon name="undo" size={16} /> Restore</button>
          <button class="pill danger" onclick={() => (sure = true)}>Delete forever</button>
        </div>
      {/if}
    </div>
  {/if}
  {#if editing}
    <div class="note">
      {#await import('./Editor.svelte') then E}
        <E.default {key} folder={note?.folder ?? ''} {fresh} />
      {:catch}
        <p class="error">The editor couldn’t load. Check the connection and try again.</p>
      {/await}
    </div>
  {:else}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="note reading" bind:this={box} onclick={click}>{@html html}</div>
  {/if}
</article>

<style>
  article { height: 100%; overflow-y: auto; background: var(--page); }
  header { position: sticky; top: 0; display: flex; align-items: center; gap: 8px; min-height: 52px; padding: 8px 16px; padding-top: max(8px, env(safe-area-inset-top)); background: color-mix(in srgb, var(--page) 88%, transparent); backdrop-filter: blur(10px); z-index: 1; }
  .back { display: inline-flex; padding: 6px; margin-left: -6px; border: 0; background: none; color: var(--ink); border-radius: var(--radius); cursor: pointer; }
  .meta { color: var(--muted); font-size: 13.5px; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tool { display: inline-flex; padding: 6px; border: 0; background: none; color: var(--muted); border-radius: var(--radius); cursor: pointer; }
  .tool:hover { background: var(--hover); color: var(--ink); }
  .done { display: inline-flex; align-items: center; gap: 6px; padding: 7px 14px; border: 0; border-radius: 999px; background: var(--primary); color: var(--on-accent); font-weight: 600; cursor: pointer; }
  .error { color: var(--danger); }
  .deleted { max-width: 648px; margin: 4px auto 0; padding: 12px 20px; display: flex; flex-wrap: wrap; align-items: center; gap: 10px 16px; border-bottom: 1px solid var(--line, var(--hover)); }
  .deleted p { flex: 1 1 240px; margin: 0; display: flex; align-items: center; gap: 8px; color: var(--muted); font-size: 14px; }
  .ways { display: flex; gap: 8px; flex-wrap: wrap; }
  .pill { display: inline-flex; align-items: center; gap: 6px; padding: 7px 14px; border: 0; border-radius: 999px; background: var(--hover); color: var(--ink); font-weight: 600; cursor: pointer; }
  .pill.danger { background: var(--danger); color: var(--on-accent); }
  /* The text as wide as the desktop's at its usual page width (settings
     `width = medium`: 608), so the lines break alike and pictures, a
     share of it (carnet_core::page::picture_width), come out the same. */
  .note { max-width: 648px; margin: 0 auto; padding: 8px 20px calc(48px + env(safe-area-inset-bottom)); }
  /* What pictures at an edge are sized by (styles.css `.pictures`). */
  .reading { container-type: inline-size; }
</style>
