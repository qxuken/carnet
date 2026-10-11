<script lang="ts">
  // Coming in: a code from a device already in (typed, scanned, or in the
  // link a QR code opened), an invite for a new account (from a signup's
  // email), a passkey, an email and password, or the twelve words. The
  // kind of code is read from its shape.
  import { onMount } from 'svelte';
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
  let scanning = $state(false);
  const camera = !!navigator.mediaDevices?.getUserMedia;
  const installed = matchMedia('(display-mode: standalone)').matches || (navigator as { standalone?: boolean }).standalone === true;
  // On an iPhone, Safari and the Home Screen keep apart what a site
  // stores: paired in Safari, the Home Screen's Carnet would start empty.
  const safari = /iPhone|iPad/.test(navigator.userAgent) && !installed;

  /// Where someone new makes an account, if this server takes signups.
  let signup = $state<string | null>(null);
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
    <h1>Your notes, here</h1>
    {#if words}
      <p>Type the twelve recovery words made on one of your devices, for when none is left.</p>
    {:else if way === 'password'}
      <p>{signupEmail ? 'The email you signed up with, and the password set on another of your devices.' : 'The email and password set on another of your devices.'}</p>
    {:else if way === 'passkey'}
      <p>A passkey added for Carnet on another of your devices, or in this browser before. The browser asks which, and for your face, fingerprint or PIN.</p>
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
        <input bind:value={password} type="password" placeholder="Password" autocomplete="current-password" aria-label="Password" />
      {:else if way === 'passkey'}
        <!-- Nothing to type: the browser asks. -->
      {:else}
        <input bind:value={code} placeholder="47-381095" autocomplete="one-time-code" autocapitalize="characters" spellcheck="false" />
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
      <a class="link" href={signup}>New here? Make an account</a>
    {/if}
    {#each ways as [w, label] (w)}
      <button class="link" disabled={sync.joining} onclick={() => choose(w)}>{label}</button>
    {/each}
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
  .note { font-size: 13px; color: var(--faint); margin-top: 18px; }
</style>
