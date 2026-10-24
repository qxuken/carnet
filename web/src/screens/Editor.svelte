<script lang="ts">
  // A note, written in as on the desktop: CodeMirror shows its words and
  // nothing else, and every edit is the desktop's own page (PageEditor, in
  // the wasm), which writes the note back as the desktop does. What's
  // written is sent half a second after typing pauses, and at once when
  // the page goes out of sight; what comes from another device meanwhile
  // is merged in. On a phone a toolbar rides on the keyboard.
  import { onDestroy, onMount } from 'svelte';
  import { EditorState } from '@codemirror/state';
  import { EditorView, keymap } from '@codemirror/view';
  import { defaultKeymap } from '@codemirror/commands';
  import { sync } from '../lib/sync.svelte';
  import { pageEditor, HIGHLIGHT, TURN } from '../lib/editor/page';
  import { theme } from '../lib/editor/theme';
  import { STYLES } from '../lib/editor/styles';
  import Icon from '../lib/Icon.svelte';
  import { overflow } from '../lib/words';
  import { focusNow } from '../lib/focus';
  import { ON_THE_WEB } from '../lib/pictures';
  import { HUES, inkOf, inkCss, washCss } from '../lib/colors';
  import { PageEditor, picture_sizes } from '../wasm/carnet_web.js';
  import { drawnPicture, picked as pictureHere, textWidth, type Picked } from '../lib/editor/picture';
  import { linkOpening } from '../lib/editor/links';
  import { commands, follow as followSlash, hintOf, iconOf, slashHere, slashKeys, type Slash } from '../lib/editor/slash';
  import { LIST_TURNS, toolSet } from '../lib/editor/tools';

  let props: {
    key: string;
    folder: string;
    fresh: boolean;
    /// A file that isn't a picture, picked for a note in `folder` (the
    /// paperclip, `/file`): kept beside the notes and synced, its `src` as
    /// the note links it. Without it the editor takes pictures alone: the
    /// paperclip is the picture chooser, and `/file` is not offered.
    addFile?: (folder: string, file: File) => Promise<string>;
  } = $props();
  // Which note, as it was opened: an editor is for one note (the reader
  // makes a new one for another), and its props may be gone by the time
  // it closes.
  let key = '';
  let fresh = false;
  let host: HTMLElement;
  let view: EditorView | undefined;
  let pe: PageEditor | undefined;
  let run: ((f: () => boolean) => (v: EditorView) => boolean) | undefined;
  let show: ((v: EditorView) => void) | undefined;
  /// The version the page's text was last the note's.
  let base = '';
  /// Written since it was last sent.
  let dirty = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  // The toolbar: on a touch screen or a phone-sized window, while the note
  // has the keyboard. iOS lays the keyboard over the page rather than
  // making the page shorter, so with it up the toolbar is put by hand.
  const touch = matchMedia('(pointer: coarse), (max-width: 759px)');
  let small = $state(touch.matches);
  let focused = $state(false);
  let keyboard = $state(false);
  /// What the caret's lines are, and the style typing has there.
  let turned = $state<Record<string, boolean>>({});
  let flags = $state(0);
  const BAR = 48;
  /// A sheet open over the toolbar: a link's address, the colours, or a
  /// picture's sizes.
  let sheet = $state<null | 'link' | 'color' | 'size'>(null);
  /// The picture the caret is on, if it is on one.
  let pic = $state<Picked | null>(null);
  /// The colours' tab: the words' own, or a highlighter.
  let inkTab = $state(true);
  let url = $state('');
  let linked = $state<string | null>(null);
  /// Where Open takes it: none for a link a page mustn't follow
  /// (`javascript:` and the like), which is still there to change.
  let opens = $state<string | null>(null);
  /// The words' colour and highlighter at the caret.
  let tint = $state<[number, number]>([0, 0]);
  /// How the caret's column was last sorted: 0 not, 1 A to Z, 2 Z to A.
  let sortedHow = $state(0);
  /// The caret's table has a header row.
  let tableHeader = $state(true);
  /// Which ways the caret's row and column can move: up, down, left, right.
  let tableMoves = $state([false, false, false, false]);
  /// What the caret is in (`PageEditor.kind_here`: `words`, `list`,
  /// `table`, `code`, `divider`, `picture`, `blocks`, `title`), which
  /// tools show; for a mix of blocks, what they hold (`blocks_here`).
  let kind = $state('words');
  let holds = $state(0);
  /// The caret's paragraphs' first letter: 0 no paragraph, 1 in the
  /// line, 2 dropped. Its to-do: 0 none, 1 not done, 2 done.
  let initial = $state(0);
  let ticked = $state(0);
  /// The `/` menu while it shows, its highlighted row, and where it
  /// hangs: under its line, or over it when there's no room below.
  let slash = $state<Slash | null>(null);
  let slashSel = $state(0);
  let slashAt = $state<{ left: number; top?: number; bottom?: number } | null>(null);
  let writing: HTMLElement;
  let chooser: HTMLInputElement | undefined = $state();
  let anyFile: HTMLInputElement | undefined = $state();
  let folderOf = '';

  /// Pictures chosen (or taken with the camera): kept, sent, and put in
  /// the note below the caret.
  async function picked(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const files = [...(input.files ?? [])];
    input.value = '';
    await take(files);
  }

  /// Files picked with the paperclip or `/file`, as the desktop takes
  /// them: a picture put in as a picture, every other file kept beside
  /// the notes (`addFile`) and linked among the caret's words.
  async function take(files: File[]) {
    if (!pe) return;
    const names: string[] = [];
    const srcs: string[] = [];
    for (const file of files) {
      try {
        if (file.type.startsWith('image/')) {
          const src = await sync.addPicture(folderOf, file);
          act((pe) => pe.insert_picture(src));
        } else if (props.addFile) {
          srcs.push(await props.addFile(folderOf, file));
          names.push(file.name);
        }
      } catch (err) {
        sync.say(err instanceof Error ? err.message : String(err));
      }
    }
    if (srcs.length) act((pe) => pe.insert_links(names, srcs));
  }

  /// The paperclip's dialog: pictures and files of every kind where the
  /// web can keep them, pictures alone where it can't.
  const pick = () => (props.addFile ? anyFile : chooser)?.click();

  /// `/` command `i` made of its line; a picture's or a file's then asks
  /// for them, to go in on the line it leaves empty.
  function runSlash(i: number) {
    const what = commands()[i]?.act;
    act((pe) => pe.run_slash(i));
    if (what === 'picture') chooser?.click();
    else if (what === 'file') pick();
  }

  /// The `/` menu as the page has it now. `/file` only where the web
  /// can keep a file.
  function slashNow(pe: PageEditor): Slash | null {
    const s = slashHere(pe);
    if (!s) return null;
    const shown = s.shown.filter((i) => commands()[i].act !== 'file' || !!props.addFile);
    return shown.length ? { ...s, shown } : null;
  }

  /// The menu hung from its line, once the view is drawn (CodeMirror's
  /// layout isn't read during its update): under it, at the text's start
  /// a little to the left, or over it when what's seen has no room below
  /// and has some above. Never over the line being typed.
  function hang() {
    requestAnimationFrame(() => {
      if (!slash || !view || !writing) return void (slashAt = null);
      const line = view.state.doc.line(Math.min(slash.line + 1, view.state.doc.lines));
      const at = view.coordsAtPos(line.from, 1);
      if (!at) return void (slashAt = null);
      const box = writing.getBoundingClientRect();
      const tall = 12 + slash.shown.length * 35;
      const vv = window.visualViewport;
      const foot = (vv ? vv.offsetTop + vv.height : innerHeight) - (small && bar ? barHeight : 0);
      const above = at.bottom + tall + 8 > foot && at.top - tall - 8 >= 0;
      const left = Math.max(0, Math.min(at.left - box.left - 16, box.width - Math.min(280, box.width)));
      slashAt = above ? { left, bottom: box.bottom - at.top + 2 } : { left, top: at.bottom - box.top + 2 };
    });
  }
  // On a phone, while the note has the keyboard (or a sheet is open); on
  // a wide screen, always, at the note's top.
  const bar = $derived(small ? focused || sheet !== null : true);

  /// Where the visible part of the page is (the visual viewport, in the
  /// layout viewport's terms): the toolbar's foot is kept at its foot.
  let seen = $state<[number, number]>([0, 0]);
  let barHeight = $state(48);

  function place() {
    const vv = window.visualViewport;
    if (!vv) return;
    // Up when what's seen is well short of the page's window (as
    // CodeMirror tells). Not from `innerHeight`: Safari shrinks that too,
    // as the page scrolls under the keyboard.
    keyboard = (vv.height * vv.scale) / document.documentElement.clientHeight < 0.85;
    seen = [vv.offsetTop, vv.height];
  }

  /// `place`, again each frame for a moment: iOS moves the page and the
  /// keyboard in an animation, and says so only at the start.
  let following = 0;
  function follow() {
    place();
    cancelAnimationFrame(following);
    let n = 0;
    const step = () => {
      place();
      if (++n < 30) following = requestAnimationFrame(step);
    };
    following = requestAnimationFrame(step);
  }

  /// The toolbar's states, read from the page.
  function moved() {
    if (!pe || !view) return;
    focused = view.hasFocus || !!document.activeElement?.closest('.cm-tablewrap');
    sortedHow = pe.sorted();
    tableHeader = pe.table_header();
    tableMoves = Array.from(pe.table_moves(), (n) => n === 1);
    turned = Object.fromEntries(Object.entries(TURN).map(([k, v]) => [k, pe!.turned_to(v)]));
    flags = pe.style_here();
    const t = pe.tint_here();
    tint = [t[0], t[1]];
    pic = pictureHere(pe);
    if (!pic && sheet === 'size') sheet = null;
    kind = pe.kind_here();
    holds = pe.blocks_here();
    initial = pe.initial_here();
    ticked = pe.tick_here();
    const s = slashNow(pe);
    slashSel = followSlash(slash, s, slashSel);
    slash = s;
    hang();
  }

  function openLink() {
    if (!pe || !view) return;
    linked = pe.link_here() ?? null;
    opens = pe.link_to_open() ?? null;
    url = linked ?? '';
    sheet = 'link';
  }

  function applyLink(to: string | null) {
    if (!pe || !view || !run) return;
    run(() => pe!.set_link(to ?? undefined))(view);
    sheet = null;
    view.focus();
    moved();
  }

  /// A page command from the toolbar, on the note, which keeps the keyboard.
  function act(f: (pe: PageEditor) => boolean) {
    if (!view || !pe || !run) return;
    run(() => f(pe!))(view);
    // The caret back to the note's text, unless it is in a table's cell
    // (which `run` has given the focus).
    if (!pe.cell_caret()) view.focus();
    moved();
  }

  /// A heading, the next level, back to text past the third.
  const heading = (pe: PageEditor) =>
    pe.turned_to(TURN.h1) ? pe.turn(TURN.h2) : pe.turned_to(TURN.h2) ? pe.turn(TURN.h3) : pe.turned_to(TURN.h3) ? pe.turn(TURN.h3) : pe.turn(TURN.h1);

  type Tool = { icon: () => string; label: string; f: (pe: PageEditor) => boolean; on?: () => boolean; off?: () => boolean };

  /// In a table's cell: its rows and columns. A move past the table's
  /// edge, or the header's up or down, is greyed.
  const tableTools: Tool[] = [
    { icon: () => 'add-row', label: 'Add a row below', f: (pe) => pe.table_op(0) },
    { icon: () => 'add-col', label: 'Add a column after', f: (pe) => pe.table_op(1) },
    { icon: () => 'del-row', label: 'Remove the row', f: (pe) => pe.table_op(2) },
    { icon: () => 'del-col', label: 'Remove the column', f: (pe) => pe.table_op(3) },
    { icon: () => 'row-up', label: 'Move the row up', f: (pe) => pe.table_op(5), off: () => !tableMoves[0] },
    { icon: () => 'row-down', label: 'Move the row down', f: (pe) => pe.table_op(6), off: () => !tableMoves[1] },
    { icon: () => 'col-left', label: 'Move the column left', f: (pe) => pe.table_op(7), off: () => !tableMoves[2] },
    { icon: () => 'col-right', label: 'Move the column right', f: (pe) => pe.table_op(8), off: () => !tableMoves[3] },
    { icon: () => 'table-header', label: 'Header row', f: (pe) => pe.table_op(4), on: () => tableHeader },
    {
      icon: () => ['sort', 'sort-up', 'sort-down'][sortedHow],
      label: 'Sort by this column',
      f: (pe) => pe.sort_table(),
      on: () => sortedHow !== 0,
    },
    { icon: () => 'trash', label: 'Remove the table', f: (pe) => pe.remove_table() },
  ];

  /// A picture selected: where it sits and its size. What a block becomes
  /// and the words' styles do nothing to it, so these stand in their
  /// place, as on the desktop; it goes up, down and away as any block.
  const placeTools: Tool[] = [
    { icon: () => 'inline', label: 'On its own line', f: (pe) => pe.set_picture_float(0, false), on: () => pic?.float === 0 },
    { icon: () => 'float-left', label: 'Left of the text', f: (pe) => pe.set_picture_float(1, false), on: () => pic?.float === 1 },
    { icon: () => 'float-right', label: 'Right of the text', f: (pe) => pe.set_picture_float(2, false), on: () => pic?.float === 2 },
    { icon: () => 'resize', label: 'Size', f: () => ((sheet = sheet === 'size' ? null : 'size'), false), on: () => sheet === 'size' },
  ];

  /// The sizes a picture is given from the list: shares of the text's
  /// width, the desktop's.
  const SIZES: [string, number][] = JSON.parse(picture_sizes());
  const sizeOf = (pct: number) => (view ? Math.round((textWidth(view) * pct) / 100) : 0);
  /// The size it is drawn at now is `pct`'s (to within 2, as the desktop ticks it).
  function sizeIs(p: Picked, pct: number) {
    const el = view && drawnPicture(view, p.line);
    return !!el && Math.abs(el.getBoundingClientRect().width - sizeOf(pct)) < 2;
  }

  /// What the block becomes; a paragraph's first letter dropped, greyed
  /// on a heading or a quote, which share the paragraph's tools.
  const turnTools: Tool[] = [
    { icon: () => 'todo', label: 'Checklist', f: (pe) => pe.turn(TURN.todo), on: () => turned.todo },
    { icon: () => 'bullets', label: 'Bulleted list', f: (pe) => pe.turn(TURN.bullet), on: () => turned.bullet },
    { icon: () => 'numbers', label: 'Numbered list', f: (pe) => pe.turn(TURN.number), on: () => turned.number },
    { icon: () => (turned.h1 ? 'h2' : 'h1'), label: 'Heading', f: heading, on: () => turned.h1 || turned.h2 || turned.h3 },
    { icon: () => 'quote', label: 'Quote', f: (pe) => pe.turn(TURN.quote), on: () => turned.quote },
    { icon: () => 'initial', label: 'Drop the first letter', f: (pe) => pe.toggle_initial(), on: () => initial === 2, off: () => initial === 0 },
  ];
  /// A list's: in and out, and a to-do's tick (Ctrl+Enter's).
  const listTools: Tool[] = [
    { icon: () => 'outdent', label: 'Outdent', f: (pe) => pe.tab(true) },
    { icon: () => 'indent', label: 'Indent', f: (pe) => pe.tab(false) },
  ];
  const tickTool: Tool = { icon: () => 'check', label: 'Done', f: (pe) => pe.tick(), on: () => ticked === 2 };
  const styleTools: Tool[] = [
    ...STYLES.filter((s) => s.tool).map((s): Tool => ({ icon: () => s.icon, label: s.label, f: (pe) => pe.style(s.which), on: () => !!(flags & s.flag) })),
    { icon: () => 'highlight', label: 'Highlight', f: (pe) => pe.tint(false, flags & 32 ? 0 : HIGHLIGHT), on: () => !!(flags & 32) },
    { icon: () => 'text-color', label: 'Colour', f: () => ((sheet = sheet === 'color' ? null : 'color'), false), on: () => sheet === 'color' || !!tint[0] },
    { icon: () => 'link', label: 'Link', f: () => (sheet === 'link' ? (sheet = null) : openLink(), false), on: () => sheet === 'link' },
  ];
  /// The paperclip: pictures and files where the web can keep files, the
  /// picture chooser where it can't.
  const clipTool: Tool = $derived(
    props.addFile
      ? { icon: () => 'attach', label: 'Add pictures or files', f: () => (pick(), false) }
      : { icon: () => 'picture', label: 'Picture', f: () => (pick(), false) },
  );
  const insertTools: Tool[] = $derived([
    { icon: () => 'table', label: 'Table', f: (pe) => pe.insert_table() },
    { icon: () => 'divider', label: 'Divider', f: (pe) => pe.insert_divider() },
    clipTool,
  ]);
  /// The block's own: up, down, away (Alt+↑ and ↓ move it too).
  const moveTools: Tool[] = [
    { icon: () => 'up', label: 'Move up', f: (pe) => pe.move_blocks(-1) },
    { icon: () => 'down', label: 'Move down', f: (pe) => pe.move_blocks(1) },
    { icon: () => 'cross', label: 'Remove block', f: (pe) => pe.remove_blocks() },
  ];
  const undoTool: Tool = { icon: () => 'undo', label: 'Undo', f: (pe) => pe.undo() };

  /// The tools for what the caret is in, as on the desktop (tools.ts):
  /// never one that does nothing there. In a list, its kinds, not a
  /// heading, a quote or a first letter, which are a paragraph's.
  const groups = $derived(
    toolSet(kind, holds).map((g): Tool[] => {
      switch (g) {
        case 'turn':
          return kind === 'list' ? turnTools.filter((t) => LIST_TURNS.has(t.label)) : turnTools;
        case 'list':
          return ticked ? [...listTools, tickTool] : listTools;
        case 'styles':
          return styleTools;
        case 'insert':
          return insertTools;
        case 'clip':
          return [clipTool];
        case 'moves':
          return moveTools;
        case 'table':
          return tableTools;
        case 'place':
          return placeTools;
        case 'undo':
          return [undoTool];
      }
    }),
  );
  /// A screen reader's name for the strip, as the desktop's: its set.
  const SET_NAMES: Record<string, string> = {
    title: 'Insert tools',
    words: 'Text tools',
    list: 'List tools',
    table: 'Table tools',
    code: 'Code block tools',
    divider: 'Divider tools',
    picture: 'Picture tools',
    blocks: 'Block tools',
  };

  /// What's written, sent; what came meanwhile, merged in.
  function send() {
    clearTimeout(timer);
    if (!pe || !dirty) return;
    dirty = false;
    const md = pe.markdown();
    const r = sync.edit(key, base, md);
    if (!r) return;
    base = r[1];
    if (r[0] !== md && view && show) {
      pe.reload(r[0]);
      show(view);
    }
  }

  function edited() {
    dirty = true;
    // Kept apart at once, before it's sent and the rest kept: a reload
    // or a closed tab loses nothing typed (pending.ts).
    if (pe) sync.pending.typed(key, base, pe.markdown(), fresh ? props.folder : null);
    clearTimeout(timer);
    timer = setTimeout(send, 500);
  }

  // Changes from another device: merged with what's written (sent
  // first), or taken as they are.
  $effect(() => {
    void sync.version;
    if (!view || !pe || !key || !show) return;
    const src = sync.source(key);
    if (!src || src[1] === base) return;
    if (dirty) send();
    else {
      base = src[1];
      if (src[0] !== pe.markdown()) {
        pe.reload(src[0]);
        show(view);
      }
    }
  });

  const hidden = () => {
    if (document.visibilityState === 'hidden') leaving();
  };
  /// Out of sight, or the page going (a reload, the tab closed): sent and
  /// kept now. The words typed are kept apart already (`edited`).
  const leaving = () => {
    send();
    void sync.keepNow();
  };

  /// Which ends of the tools have more past them, at any width: a fade
  /// and an arrow there say so, and the arrow scrolls to them.
  let more = $state({ start: false, end: false });
  function ends(el: HTMLElement) {
    const look = () => {
      const m = overflow(el.scrollLeft, el.clientWidth, el.scrollWidth);
      if (m.start !== more.start || m.end !== more.end) more = m;
    };
    const resized = new ResizeObserver(look);
    resized.observe(el);
    // Other tools (in a table, on a picture): another width.
    const changed = new MutationObserver(look);
    changed.observe(el, { childList: true });
    el.addEventListener('scroll', look, { passive: true });
    look();
    return {
      destroy() {
        resized.disconnect();
        changed.disconnect();
        el.removeEventListener('scroll', look);
      },
    };
  }
  let toolsEl: HTMLElement | undefined = $state();
  const scrollTools = (forward: boolean) => toolsEl?.scrollBy({ left: (forward ? 1 : -1) * toolsEl.clientWidth * 0.7, behavior: 'smooth' });

  onMount(() => {
    key = props.key;
    fresh = props.fresh;
    const folder = props.folder;
    folderOf = folder;
    const src = sync.source(key) ?? ['', ''];
    base = src[1];
    pe = new PageEditor(src[0]);
    const pictures = (s: string, wiki: boolean) => {
      if (/^data:/i.test(s)) return Promise.resolve(s);
      // On the web: not loaded (pictures.ts).
      if (/^https?:/i.test(s)) return Promise.reject(new Error(ON_THE_WEB));
      const path = sync.picturePath(folder, s, wiki);
      // Outside the notes folder: only on the device that has it.
      return path ? sync.picture(path) : Promise.reject(new Error('not in the vault'));
    };
    const page = pageEditor(pe, { pictures, edited, moved });
    run = page.run;
    show = page.show;
    const text = pe.text();
    // A new note starts in its title; another, at the end of its words.
    const lines = text.split('\n');
    const last = lines.length - 1;
    if (fresh) pe.select(0, 0, 0, 0);
    else pe.select(last, lines[last].length, last, lines[last].length);
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: text,
        selection: { anchor: fresh ? 0 : text.length },
        extensions: [
          ...page.extensions,
          linkOpening(pe),
          slashKeys({
            menu: () => slash,
            sel: () => slashSel,
            choose: (n) => (slashSel = n),
            take: runSlash,
            close: () => {
              pe?.slash_close();
              moved();
            },
          }),
          keymap.of([{ key: 'Mod-k', run: () => (openLink(), true) }, { key: 'Escape', run: () => (sheet !== null ? ((sheet = null), true) : false) }]),
          keymap.of(defaultKeymap),
          theme,
          EditorView.lineWrapping,
          EditorView.contentAttributes.of({ spellcheck: 'true', autocorrect: 'on', autocapitalize: 'sentences', 'aria-label': 'Note' }),
          // The caret kept clear of the toolbar (CodeMirror keeps it clear
          // of the keyboard, measuring what's seen).
          EditorView.scrollMargins.of(() => ({ bottom: (bar ? BAR : 0) + 12 })),
          EditorView.updateListener.of((u) => {
            if (u.focusChanged) focused = u.view.hasFocus || !!document.activeElement?.closest('.cm-tablewrap');
          }),
        ],
      }),
    });
    view.focus();
    moved();
    document.addEventListener('visibilitychange', hidden);
    addEventListener('pagehide', leaving);
    touch.onchange = () => (small = touch.matches);
    place();
    window.visualViewport?.addEventListener('resize', follow);
    window.visualViewport?.addEventListener('scroll', follow);
    window.addEventListener('scroll', follow);
  });

  onDestroy(() => {
    document.removeEventListener('visibilitychange', hidden);
    removeEventListener('pagehide', leaving);
    touch.onchange = null;
    window.visualViewport?.removeEventListener('resize', follow);
    window.visualViewport?.removeEventListener('scroll', follow);
    window.removeEventListener('scroll', follow);
    cancelAnimationFrame(following);
    // A new note left with nothing in it isn't kept.
    if (fresh && pe && !pe.markdown().trim()) sync.discard(key);
    else send();
    view?.destroy();
    pe?.free();
  });
