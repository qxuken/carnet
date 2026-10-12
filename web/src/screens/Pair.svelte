<script lang="ts">
  // Coming in: a code from a device already in (typed, scanned, or in the
  // link a QR code opened), an invite for a new account (from a signup's
  // email), a passkey, an email and password, or the twelve words. The
  // kind of code is read from its shape.
  import { onMount, tick } from 'svelte';
  import { sync } from '../lib/sync.svelte';
  import Icon from '../lib/Icon.svelte';
  import Scan from './Scan.svelte';
  import { code_from_link, server_fit } from '../wasm/carnet_web.js';

  /// Which way in: a code (an invite, or one from a device), the words,
  /// an email and password, or a passkey.
  let way = $state<'code' | 'words' | 'password' | 'passkey'>('code');
  const words = $derived(way === 'words');
  let code = $state('');
  let email = $state('');
  let password = $state('');
  let showPassword = $state(false);
  let scanning = $state(false);
  const camera = !!navigator.mediaDevices?.getUserMedia;
  const installed = matchMedia('(display-mode: standalone)').matches || (navigator as { standalone?: boolean }).standalone === true;
  // On an iPhone, Safari and the Home Screen keep apart what a site
  // stores: paired in Safari, the Home Screen's Carnet would start empty.
  const safari = /iPhone|iPad/.test(navigator.userAgent) && !installed;

  /// Where someone new makes an account, if this server takes signups.
  let signup = $state<string | null>(null);
  /// New here: the email field shown, what's typed in it, an invite
  /// being asked for, the email it was sent to, and what the server said.
  let newHere = $state(false);
  let signupAddress = $state('');
  let sending = $state(false);
  let sentTo = $state<string | null>(null);
  let sentSaid = $state('');
  /// Asked again within the server's mail limit: nothing went. When the
  /// last one did, and when another may be asked for, in seconds.
  let lately = $state<{ ago: number; again: number } | null>(null);
  let signupError = $state('');
  let codeField = $state<HTMLInputElement | null>(null);
  /// Whether this server takes passwords and passkeys; offered unless it
  /// says not.
  let passwords = $state(true);
  let passkeys = $state(true);
  /// A password here signs in with the email the account signed up with
  /// (a hosted service holds them to it).
  let signupEmail = $state(false);
  /// Why this app and the server can't sync, if they can't (`compat`).
  let outdated = $state('');
  type Info = { signup?: string; passwords?: boolean; password_email?: string; protocol?: number; oldest?: number; deprecated?: number; features?: string[] };
  onMount(() => {
    fetch('/v1/info')
      .then((r) => (r.ok ? r.json() : null))
      .then((i: Info | null) => {
        signup = i?.signup ?? null;
        passwords = i?.passwords ?? true;
        signupEmail = i?.password_email === 'account';
        if (i?.features) passkeys = i.features.includes('passkeys');
        if (i) outdated = server_fit(i.protocol ?? 0, i.oldest ?? 0, i.deprecated ?? 0);
      })
      .catch(() => {});
  });

  onMount(() => {
    if (!sync.linked) return;
    code = sync.linked;
    if (!safari) sync.enter(code);
  });

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

  /// The other ways in, as links: a passkey only where this browser can.
  const ways = $derived(
    (
      [
        ['code', 'Use a code'],
        ['passkey', 'Use a passkey'],
        ['password', 'Use email and password'],
        ['words', 'Use recovery words'],
      ] as const
    ).filter(([w]) => w !== way && (w !== 'passkey' || (passkeys && sync.passkeySupport !== 'no')) && (w !== 'password' || passwords)),
  );

  /// Another way chosen: a passkey's begins at once, the browser asking
  /// which.
  function choose(w: typeof way) {
    way = w;
    code = '';
    sync.error = null;
    if (w === 'passkey') void sync.signInWithPasskey();
  }

  /// The email field, focused as it opens.
  const focused = (el: HTMLInputElement) => el.focus();

  /// The words in a server's answer: its JSON's `error` or `message`, or
  /// its text if short and not a page.
  function said(body: string): string {
    try {
      const j = JSON.parse(body) as { error?: unknown; message?: unknown };
      const s = typeof j?.error === 'string' ? j.error : typeof j?.message === 'string' ? j.message : '';
      return s.trim();
    } catch {
      const t = body.trim();
      return t.length <= 300 && !t.startsWith('<') ? t : '';
    }
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
  /// the code field, focused.
  async function sendInvite(e: SubmitEvent) {
    e.preventDefault();
    const to = signupAddress.trim();
    if (!to || sending) return;
    if (!/^[^\s@]+@[^\s@]+$/.test(to)) {
      signupError = 'That doesn’t look like an email address.';
      return;
    }
    sending = true;
    signupError = '';
    try {
      const r = await fetch('/v1/signup', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email: to }) });
      const body = await r.text().catch(() => '');
      const words = said(body);
      if (r.ok) {
        sentTo = to;
        sentSaid = r.status === 204 ? '' : words;
        lately = mailedLately(body);
        if (lately) sentSaid = '';
        newHere = false;
        way = 'code';
        code = '';
        sync.error = null;
        await tick();
        codeField?.focus();
      } else if ((r.status === 404 || r.status === 405) && signup) {
        signupError = `This server makes accounts on its own page: ${signup}`;
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
    if (way === 'passkey') {
      void sync.signInWithPasskey();
      return;
    }
    if (way === 'password') {
      if (email.trim() && password) sync.signIn(email, password);
      return;
    }
    if (!code.trim()) return;
    if (words) sync.recover(code);
    else sync.enter(code);
  }
