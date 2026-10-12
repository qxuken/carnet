<script lang="ts">
  // This browser and the account's devices, as the desktop's Settings →
  // Sync has them: who's in, letting one more in, the recovery words, the
  // passkeys, and leaving. On the server owner's account, invites and the
  // accounts too.
  import { onDestroy, onMount } from 'svelte';
  import { sync } from '../lib/sync.svelte';
  import { ago, left } from '../lib/time';
  import Icon from '../lib/Icon.svelte';
  import { pair_link, qr_svg } from '../wasm/carnet_web.js';

  let { back }: { back: (() => void) | null } = $props();
  /// The device whose removal is asked to be confirmed.
  let removing = $state<string | null>(null);
  /// The account whose removal is asked to be confirmed.
  let removingAccount = $state<string | null>(null);
  /// The passkey whose removal is asked to be confirmed.
  let removingPasskey = $state<string | null>(null);
  /// Making a recovery code asked once, where it would replace one.
  let replacingWords = $state(false);
  /// The most passkeys an account has (`passkey::MAX`).
  const MAX_PASSKEYS = 20;
  /// This browser's name, as typed while it's being changed.
  let naming = $state<string | null>(null);

  /// The name selected as the field opens: typing replaces it.
  const selected = (input: HTMLInputElement) => input.select();

  function saveName() {
    if (naming === null) return;
    void sync.rename(naming);
    naming = null;
  }
  let forgetting = $state(false);
  const online = $derived(sync.status === 'Up to date');
  const installed = matchMedia('(display-mode: standalone)').matches || (navigator as { standalone?: boolean }).standalone === true;
  const ios = /iPhone|iPad/.test(navigator.userAgent);
  const qr = $derived.by(() => {
    if (!sync.code) return '';
    const link = pair_link(location.origin, sync.code);
    return link ? (qr_svg(link) ?? '') : '';
  });

  onMount(() => {
    sync.listDevices();
    // Used elsewhere since: asked afresh each time.
    sync.invites = null;
    sync.listInvites();
  });
  onDestroy(() => {
    sync.doneWithDevices();
    if (sync.code) sync.pairCancel();
    sync.wordsDone();
    sync.inviteDone();
  });

  function account(a: { devices: number; seen: number }) {
    const devices = a.devices === 1 ? '1 device' : `${a.devices} devices`;
    return `${devices}, ${a.seen ? `last seen ${ago(a.seen)}` : 'not seen yet'}`;
  }

  /// Bytes as people say them: "340 KB", "1.2 GB".
  function size(n: number) {
    if (n < 1000) return `${n} bytes`;
    const units = ['KB', 'MB', 'GB', 'TB'];
    let v = n / 1000;
    let u = 0;
    while (v >= 1000 && u < units.length - 1) {
      v /= 1000;
      u++;
    }
    return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[u]}`;
  }

  const space = $derived.by(() => {
    const u = sync.usage;
    if (!u) return null;
    const kept = size(u.notes + u.pictures);
    let line = u.limit === null ? `${kept} kept on the server` : `${kept} of ${size(u.limit)} used`;
    if (u.plan) {
      line += ` · ${u.plan}`;
      if (u.until) line += `, until ${new Date(u.until * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC' })}`;
    }
    const used = u.notes + u.pictures;
    const over = u.limit === null ? '' : used > u.limit ? `Over this plan’s ${size(u.limit)}` : used === u.limit ? `Reached this plan’s ${size(u.limit)}` : '';
    return { line, writable: u.writable, over };
  });

  /// The password being set: its fields; asked to take it away.
  let pw = $state<{ email: string; password: string } | null>(null);
  /// Its letters shown, by the eye in the field; hidden again for the next.
  let showPw = $state(false);
  $effect(() => {
    if (pw === null) showPw = false;
  });
  let saving = $state(false);
  let dropping = $state(false);
  let deletingAccount = $state(false);
  // Set: the fields go, and the list says the email.
  let sets = sync.passwordSets;
  $effect(() => {
    if (sync.passwordSets !== sets) {
      sets = sync.passwordSets;
      pw = null;
      saving = false;
    }
  });
  // Refused (an email taken, say): try again.
  $effect(() => {
    if (sync.error) saving = false;
  });

  function seen(d: { you: boolean; seen: number; online?: boolean }) {
    if (d.you) return 'This browser';
    // Logged in now: not the time its session began. (Offline here, the
    // list is an old one: it can't say who's online now.)
    if (d.online && online) return 'Online now';
    if (!d.seen) return 'Not seen since it came in';
    return `Last seen ${ago(d.seen)}`;
  }
