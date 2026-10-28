<script lang="ts">
  // Coming in, in two steps: how (only the ways this server lists: a code
  // from a device already in or an invite, a passkey, an email and
  // password, a new account), then that way's form. A server that offers
  // only a code opens on its form, and so does a pairing or invite link,
  // its code filled in, to connect with a click: a link anyone could send
  // mustn't sign this browser into their account unasked.
  // The kind of code is read from its shape.
  import { onMount } from 'svelte';
  import { sync } from '../lib/sync.svelte';
  import * as passkey from '../lib/passkey';
  import Icon from '../lib/Icon.svelte';
  import Scan from './Scan.svelte';
  import { code_from_link, server_fit } from '../wasm/carnet_web.js';
  import { linkedCode, serverSays, shownPage } from '../lib/words';

  /// The steps: choosing a way in, then a way's form. Null until the
  /// server has said what it offers.
  type Way = 'code' | 'passkey' | 'password' | 'words' | 'signup';
  type Step = 'choose' | Way;
  let step = $state<Step | null>(null);
  /// The step left last, for Back to come back to its row.
  let left = $state<Step | null>(null);
  /// Moved by a click (not opened on): the new step's field takes focus.
  let moved = false;

  let code = $state('');
  /// The code a pairing or invite link brought, said so while it's the one
  /// in the field.
  let fromLink = $state<string | null>(null);
  let email = $state('');
  let password = $state('');
  let showPassword = $state(false);
  let scanning = $state(false);
  const camera = !!navigator.mediaDevices?.getUserMedia;
  const installed = matchMedia('(display-mode: standalone)').matches || (navigator as { standalone?: boolean }).standalone === true;
  // On an iPhone, Safari and the Home Screen keep apart what a site
  // stores: paired in Safari, the Home Screen's Carnet would start empty.
  const safari = /iPhone|iPad/.test(navigator.userAgent) && !installed;

  /// What the server offers. Passwords and passkeys are offered unless it
  /// says not (an older server, or none reached); a new account only if
  /// it says so.
  let passwords = $state(true);
  let passkeys = $state(true);
  let signups = $state(false);
  /// Where this server makes accounts on its own page, if it has one (a
  /// hosted service from before invites were asked for here).
  let signupPage = $state<string | null>(null);
  /// A password here signs in with the email the account signed up with
  /// (a hosted service holds them to it).
  let signupEmail = $state(false);
  /// Whether this browser can use a passkey Carnet can.
  let browserPasskeys = $state(false);
  /// Why this app and the server can't sync, if they can't (`compat`).
  let outdated = $state('');

  /// New here: what's typed in the email field, an invite being asked
  /// for, the email it was sent to, and what the server said.
  let signupAddress = $state('');
  let sending = $state(false);
  let sentTo = $state<string | null>(null);
  let sentSaid = $state('');
  /// Asked again within the server's mail limit: nothing went. When the
  /// last one did, and when another may be asked for, in seconds.
  let lately = $state<{ ago: number; again: number } | null>(null);
  let signupError = $state('');

  type Info = { signup?: string; passwords?: boolean; password_email?: string; protocol?: number; oldest?: number; deprecated?: number; features?: string[] };
  onMount(() => {
    // Opened by a pairing or invite link: its code, in its form, to
    // connect with a click.
    if (sync.linked) {
      code = fromLink = sync.linked;
      step = 'code';
    }
    const info = fetch('/v1/info', { signal: AbortSignal.timeout(8000) })
      .then((r) => (r.ok ? (r.json() as Promise<Info>) : null))
      .catch(() => null);
    const can = passkey.support().catch(() => 'no' as const);
    void Promise.all([info, can]).then(([i, s]) => {
      if (i) {
        const f = i.features;
        passwords = f ? f.includes('passwords') : (i.passwords ?? true);
        if (f) passkeys = f.includes('passkeys');
        // A hosted service from before signups were listed said only its page.
        signups = !!f?.includes('signup') || !!i.signup;
        // Shown only if it's plainly a web page.
        signupPage = shownPage(i.signup, location.protocol);
        signupEmail = i.password_email === 'account';
        outdated = server_fit(i.protocol ?? 0, i.oldest ?? 0, i.deprecated ?? 0);
      }
      browserPasskeys = s !== 'no';
      step ??= choices.length > 1 ? 'choose' : 'code';
    });
  });

  /// The ways in this server and browser offer, as rows: a code always.
  const choices = $derived(
    (
      [
        { way: 'code', icon: 'device', title: 'Enter the code', line: 'A code from another of your devices, or an invite', shown: true },
        { way: 'passkey', icon: 'passkey', title: 'Use a passkey', line: 'Saved on this device or your phone', shown: passkeys && browserPasskeys },
        {
          way: 'password',
          icon: 'key',
          title: 'Email and password',
          line: signupEmail ? 'The email you signed up with, and your password' : 'The ones set on another of your devices',
          shown: passwords,
        },
        { way: 'signup', icon: 'person-add', title: 'Make an account', line: 'We’ll email you an invite', shown: signups },
      ] as const
    ).filter((c) => c.shown),
  );
  /// Whether there's a choosing step: more than a code on offer.
  const stepped = $derived(choices.length > 1);
  /// Where Back goes: to the choosing, if there is one; from the words,
  /// else, to the code.
  const backTo = $derived<Step | null>(step === 'choose' || step === null ? null : stepped ? 'choose' : step === 'words' ? 'code' : null);

  // A link opened in this tab while it's open here: its code, in its
  // form, as one it was opened by; but only into an empty form, and never
  // while connecting. Something else typed, the page asks first, so a
  // link opened again and again can't change a code unasked.
  let linkSeen = sync.linked;
  /// A link's code, waiting for a yes before it takes the form's place.
  let askLink = $state<string | null>(null);
  $effect(() => {
    const l = sync.linked;
    if (!l || l === linkSeen) return;
    linkSeen = l;
    const typed = step === 'password' ? email + password : step === 'signup' ? signupAddress : code;
    const act = linkedCode(l, typed, sync.joining);
    if (act === 'fill') useLink(l);
    else if (act === 'ask') askLink = l;
  });

  /// The link's code in its form.
  function useLink(l: string) {
    askLink = null;
    left = step;
    code = fromLink = l;
    step = 'code';
    sync.error = null;
  }

  /// To another step: what's typed for a code goes, and so does any error.
  /// A passkey's begins at once, the browser asking which.
  function to(s: Step) {
    left = step;
    moved = true;
    step = s;
    code = '';
    signupError = '';
    sync.error = null;
    if (s === 'passkey') void sync.signInWithPasskey();
  }

  /// A step's field, focused as it opens (when moved to, not opened on).
  const focused = (el: HTMLElement) => {
    if (moved) el.focus();
  };
  /// Back at the choosing: the row just left, focused.
  const focusedIf = (el: HTMLElement, yes: boolean) => {
    if (yes && moved) el.focus();
  };

  /// What the camera read: a link from another device, or a bare code.
  function scanned(text: string) {
    scanning = false;
    sync.error = null;
    try {
      const at = new URL(text);
      if (at.origin !== location.origin) {
        sync.error = `That code is for Carnet at ${at.host}. Open that address on this device, and pair there.`;
        return;
      }
    } catch {
      // Not a link: the code alone.
    }
    const c = code_from_link(text);
    if (!c) {
      sync.error = 'That QR code isn’t a Carnet pairing code.';
      return;
    }
    code = c;
    sync.pair(c);
  }

  /// The words in a server's answer: its JSON's `error` or `message`, or
  /// its text if short and not a page. Shown as the server's, not
  /// Carnet's: on one line, cut short, "The server says: …" (as
  /// `session::server_says` has it).
  function said(body: string): string {
    let s = '';
    try {
      const j = JSON.parse(body) as { error?: unknown; message?: unknown };
      s = typeof j?.error === 'string' ? j.error : typeof j?.message === 'string' ? j.message : '';
    } catch {
      const t = body.trim();
      s = t.length <= 300 && !t.startsWith('<') ? t : '';
    }
    return serverSays(s);
  }

  /// A yes that mailed nothing: asked again within the server's mail
  /// limit, `{"sent": false, "mailed_ago", "again_in"}` (as its own signup
  /// page reads it). Null for an invite sent now.
  function mailedLately(body: string): { ago: number; again: number } | null {
    try {
      const j = JSON.parse(body) as { sent?: unknown; mailed_ago?: unknown; again_in?: unknown };
      if (j?.sent !== false) return null;
      const n = (v: unknown) => (typeof v === 'number' && v > 0 ? v : 0);
      return { ago: n(j.mailed_ago), again: n(j.again_in) };
    } catch {
      return null;
    }
  }

  /// Said of an invite asked for too soon, as the desktop says it: when the
  /// last one went, and when another may be asked for.
  function latelyLine(to: string, l: { ago: number; again: number }) {
    const minutes = (n: number) => (n === 1 ? '1 minute' : `${n} minutes`);
    const ago = l.ago < 60 ? 'a moment ago' : `${minutes(Math.round(l.ago / 60))} ago`;
    return `We emailed ${to} ${ago}. Look for it in your inbox (and the spam folder), and paste its code here. You can ask for another email in ${minutes(Math.max(1, Math.ceil(l.again / 60)))}.`;
  }

  /// An invite asked for, here: the server mails it, and its code goes in
  /// the code form, focused.
  async function sendInvite(e: SubmitEvent) {
    e.preventDefault();
    const address = signupAddress.trim();
    if (!address || sending) return;
    if (!/^[^\s@]+@[^\s@]+$/.test(address)) {
      signupError = 'That doesn’t look like an email address.';
      return;
    }
    sending = true;
    signupError = '';
    try {
      const r = await fetch('/v1/signup', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email: address }) });
      const body = await r.text().catch(() => '');
      const words = said(body);
      if (r.ok) {
        sentTo = address;
        sentSaid = r.status === 204 ? '' : words;
        lately = mailedLately(body);
        if (lately) sentSaid = '';
        to('code');
      } else if ((r.status === 404 || r.status === 405) && signupPage) {
        signupError = `This server makes accounts on its own page: ${signupPage}`;
      } else {
        signupError =
          words ||
          (r.status === 429
            ? 'Too many tries: wait a minute, then try again.'
            : r.status < 500
              ? `The server didn’t take that (error ${r.status}).`
              : 'Signing up isn’t working just now: try again in a few minutes.');
      }
    } catch {
      signupError = 'Couldn’t reach the server. Check the connection and try again.';
    } finally {
      sending = false;
    }
  }

  function go(e: SubmitEvent) {
    e.preventDefault();
    if (step === 'passkey') {
      void sync.signInWithPasskey();
      return;
    }
    if (step === 'password') {
      if (email.trim() && password) sync.signIn(email, password);
      return;
    }
    if (!code.trim()) return;
    if (step === 'words') sync.recover(code);
    else sync.enter(code);
  }