</script>

<main>
  <div class="card">
    <img src="/icon.svg" alt="" width="56" height="56" />
    <h1>{newHere ? 'Make an account' : 'Your notes, here'}</h1>
    {#if newHere}
      <!-- New here: only the asking, nothing about devices or other ways
           in; the invite's code then goes in the code field. -->
      <form class="signup" onsubmit={sendInvite}>
        <p>Your email: the server sends an invite there, with a code to type here.</p>
        <input bind:value={signupAddress} use:focused type="email" placeholder="you@example.com" autocomplete="email" aria-label="Email" oninput={() => (signupError = '')} />
        <button class="main" disabled={sending || !signupAddress.trim()}>{sending ? 'Sending…' : 'Send me an invite'}</button>
        {#if signupError}<p class="error" role="alert">{signupError}</p>{/if}
      </form>
      <button type="button" class="link" onclick={() => { newHere = false; signupError = ''; }}>I already have an account</button>
    {:else}
    {#if words}
      <p>Type the twelve recovery words made on one of your devices, for when none is left.</p>
    {:else if way === 'password'}
      <p>{signupEmail ? 'The email you signed up with, and the password set on another of your devices.' : 'The email and password set on another of your devices.'}</p>
    {:else if way === 'passkey'}
      <p>A passkey added for Carnet on another of your devices, or in this browser before. The browser asks which, and for your face, fingerprint or PIN.</p>
    {:else if sentTo && !newHere && lately}
      <!-- Asked again too soon: nothing new went. -->
      <p class="sent" role="status"><Icon name="clock" size={16} /> {latelyLine(sentTo, lately)}</p>
    {:else if sentTo && !newHere}
      <p class="sent" role="status"><Icon name="check" size={16} /> We sent an invite to <b>{sentTo}</b>. Paste its code here.</p>
      {#if sentSaid}<p class="said">{sentSaid}</p>{/if}
    {:else}
      <p>On a device already in, open <b>Settings → Sync → Add a device</b> and choose <b>Show a code</b>. Scan it, or type it here. An invite from your email goes here too.</p>
    {/if}
    {#if safari && !words}
      <p class="tip">On an iPhone, add Carnet to the Home Screen first (<b>Share → Add to Home Screen</b>), then pair it there: Safari and the Home Screen keep separate copies.</p>
    {/if}
    <form onsubmit={go}>
      {#if words}
        <textarea bind:value={code} rows="3" placeholder="twelve words, spaces between" autocomplete="off" autocapitalize="off" spellcheck="false"></textarea>
      {:else if way === 'password'}
        <input bind:value={email} type="email" placeholder="you@example.com" autocomplete="username" aria-label="Email" />
        <!-- An eye inside its right end shows the letters, or hides them. -->
        <span class="pw">
          <input bind:value={password} type={showPassword ? 'text' : 'password'} placeholder="Password" autocomplete="current-password" aria-label="Password" />
          <button type="button" class="eye" onclick={() => (showPassword = !showPassword)} aria-label={showPassword ? 'Hide password' : 'Show password'} title={showPassword ? 'Hide password' : 'Show password'}>
            <Icon name={showPassword ? 'eye-off' : 'eye'} size={20} />
          </button>
        </span>
      {:else if way === 'passkey'}
        <!-- Nothing to type: the browser asks. -->
      {:else}
        <input bind:this={codeField} bind:value={code} placeholder={sentTo ? 'K7QF-2M9D-XW4H-P3TA' : '47-381095'} aria-label="Code" autocomplete="one-time-code" autocapitalize="characters" spellcheck="false" />
      {/if}
      <button class="main" disabled={sync.joining || (way === 'password' ? !email.trim() || !password : way !== 'passkey' && !code.trim())}>
        {#if way === 'passkey'}<Icon name="passkey" size={18} />{/if}
        {sync.joining ? 'Connecting…' : words ? 'Recover' : way === 'password' ? 'Sign in' : way === 'passkey' ? 'Sign in with a passkey' : 'Connect'}
      </button>
      {#if way === 'code' && camera}
        <button type="button" class="second" disabled={sync.joining} onclick={() => { sync.error = null; scanning = true; }}><Icon name="scan" size={18} /> Scan the code</button>
      {/if}
    </form>
    {#if outdated}<p class="error" role="alert">{outdated}</p>{:else if sync.error}<p class="error" role="alert">{sync.error}</p>{/if}
    {#if signup && way === 'code'}
      <button class="link" disabled={sync.joining} onclick={() => { newHere = true; signupError = ''; }}>{sentTo ? 'No invite yet? Ask again' : 'New here? Make an account'}</button>
    {/if}
    {#each ways as [w, label] (w)}
      <button class="link" disabled={sync.joining} onclick={() => choose(w)}>{label}</button>
    {/each}
    {/if}
    {#if scanning}<Scan found={scanned} close={() => (scanning = false)} />{/if}
    <p class="note">End-to-end encrypted: the server keeps your notes sealed, and never sees a word of them. This browser keeps its keys sealed too.</p>
  </div>
</main>

<style>
  main { min-height: 100%; display: grid; place-items: center; padding: 24px 16px; }
  .card { width: 100%; max-width: 420px; text-align: center; }
  h1 { font: 600 28px/1.2 var(--serif); margin: 14px 0 8px; }
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
  .tip { font-size: 14px; background: var(--accent-soft); color: var(--ink); padding: 10px 14px; border-radius: var(--radius); text-align: left; }
  .link { display: block; background: none; border: 0; color: var(--accent-ink); cursor: pointer; margin: 14px auto; padding: 6px; text-decoration: none; font: inherit; }
  .error { color: var(--danger); margin: 12px 0 0; }
  .sent { color: var(--ink); }
  .sent :global(.icon) { vertical-align: -3px; margin-right: 2px; color: var(--success); }
  .said { font-size: 14px; }
  .signup p { margin: 0 0 8px; }
  .signup .error { margin: 2px 0 0; }
  .note { font-size: 13px; color: var(--faint); margin-top: 18px; }
</style>