</script>

<article>
  <header>
    {#if back}
      <button class="back" onclick={back} aria-label="All notes"><Icon name="back" size={22} /></button>
    {/if}
    <h1>Settings</h1>
  </header>

  <div class="body">
    {#if sync.updateSoon}
      <p class="row danger"><Icon name="warning" size={16} /> {sync.updateSoon}</p>
    {/if}
    {#if space}
      <p class="row quiet"><Icon name="cloud" size={16} /> {space.line}</p>
    {/if}
    <!-- Read only: said here, and only here (not again as an error). -->
    {#if sync.readOnly || (space && !space.writable)}
      <p class="row danger"><Icon name="warning" size={16} /> Read only for now: what’s written here stays here, and syncs once the account can write again.</p>
    {:else if space?.over}
      <!-- The server takes no new picture at or over the limit; notes'
           text it always takes. -->
      <p class="row warn"><Icon name="warning" size={16} /> {space.over}: new pictures stay on this device until there’s room. Notes still sync.</p>
    {/if}
    <h2>Devices</h2>
    <section>
      {#if sync.devices === null}
        <p class="row quiet"><Icon name="clock" size={16} /> {online ? 'Listing this account’s devices…' : 'The list comes once this browser is online.'}</p>
      {:else}
        {#each sync.devices as d (d.id)}
          {#if d.you && naming !== null}
            <form class="row naming" onsubmit={(e) => { e.preventDefault(); saveName(); }}>
              <Icon name="device" size={20} />
              <!-- svelte-ignore a11y_autofocus -->
              <input bind:value={naming} use:selected maxlength="64" aria-label="What this browser is called" placeholder="What this browser is called" autofocus />
              <button type="button" class="pill" onclick={() => (naming = null)}>Cancel</button>
              <button class="pill primary" disabled={!online}>Save</button>
            </form>
          {:else}
          <div class="row">
            <Icon name="device" size={20} />
            <div class="text">
              <b>{d.name ?? 'A device not yet named'}</b>
              <!-- Its id, short: two devices of one name told apart. -->
              <span>{seen(d)} · <span class="id">{d.id.slice(0, 6)}</span></span>
            </div>
            {#if d.you}
              <button class="pill" disabled={!online} onclick={() => (naming = d.name ?? '')}>Rename</button>
            {:else if removing !== d.id}
              <button class="pill" disabled={!online} onclick={() => (removing = d.id)}>Remove</button>
            {/if}
          </div>
          {/if}
          {#if removing === d.id}
            <div class="confirm">
              <p>Removed, it can’t sync, nor read anything written from now on. Its own copy of the notes stays on it.</p>
              <div class="buttons">
                <button class="pill" onclick={() => (removing = null)}>Cancel</button>
                <button class="pill danger" disabled={!online} onclick={() => { sync.removeDevice(d.id); removing = null; }}>Yes, remove</button>
              </div>
            </div>
          {/if}
        {/each}
      {/if}

      <div class="row rule">
        <Icon name="scan" size={20} />
        <div class="text">
          <b>Add a device</b>
          <span>Show a code here, then scan it with the new device’s camera, or type it there. It works once.</span>
        </div>
        {#if sync.code}
          <button class="pill" onclick={() => sync.pairCancel()}>Cancel</button>
        {:else}
          <button class="pill" disabled={!online} onclick={() => sync.pairOpen()}>Show a code</button>
        {/if}
      </div>
      {#if sync.code}
        <div class="code">
          <div class="qr" role="img" aria-label="The code as a QR code">{@html qr}</div>
          <p class="digits" aria-label="Pairing code">{sync.code}</p>
        </div>
      {:else if sync.paired && sync.devices?.some((d) => d.id === sync.paired)}
        <p class="row ok" role="status"><Icon name="check" size={16} /> A new device is in: it’s in the list above.</p>
      {/if}

      <div class="row rule">
        <Icon name="key" size={20} />
        <div class="text">
          <b>Recovery code</b>
          <span>Twelve words that let a new device in when none of yours is left.</span>
        </div>
        {#if sync.words}
          <button class="pill primary" onclick={() => sync.wordsDone()}>Done</button>
        {:else if sync.wordsWaiting}
          <button class="pill" disabled>Making one…</button>
        {:else if !replacingWords}
          <button class="pill" disabled={!online} onclick={() => (sync.hasRecovery() ? (replacingWords = true) : sync.makeRecovery())}>Make one</button>
        {/if}
      </div>
      {#if replacingWords}
        <div class="confirm">
          <p>Replace the recovery code? The words you have now will stop working.</p>
          <div class="buttons">
            <button class="pill" onclick={() => (replacingWords = false)}>Cancel</button>
            <button class="pill danger" disabled={!online} onclick={() => { sync.makeRecovery(); replacingWords = false; }}>Yes, replace</button>
          </div>
        </div>
      {/if}
      {#if sync.words}
        <div class="words">
          <ol aria-label="Recovery words">
            {#each sync.words.split(' ') as w, i (i)}
              <li><span>{i + 1}</span>{w}</li>
            {/each}
          </ol>
          <p>Write them down and keep them somewhere safe: they are shown this once, and kept nowhere on this browser. A new code replaces this one.</p>
        </div>
      {/if}
    </section>

    {#if sync.passkeys !== null && sync.features.includes('passkeys')}
      <h2>Passkeys</h2>
      <section>
        {#each sync.passkeys as p, i (p.id)}
          <div class="row" class:rule={i > 0}>
            <Icon name="passkey" size={20} />
            <div class="text">
              <b>{p.name ?? 'A passkey'}</b>
              <span>{p.made ? `Added ${ago(p.made)}` : 'Signs a browser in to this account'}</span>
            </div>
            {#if removingPasskey !== p.id}
              <button class="pill" disabled={!online} onclick={() => (removingPasskey = p.id)}>Remove</button>
            {/if}
          </div>
          {#if removingPasskey === p.id}
            <div class="confirm">
              <p>It can’t sign in to this account any more; devices it let in stay in. It stays in the passkey manager that keeps it until you delete it there.</p>
              <div class="buttons">
                <button class="pill" onclick={() => (removingPasskey = null)}>Cancel</button>
                <button class="pill danger" disabled={!online} onclick={() => { sync.removePasskey(p); removingPasskey = null; }}>Yes, remove</button>
              </div>
            </div>
          {/if}
        {/each}
        <div class="row" class:rule={sync.passkeys.length > 0}>
          <Icon name={sync.passkeySupport === 'no' ? 'warning' : 'plus'} size={20} />
          <div class="text">
            <b>Add a passkey</b>
            {#if sync.passkeySupport === 'no'}
              <span>This browser can’t make passkeys Carnet can use. Chrome, Edge, Safari 18 and Firefox can.</span>
            {:else if sync.passkeys.length >= MAX_PASSKEYS}
              <span>Twenty at most: remove one to add another.</span>
            {:else}
              <span>Then a new browser comes in with your face, fingerprint or phone, instead of a code.</span>
            {/if}
          </div>
          {#if sync.passkeySupport !== 'no'}
            <button class="pill" disabled={!online || sync.addingPasskey || sync.passkeys.length >= MAX_PASSKEYS} onclick={() => sync.addPasskey()}>
              {sync.addingPasskey ? 'Adding…' : 'Add'}
            </button>
          {/if}
        </div>
      </section>
    {/if}

    {#if sync.password?.allowed}
      <h2>Email and password</h2>
      <section>
        {#if pw === null}
          <div class="row">
            <Icon name="key" size={20} />
            <div class="text">
              {#if sync.password.id}
                <b>{sync.password.id}</b>
                <span>A new device, or a browser, can come in with this email and the password.</span>
              {:else}
                <b>No password</b>
                {#if sync.password.only}
                  <span>Set one, and a new device can come in with {sync.password.only} and it, instead of a code.</span>
                {:else}
                  <span>Set one, and a new device can come in with your email and it, instead of a code.</span>
                {/if}
              {/if}
            </div>
            {#if sync.password.id && !dropping}
              <button class="pill" disabled={!online} onclick={() => (dropping = true)}>Take away</button>
              <button class="pill" disabled={!online} onclick={() => (pw = { email: sync.password?.only ?? sync.password?.id ?? '', password: '' })}>Change</button>
            {:else if !sync.password.id}
              <button class="pill" disabled={!online} onclick={() => (pw = { email: sync.password?.only ?? '', password: '' })}>Set one</button>
            {/if}
          </div>
          {#if dropping}
            <div class="confirm">
              <p>Then only a code from a device, or the recovery words, let a new device in.</p>
              <div class="buttons">
                <button class="pill" onclick={() => (dropping = false)}>Cancel</button>
                <button class="pill danger" disabled={!online} onclick={() => { sync.dropPassword(); dropping = false; }}>Yes, take it away</button>
              </div>
            </div>
          {/if}
        {:else}
          <form class="row naming password" onsubmit={(e) => { e.preventDefault(); if (pw && sync.setPassword(sync.password?.only ?? pw.email, pw.password)) saving = true; }}>
            <Icon name="key" size={20} />
            {#if sync.password.only}
              <!-- The service holds passwords to the email signed up with:
                   shown, not asked (and named for the browser's keychain). -->
              <input value={sync.password.only} type="email" autocomplete="username" aria-label="Email" readonly />
            {:else}
              <!-- svelte-ignore a11y_autofocus -->
              <input bind:value={pw.email} type="email" autocomplete="username" placeholder="you@example.com" aria-label="Email" autofocus />
            {/if}
            <!-- An eye inside its right end shows the letters, or hides them. -->
            <span class="pw">
              <!-- svelte-ignore a11y_autofocus -->
              <input bind:value={pw.password} type={showPw ? 'text' : 'password'} autocomplete="new-password" placeholder="10 letters or more" aria-label="New password" autofocus={!!sync.password.only} />
              <button type="button" class="eye" onclick={() => (showPw = !showPw)} aria-label={showPw ? 'Hide password' : 'Show password'} title={showPw ? 'Hide password' : 'Show password'}>
                <Icon name={showPw ? 'eye-off' : 'eye'} size={18} />
              </button>
            </span>
            <button type="button" class="pill" onclick={() => (pw = null)}>Cancel</button>
            <button class="pill primary" disabled={!online || saving || !(sync.password?.only ?? pw.email).includes('@') || pw.password.length < 8}>{saving ? 'Saving…' : 'Save'}</button>
          </form>
          {#if sync.password.only}
            <p class="row quiet">On this service, the password signs in with the email you signed up with.</p>
          {/if}
          <p class="row quiet">The password never leaves this browser: the server can’t read it, nor reset it. Keep the recovery words too.</p>
        {/if}
      </section>
    {/if}

    {#if sync.invites?.owner}
      <h2>Invites</h2>
      <section>
        <div class="row">
          <Icon name="plus" size={20} />
          <div class="text">
            <b>Invite someone</b>
            <span>An invite makes a new account on this server, for someone else’s own notes. It works once, within a week.</span>
          </div>
          {#if sync.invite}
            <button class="pill primary" onclick={() => sync.inviteDone()}>Done</button>
          {:else}
            <button class="pill" disabled={!online} onclick={() => sync.makeInvite()}>Make an invite</button>
          {/if}
        </div>
        {#if sync.invite}
          <div class="invite">
            <p class="digits" aria-label="Invite code">{sync.invite}</p>
            <p>Give them this code and the server’s address, {location.origin}. In Carnet’s Settings → Sync they type both, and Connect.</p>
          </div>
        {/if}
        {#each sync.invites.waiting as w (w.hash)}
          <div class="row rule">
            <Icon name="clock" size={20} />
            <div class="text">
              <b class:code-text={w.code}>{w.code ?? 'An invite'}</b>
              <span>Not used yet · {left(w.until)}</span>
            </div>
            <button class="pill" disabled={!online} onclick={() => sync.dropInvite(w.hash)}>Take back</button>
          </div>
        {/each}
      </section>
      {#if sync.invites.accounts.some((a) => !a.you)}
        <h2>Other accounts on this server</h2>
        <section>
          {#each sync.invites.accounts.filter((a) => !a.you) as a, i (a.id)}
            <div class="row" class:rule={i > 0}>
              <Icon name="person" size={20} />
              <div class="text">
                <b>An account</b>
                <span>{account(a)} · <span class="id">{a.id.slice(0, 6)}</span></span>
              </div>
              {#if removingAccount !== a.id}
                <button class="pill" disabled={!online} onclick={() => (removingAccount = a.id)}>Remove</button>
              {/if}
            </div>
            {#if removingAccount === a.id}
              <div class="confirm">
                <p>Everything it keeps on this server goes: its notes there, its pictures, its devices’ way in. What’s on their devices stays.</p>
                <div class="buttons">
                  <button class="pill" onclick={() => (removingAccount = null)}>Cancel</button>
                  <button class="pill danger" disabled={!online} onclick={() => { sync.removeAccount(a.id); removingAccount = null; }}>Yes, remove</button>
                </div>
              </div>
            {/if}
          {/each}
        </section>
      {/if}
    {/if}

    <h2>This browser</h2>
    <section>
      {#if ios && !installed}
        <div class="row">
          <Icon name="warning" size={20} />
          <div class="text">
            <b>Keep your notes here offline</b>
            <span>Safari can clear what a site keeps. Use <b>Share → Add to Home Screen</b>, then pair Carnet there.</span>
          </div>
        </div>
      {/if}
      <div class="row" class:rule={ios && !installed}>
        <Icon name="trash" size={20} />
        <div class="text">
          <b>Forget this browser</b>
          <span>It leaves the account and forgets its copy of the notes. Your notes stay on your other devices.</span>
        </div>
        {#if !forgetting}
          <button class="pill" onclick={() => (forgetting = true)}>Forget</button>
        {/if}
      </div>
      {#if forgetting}
        <div class="confirm">
          <p>To come back, this browser needs a code from another device, {sync.passkeys?.length ? 'a passkey, ' : ''}or the recovery words.</p>
          <div class="buttons">
            <button class="pill" onclick={() => (forgetting = false)}>Cancel</button>
            <button class="pill danger" onclick={() => sync.forget()}>Yes, forget</button>
          </div>
        </div>
      {/if}
    </section>

    {#if !sync.invites?.owner && sync.features.includes('account-delete')}
      <h2>The account</h2>
      <section>
        <div class="row">
          <Icon name="trash" size={20} />
          <div class="text">
            <b>Delete the account</b>
            <span>Everything the server keeps of it, on every device.</span>
          </div>
          {#if !deletingAccount}
            <button class="pill" disabled={!online} onclick={() => (deletingAccount = true)}>Delete</button>
          {/if}
        </div>
        {#if deletingAccount}
          <div class="confirm">
            <p>Every note and picture the server keeps goes, for good, and every device leaves the account. What’s on your computers stays there, as files.</p>
            <div class="buttons">
              <button class="pill" onclick={() => (deletingAccount = false)}>Cancel</button>
              <button class="pill danger" disabled={!online} onclick={() => sync.deleteAccount()}>Yes, delete the account</button>
            </div>
          </div>
        {/if}
      </section>
    {/if}

    {#if sync.error}<p class="error" role="alert">{sync.error}</p>{/if}
    <p class="note">Account {sync.account}. Sync is end-to-end encrypted: the server keeps your notes sealed, and never sees a word of them. This browser keeps its keys sealed too.</p>
  </div>
</article>

<style>
  article { height: 100%; overflow-y: auto; background: var(--page); }
  header { position: sticky; top: 0; display: flex; align-items: center; gap: 8px; min-height: 52px; padding: 8px 16px; padding-top: max(8px, env(safe-area-inset-top)); background: color-mix(in srgb, var(--page) 88%, transparent); backdrop-filter: blur(10px); z-index: 1; }
  h1 { font: 600 22px/1.2 var(--serif); margin: 0; }
  .back { display: inline-flex; padding: 6px; margin-left: -6px; border: 0; background: none; color: var(--ink); border-radius: var(--radius); cursor: pointer; }
  .body { max-width: 640px; margin: 0 auto; padding: 8px 16px calc(40px + env(safe-area-inset-bottom)); }
  h2 { font-size: 13px; font-weight: 600; color: var(--muted); margin: 18px 4px 8px; }
  section { background: var(--card); border-radius: var(--radius-card); box-shadow: 0 1px 3px #0000000d; overflow: hidden; }
  .row { display: flex; align-items: center; gap: 12px; padding: 12px 16px; margin: 0; color: var(--muted); }
  .row.rule { border-top: 1px solid var(--border); }
  .row.quiet, .row.ok { font-size: 14px; gap: 8px; }
  .row.danger { font-size: 14px; color: var(--danger); }
  .row.warn { font-size: 14px; gap: 8px; color: var(--warning); }
  .row.ok { color: var(--success); }
  .text { display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 0; }
  .text b { color: var(--ink); font-weight: 600; }
  .id { color: var(--faint); font-variant-numeric: tabular-nums; }
  .text span { font-size: 13.5px; }
  .pill { flex: none; padding: 7px 14px; border: 0; border-radius: 999px; background: var(--sunken); color: var(--ink); font-weight: 500; cursor: pointer; }
  .pill:disabled { opacity: 0.45; cursor: default; }
  .pill.primary { background: var(--primary); color: var(--on-accent); }
  .pill.danger { background: var(--danger); color: var(--card); }
  .confirm { padding: 0 16px 14px 48px; color: var(--muted); font-size: 13.5px; }
  .confirm p { margin: 0 0 10px; }
  .buttons { display: flex; gap: 8px; justify-content: flex-end; }
  .code { display: flex; align-items: center; gap: 20px; padding: 4px 16px 18px 48px; flex-wrap: wrap; }
  .qr { width: 168px; height: 168px; padding: 0; color: #1f1b18; background: #fff; border-radius: var(--radius); flex: none; }
  .qr :global(svg) { width: 100%; height: 100%; display: block; }
  .digits { font: 600 34px/1.1 var(--serif); color: var(--ink); margin: 0; letter-spacing: 0.02em; user-select: all; }
  .words { padding: 0 16px 16px 48px; }
  .invite { padding: 0 16px 16px 48px; }
  .naming input { flex: 1; min-width: 0; padding: 7px 12px; border: 0; border-radius: var(--radius); background: var(--sunken); color: var(--ink); font: inherit; font-size: 15px; outline: none; }
  .naming input:focus { box-shadow: 0 0 0 2px var(--accent-soft); }
  .naming.password { flex-wrap: wrap; }
  .naming.password input { flex: 1 1 180px; }
  .naming.password input[readonly] { color: var(--muted); }
  .naming.password .pw { flex: 1 1 180px; min-width: 0; display: flex; position: relative; }
  .naming.password .pw input { padding-right: 40px; }
  .eye {
    position: absolute; right: 4px; top: 50%; transform: translateY(-50%); display: inline-flex; padding: 5px; border: 0;
    border-radius: 6px; background: none; color: var(--muted); cursor: pointer;
  }
  .eye:hover { color: var(--ink); }
  .eye:focus-visible { outline: 2px solid var(--accent-ink); outline-offset: 0; }
  .invite p { margin: 0; font-size: 13px; color: var(--muted); }
  .invite .digits { margin-bottom: 8px; font-size: 26px; }
  .code-text { font-variant-numeric: tabular-nums; letter-spacing: 0.02em; }
  .words ol { list-style: none; margin: 0 0 10px; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(120px, 1fr)); gap: 6px 12px; user-select: text; }
  .words li { font: 400 17px/1.4 var(--serif); color: var(--ink); display: flex; gap: 8px; align-items: baseline; }
  .words li span { width: 18px; text-align: right; font: 400 12px var(--sans); color: var(--muted); }
  .words p { margin: 0; font-size: 13px; color: var(--muted); }
  .error { color: var(--danger); font-size: 14px; margin: 14px 4px 0; }
  .note { color: var(--faint); font-size: 13px; margin: 18px 4px 0; }
  @media (max-width: 480px) {
    .confirm, .code, .words, .invite { padding-left: 16px; }
  }
</style>