</script>

<main>
  <div class="card">
    {#if backTo}
      {@const back = backTo}
      <button type="button" class="back" disabled={sync.joining} onclick={() => to(back)}><Icon name="back" size={18} /> Back</button>
    {/if}
    <img src="/icon.svg" alt="" width="56" height="56" />
    <h1>Your notes, here</h1>

    {#if step === 'choose'}
      <p>Choose sign-in method</p>
      <div class="choices">
        {#each choices as c (c.way)}
          <button
            type="button"
            class="choice"
            disabled={sync.joining}
            aria-labelledby="way-{c.way}"
            aria-describedby="way-{c.way}-line"
            use:focusedIf={c.way === left}
            onclick={() => to(c.way)}
          >
            <span class="badge"><Icon name={c.icon} size={22} /></span>
            <span class="words">
              <span class="title" id="way-{c.way}">{c.title}</span>
              <span class="line" id="way-{c.way}-line">{c.line}</span>
            </span>
            <span class="go"><Icon name="chevron-right" size={18} /></span>
          </button>
        {/each}
      </div>
      {#if !outdated && sync.error}<p class="error" role="alert">{sync.error}</p>{/if}
      <button type="button" class="link quiet" disabled={sync.joining} onclick={() => to('words')}>Use recovery words</button>
    {:else if step === 'signup'}
      <h2>Make an account</h2>
      <form class="signup" onsubmit={sendInvite}>
        <p>Your email: we’ll send an invite there, with a code to type here.</p>
        <input bind:value={signupAddress} use:focused type="email" placeholder="you@example.com" autocomplete="email" aria-label="Email" oninput={() => (signupError = '')} />
        <button class="main" disabled={sending || !signupAddress.trim()}>{sending ? 'Sending…' : 'Send me an invite'}</button>
        {#if signupError}<p class="error" role="alert">{signupError}</p>{/if}
      </form>
    {:else if step}
      {#if step === 'words'}
        <h2>Recovery words</h2>
        <p>Type the twelve recovery words made on one of your devices, for when none is left.</p>
      {:else if step === 'password'}
        <h2>Email and password</h2>
        <p>{signupEmail ? 'The email you signed up with, and the password set on another of your devices.' : 'The email and password set on another of your devices.'}</p>
      {:else if step === 'passkey'}
        <h2 tabindex="-1" use:focused>Use a passkey</h2>
        <p>A passkey added for Carnet on another of your devices, or in this browser before. The browser asks which, and for your face, fingerprint or PIN.</p>
      {:else}
        <h2>Enter the code</h2>
        {#if sentTo && lately}
          <!-- Asked again too soon: nothing new went. -->
          <p class="sent" role="status"><Icon name="clock" size={16} /> {latelyLine(sentTo, lately)}</p>
        {:else if sentTo}
          <p class="sent" role="status"><Icon name="check" size={16} /> We sent an invite to <b>{sentTo}</b>. Paste its code here.</p>
          {#if sentSaid}<p class="said">{sentSaid}</p>{/if}
        {:else if fromLink && code === fromLink}
          <p class="from" role="status"><Icon name="link" size={16} /> From a link. Connect only if you made it, or it’s your own invite.</p>
        {:else}
          <p>On a device already in, open <b>Settings → Sync → Add a device</b> and choose <b>Show a code</b>. Scan it, or type it here. An invite from your email goes here too.</p>
        {/if}
      {/if}
      {#if safari && step !== 'words'}
        <p class="tip">On an iPhone, add Carnet to the Home Screen first (<b>Share → Add to Home Screen</b>), then pair it there: Safari and the Home Screen keep separate copies.</p>
      {/if}
      <form onsubmit={go}>
        {#if step === 'words'}
          <textarea bind:value={code} use:focused rows="3" placeholder="twelve words, spaces between" aria-label="Recovery words" autocomplete="off" autocapitalize="off" spellcheck="false"></textarea>
        {:else if step === 'password'}
          <input bind:value={email} use:focused type="email" placeholder="you@example.com" autocomplete="username" aria-label="Email" />
          <!-- An eye inside its right end shows the letters, or hides them. -->
          <span class="pw">
            <input bind:value={password} type={showPassword ? 'text' : 'password'} placeholder="Password" autocomplete="current-password" aria-label="Password" />
            <button type="button" class="eye" onclick={() => (showPassword = !showPassword)} aria-label={showPassword ? 'Hide password' : 'Show password'} title={showPassword ? 'Hide password' : 'Show password'}>
              <Icon name={showPassword ? 'eye-off' : 'eye'} size={20} />
            </button>
          </span>
        {:else if step === 'passkey'}
          <!-- Nothing to type: the browser asks. -->
        {:else}
          <input bind:value={code} use:focused placeholder={sentTo ? 'K7QF-2M9D-XW4H-P3TA' : '4721-381095'} aria-label="Code" autocomplete="one-time-code" autocapitalize="characters" spellcheck="false" />
        {/if}
        <button class="main" disabled={sync.joining || (step === 'password' ? !email.trim() || !password : step !== 'passkey' && !code.trim())}>
          {#if step === 'passkey'}<Icon name="passkey" size={18} />{/if}
          {sync.joining ? 'Connecting…' : step === 'words' ? 'Recover' : step === 'password' ? 'Sign in' : step === 'passkey' ? 'Sign in with a passkey' : 'Connect'}
        </button>
        {#if step === 'code' && camera}
          <button type="button" class="second" disabled={sync.joining} onclick={() => { sync.error = null; scanning = true; }}><Icon name="scan" size={18} /> Scan the code</button>
        {/if}
      </form>
      {#if !outdated && sync.error}<p class="error" role="alert">{sync.error}</p>{/if}
      {#if step === 'code' && sentTo && signups}
        <button type="button" class="link" disabled={sync.joining} onclick={() => to('signup')}>No invite yet? Ask again</button>
      {/if}
      {#if step === 'code' && !stepped}
        <button type="button" class="link quiet" disabled={sync.joining} onclick={() => to('words')}>Use recovery words</button>
      {/if}
    {/if}
    {#if askLink}
      {@const l = askLink}
      <div class="ask" role="status">
        <p><Icon name="link" size={16} /> A link brought a code. Use the code from the link?</p>
        <div class="buttons">
          <button type="button" class="second" onclick={() => (askLink = null)}>Cancel</button>
          <button type="button" class="second" disabled={sync.joining} onclick={() => useLink(l)}>Yes, use it</button>
        </div>
      </div>
    {/if}
    {#if outdated}<p class="error" role="alert">{outdated}</p>{/if}
    {#if scanning}<Scan found={scanned} close={() => (scanning = false)} />{/if}
    <p class="note">End-to-end encrypted: the server keeps your notes sealed, and never sees a word of them. This browser keeps its keys sealed too.</p>
  </div>
</main>

<style>
  main { min-height: 100%; display: grid; place-items: center; padding: 24px 16px; }
  .card { position: relative; width: 100%; max-width: 420px; text-align: center; }
  h1 { font: 600 28px/1.2 var(--serif); margin: 14px 0 8px; }
  h2 { font: 600 18px/1.3 var(--sans); margin: 18px 0 6px; }
  h2:focus { outline: none; }
  p { color: var(--muted); margin: 0 0 18px; }
  form { display: flex; flex-direction: column; gap: 10px; }
  input, textarea {
    font: 500 20px/1.3 var(--sans); text-align: center; padding: 12px 14px; border-radius: var(--radius);
    border: 1px solid var(--border-strong); background: var(--card); color: var(--ink); width: 100%;
  }
  textarea { font-size: 16px; text-align: left; resize: none; }
  .pw { position: relative; display: flex; }
  /* Room both sides, so the letters stay centred clear of the eye. */
  .pw input { padding-left: 48px; padding-right: 48px; }
  .eye {
    position: absolute; right: 6px; top: 50%; transform: translateY(-50%); display: inline-flex; padding: 8px; border: 0;
    border-radius: var(--radius); background: none; color: var(--muted); cursor: pointer;
  }
  .eye:hover { color: var(--ink); }
  .eye:focus-visible { outline: 2px solid var(--accent-ink); outline-offset: 0; }
  button.main {
    display: inline-flex; align-items: center; justify-content: center; gap: 8px;
    padding: 12px; border: 0; border-radius: var(--radius); background: var(--primary); color: var(--on-accent);
    font-weight: 600; cursor: pointer;
  }
  button.main:disabled, button.second:disabled { opacity: 0.45; cursor: default; }
  button.second {
    display: inline-flex; align-items: center; justify-content: center; gap: 8px; padding: 11px; border: 0;
    border-radius: var(--radius); background: var(--sunken); color: var(--ink); font-weight: 600; cursor: pointer;
  }
  /* Back: level with the icon, at the card's left, so nothing below moves. */
  .back {
    position: absolute; left: -6px; top: 13px; display: inline-flex; align-items: center; gap: 2px;
    padding: 6px 10px 6px 4px; border: 0; border-radius: var(--radius); background: none; color: var(--accent-ink); cursor: pointer;
  }
  .back:not(:disabled):hover { background: var(--hover); }
  .back:disabled, .link:disabled, .choice:disabled { opacity: 0.45; cursor: default; }
  .choices { display: flex; flex-direction: column; gap: 10px; }
  /* Each a step off the page: its edge a control's, a heavier foot; a
     further step under the pointer, and another pressed. */
  .choice {
    display: flex; align-items: center; gap: 14px; width: 100%; padding: 14px 12px 14px 14px;
    border: 1px solid color-mix(in srgb, var(--border-strong) 88%, var(--ink)); border-radius: var(--radius-card);
    background: var(--card); color: var(--ink); box-shadow: 0 1px 0 var(--border-strong);
    text-align: left; cursor: pointer; transition: background-color 0.12s, border-color 0.12s;
  }
  .choice:not(:disabled):hover { border-color: var(--accent); background: linear-gradient(var(--hover), var(--hover)) var(--card); }
  .choice:not(:disabled):active { background: linear-gradient(var(--pressed), var(--pressed)) var(--card); box-shadow: none; }
  .badge {
    display: inline-flex; align-items: center; justify-content: center; flex: none; width: 40px; height: 40px;
    border-radius: 50%; background: var(--accent-soft); color: var(--accent-ink);
  }
  .words { display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 0; }
  .title { font-weight: 600; }
  .line { font-size: 14px; line-height: 1.35; color: var(--muted); }
  .go { display: inline-flex; flex: none; color: var(--faint); }
  .tip { font-size: 14px; background: var(--accent-soft); color: var(--ink); padding: 10px 14px; border-radius: var(--radius); text-align: left; }
  .link { display: block; background: none; border: 0; color: var(--accent-ink); cursor: pointer; margin: 14px auto; padding: 6px; text-decoration: none; font: inherit; }
  .link.quiet { color: var(--muted); font-size: 15px; margin-top: 16px; }
  .link.quiet:not(:disabled):hover { color: var(--accent-ink); }
  .error { color: var(--danger); margin: 12px 0 0; }
  .sent { color: var(--ink); }
  .from { color: var(--ink); }
  .ask { margin-top: 12px; }
  .ask p { color: var(--ink); margin: 0 0 10px; }
  .ask :global(.icon) { vertical-align: -3px; margin-right: 2px; color: var(--muted); }
  .ask .buttons { display: flex; gap: 10px; justify-content: center; }
  .from :global(.icon) { vertical-align: -3px; margin-right: 2px; color: var(--muted); }
  .sent :global(.icon) { vertical-align: -3px; margin-right: 2px; color: var(--success); }
  .said { font-size: 14px; }
  .signup p { margin: 0 0 8px; }
  .signup .error { margin: 2px 0 0; }
  .note { font-size: 13px; color: var(--faint); margin-top: 18px; }
</style>