</script>

{#snippet toolbar()}
  <!-- Pressed without taking the keyboard from the note. -->
  <div class="bar" class:fixed={small} role="toolbar" aria-label="Formatting" tabindex="-1" style:top={small && keyboard ? `${seen[0] + seen[1] - barHeight}px` : null} class:keyboard={small && keyboard} bind:offsetHeight={barHeight} onpointerdown={(e) => e.preventDefault()}>
    {#if sheet === 'color'}
      <div class="sheet" role="dialog" aria-label="Colour">
        <div class="tabs" role="tablist">
          <button role="tab" aria-selected={inkTab} class:on={inkTab} onclick={() => (inkTab = true)}>Text</button>
          <button role="tab" aria-selected={!inkTab} class:on={!inkTab} onclick={() => (inkTab = false)}>Highlight</button>
        </div>
        <div class="hues">
          {#each HUES as h (h.name)}
            {@const c = inkTab ? inkOf(h) : h.wash}
            <button
              class="hue"
              class:picked={(inkTab ? tint[0] : tint[1]) === c}
              aria-label={h.name}
              title={h.name}
              style:--swatch={inkTab ? inkCss(c) : washCss(c)}
              onclick={() => act((pe) => pe.tint(inkTab, c))}
            >{#if inkTab}A{/if}</button>
          {/each}
        </div>
        <div class="row">
          <button class="pill" onclick={() => act((pe) => pe.tint(inkTab, 0))}><Icon name="cross" size={14} /> No colour</button>
          {#if !inkTab}
            <button class="pill" onclick={() => act((pe) => pe.tint(false, HIGHLIGHT))}><span class="dot"></span> Highlight</button>
          {/if}
        </div>
      </div>
    {:else if sheet === 'size' && pic}
      {@const p = pic}
      <div class="sheet" role="dialog" aria-label="Picture size">
        <div class="row">
          {#each SIZES as [name, pct] (pct)}
            <button class="pill" class:on={sizeIs(p, pct)} aria-pressed={sizeIs(p, pct)} onclick={() => act((pe) => pe.set_picture_width(sizeOf(pct)))}>{name}</button>
          {/each}
          <button class="pill" disabled={!p.width} onclick={() => act((pe) => pe.set_picture_width(undefined))}>Original size</button>
        </div>
        <p class="recent">Or drag the handles on the picture.</p>
      </div>
    {:else if sheet === 'link'}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div role="dialog" aria-label="Link" tabindex="-1" onpointerdown={(e) => e.stopPropagation()}>
      <form class="sheet link" onsubmit={(e) => { e.preventDefault(); applyLink(url.trim() || null); }}>
        <input bind:value={url} placeholder="example.com" inputmode="url" autocapitalize="off" autocomplete="off" spellcheck="false" aria-label="Address" use:focusNow />
        <div class="row">
          {#if opens}
            <a class="pill" href={opens} target="_blank" rel="noopener noreferrer">Open</a>
          {/if}
          {#if linked}
            <button type="button" class="pill" onclick={() => applyLink(null)}>Remove link</button>
          {/if}
          <button type="button" class="pill" onclick={() => { sheet = null; view?.focus(); }}>Cancel</button>
          <button class="pill primary" disabled={!url.trim()}>{linked ? 'Change' : 'Link'}</button>
        </div>
      </form>
      </div>
    {/if}
    <div class="strip">
      <div class="tools-wrap" class:more-start={more.start} class:more-end={more.end}>
      {#if more.start}
        <button class="more start" tabindex="-1" aria-hidden="true" title="More tools" onclick={() => scrollTools(false)}><Icon name="chevron-right" size={18} /></button>
      {/if}
      <div class="tools" role="group" aria-label={SET_NAMES[kind] ?? 'Tools'} bind:this={toolsEl} use:ends>
        {#each groups as g, n (g)}
          {#if n > 0}<span class="gap"></span>{/if}
          {#each g as t (t.label)}
            <button class:on={t.on?.()} disabled={t.off?.()} aria-label={t.label} aria-pressed={t.on ? t.on() : undefined} title={t.label} onclick={() => act(t.f)}>
              <Icon name={t.icon()} size={20} />
            </button>
          {/each}
        {/each}
      </div>
      {#if more.end}
        <button class="more end" tabindex="-1" aria-hidden="true" title="More tools" onclick={() => scrollTools(true)}><Icon name="chevron-right" size={18} /></button>
      {/if}
      </div>
      {#if small}
        <button class="away" aria-label="Hide the keyboard" title="Hide the keyboard" onclick={() => { sheet = null; view?.contentDOM.blur(); }}>
          <Icon name="chevron-down" size={20} />
        </button>
      {/if}
    </div>
  </div>
{/snippet}

{#if bar && !small}{@render toolbar()}{/if}
<div class="writing" bind:this={writing}>
  <div class="editor" class:barred={bar && small} bind:this={host}></div>
  {#if slash && slashAt && focused}
    <!-- The `/` menu: pressed without taking the keyboard from the note. -->
    <div class="slash" role="listbox" aria-label="Blocks" tabindex="-1" style:left="{slashAt.left}px" style:top={slashAt.top !== undefined ? `${slashAt.top}px` : null} style:bottom={slashAt.bottom !== undefined ? `${slashAt.bottom}px` : null} onpointerdown={(e) => e.preventDefault()}>
      {#each slash.shown as i, row (i)}
        {@const c = commands()[i]}
        <button role="option" aria-selected={row === slashSel} class:on={row === slashSel} onclick={() => runSlash(i)}>
          <Icon name={iconOf(i)} size={18} />
          <span class="name">{c.name}</span>
          <span class="hint">{hintOf(c)}</span>
        </button>
      {/each}
    </div>
  {/if}
</div>
<input class="chooser" type="file" accept="image/*" multiple bind:this={chooser} onchange={picked} tabindex="-1" aria-hidden="true" />
{#if props.addFile}
  <input class="chooser" type="file" multiple bind:this={anyFile} onchange={picked} tabindex="-1" aria-hidden="true" />
{/if}
{#if bar && small}{@render toolbar()}{/if}

<style>
  /* A container, for the pictures' widths (theme.ts `cqi`): the text's. */
  .editor { min-height: 60vh; container-type: inline-size; }
  /* The `/` menu, as the desktop's: a card under its line, each row a
     command's icon, name and what to type for it next time, the one
     Enter takes on the accent's wash. */
  .writing { position: relative; }
  .slash {
    position: absolute; z-index: 6; width: 280px; max-width: 100%; padding: 6px; display: flex; flex-direction: column; gap: 1px;
    border-radius: var(--radius-card); background: var(--card); border: 1px solid var(--border);
    box-shadow: 0 4px 18px color-mix(in srgb, #000 16%, transparent); animation: hang 140ms ease-out;
  }
  @keyframes hang { from { opacity: 0; transform: translateY(-4px); } }
  .slash button {
    width: 100%; height: 34px; padding: 0 10px; gap: 10px; justify-content: flex-start; border-radius: var(--radius); color: var(--muted);
  }
  .slash button:hover { background: var(--hover); }
  .slash button.on { background: var(--accent-soft); color: var(--ink); }
  .slash .name { flex: 1; min-width: 0; text-align: left; font: 600 14px var(--sans); color: var(--ink); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .slash .hint { font: 12.5px var(--mono); color: var(--muted); white-space: nowrap; }
  .chooser { display: none; }
  .editor.barred { padding-bottom: 64px; }
  .bar { z-index: 5; display: flex; flex-direction: column; }
  /* A wide screen: under the note's header, as the desktop's strip. */
  .bar:not(.fixed) {
    position: sticky; top: 52px; margin: 0 0 14px; border-radius: var(--radius-card);
    background: color-mix(in srgb, var(--side) 92%, transparent); backdrop-filter: blur(12px); border: 1px solid var(--border);
  }
  .bar.fixed {
    position: fixed; left: 0; right: 0; bottom: 0; padding-bottom: env(safe-area-inset-bottom);
    background: color-mix(in srgb, var(--side) 92%, transparent); backdrop-filter: blur(12px);
    border-top: 1px solid var(--border); touch-action: manipulation;
  }
  /* On the keyboard: its foot at the foot of what's seen (set as `top`,
     from the visual viewport, which iOS moves with the page), and the
     phone's own edge under the keyboard. */
  .bar.keyboard { bottom: auto; padding-bottom: 0; }
  .strip { display: flex; align-items: center; height: 48px; }
  .tools { display: flex; align-items: center; flex: 1; min-width: 0; overflow-x: auto; scrollbar-width: none; padding: 0 6px; }
  .tools::-webkit-scrollbar { display: none; }
  /* More tools past an end: they fade out there, under an arrow that
     scrolls to them. */
  .tools-wrap { position: relative; display: flex; flex: 1; min-width: 0; }
  .tools-wrap.more-end .tools { mask-image: linear-gradient(to right, #000 calc(100% - 64px), transparent calc(100% - 16px)); }
  .tools-wrap.more-start .tools { mask-image: linear-gradient(to left, #000 calc(100% - 64px), transparent calc(100% - 16px)); }
  .tools-wrap.more-start.more-end .tools {
    mask-image: linear-gradient(to right, transparent 16px, #000 64px, #000 calc(100% - 64px), transparent calc(100% - 16px));
  }
  .more { position: absolute; top: 5px; z-index: 1; width: 32px; color: var(--muted); }
  .more:hover { color: var(--ink); }
  .more.end { right: 0; }
  .more.start { left: 0; transform: scaleX(-1); }
  button {
    flex: none; display: inline-flex; align-items: center; justify-content: center; width: 42px; height: 38px;
    border: 0; border-radius: var(--radius); background: none; color: var(--ink); cursor: pointer;
  }
  button.on { background: var(--accent-soft); color: var(--accent-ink); }
  button:disabled { opacity: 0.35; cursor: default; }
  .gap { flex: none; width: 1px; height: 22px; margin: 0 4px; background: var(--border-strong); }
  .away { border-left: 1px solid var(--border); border-radius: 0; width: 48px; height: 48px; color: var(--muted); }
  .sheet { padding: 12px 14px 4px; border-bottom: 1px solid var(--border); display: flex; flex-direction: column; gap: 10px; }
  .bar:not(.fixed) .sheet { order: 1; border-bottom: 0; border-top: 1px solid var(--border); padding-bottom: 12px; }
  .tabs { display: flex; gap: 4px; padding: 3px; border-radius: var(--radius); background: var(--sunken); align-self: flex-start; }
  .tabs button { width: auto; height: 30px; padding: 0 14px; font-size: 14px; color: var(--muted); }
  .tabs button.on { background: var(--card); color: var(--ink); }
  .hues { display: grid; grid-template-columns: repeat(10, 1fr); gap: 6px; max-width: 420px; }
  .hue {
    width: 100%; aspect-ratio: 1; height: auto; border-radius: 50%; background: var(--card); color: var(--swatch);
    font: 600 15px var(--sans); box-shadow: inset 0 0 0 1px var(--border-strong);
  }
  .hues .hue:not(:empty) { background: var(--card); }
  .hues .hue:empty { background: var(--swatch); }
  .hue.picked { box-shadow: inset 0 0 0 2px var(--accent); }
  .row { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
  .recent { margin: 0; font-size: 12.5px; color: var(--muted); }
  .pill {
    width: auto; height: 32px; padding: 0 12px; gap: 6px; border-radius: 999px; background: var(--sunken); font-size: 14px;
    text-decoration: none; color: var(--ink);
  }
  .pill.primary { background: var(--primary); color: var(--on-accent); }
  .pill.on { background: var(--accent-soft); color: var(--accent-ink); font-weight: 600; }
  .pill:disabled { opacity: 0.45; }
  .dot { width: 12px; height: 12px; border-radius: 3px; background: var(--highlight); }
  .link input {
    font: 16px var(--sans); padding: 9px 12px; border-radius: var(--radius); border: 1px solid var(--border-strong);
    background: var(--card); color: var(--ink); width: 100%;
  }
  .link .row { justify-content: flex-end; }
</style>
