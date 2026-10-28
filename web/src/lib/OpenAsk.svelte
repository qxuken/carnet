<script lang="ts">
  // The question before a file that could run opens, in the middle of the
  // page, as the desktop asks it (app_open.rs): what it is, what it could
  // do, and Cancel, which has the keyboard, or Open anyway. Esc, Enter
  // and a press outside it cancel.
  import { opener } from './open.svelte';
  import { fileName, OPEN_ASK_WHY, openAsk } from './files';

  let box: HTMLDialogElement | undefined = $state();
  const name = $derived(opener.asking ? fileName(opener.asking) : '');

  $effect(() => {
    if (!box) return;
    if (opener.asking && !box.open) box.showModal();
    else if (!opener.asking && box.open) box.close();
  });

  /// A press on the backdrop (the dialog itself, outside its box): Cancel.
  function outside(e: MouseEvent) {
    if (e.target === box) opener.cancel();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<dialog bind:this={box} aria-labelledby="open-ask" onclose={() => opener.cancel()} onclick={outside}>
  <div class="box">
    <p id="open-ask" class="ask">{openAsk(name)}</p>
    <p class="why">{OPEN_ASK_WHY}</p>
    <div class="buttons">
      <button class="pill" onclick={() => opener.anyway()}>Open anyway</button>
      <!-- svelte-ignore a11y_autofocus -->
      <button class="pill primary" autofocus onclick={() => opener.cancel()}>Cancel</button>
    </div>
  </div>
</dialog>

<style>
  dialog { padding: 0; border: 1px solid var(--border); border-radius: var(--radius-card); background: var(--card); color: var(--ink); box-shadow: 0 6px 24px var(--shadow, #00000026); width: min(400px, calc(100vw - 32px)); }
  dialog::backdrop { background: #00000033; }
  .box { display: grid; gap: 10px; padding: 16px; }
  .ask { margin: 0; font-weight: 600; overflow-wrap: anywhere; }
  .why { margin: 0; color: var(--muted); font-size: 13.5px; line-height: 1.45; }
  .buttons { display: flex; gap: 6px; justify-content: flex-end; }
  .pill { padding: 6px 12px; border: 1px solid var(--border); border-radius: var(--radius); background: none; color: var(--ink); font: inherit; font-size: 13.5px; cursor: pointer; }
  .pill:hover { background: var(--hover); }
  .pill.primary { background: var(--primary); border-color: var(--primary); color: var(--on-accent); }
</style>
