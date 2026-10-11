// Passkeys (docs/SYNC.md §7): made and used here, by WebAuthn with its PRF
// extension. What the PRF gives, the same each time for the same salt, is
// the passkey's secret: only this browser and the passkey see it, and the
// wasm makes the slot's keys from it. No server checks a WebAuthn
// signature; what proves the passkey is the recovery signed with that
// secret.

import { passkey_salt } from '../wasm/carnet_web.js';

/// Whether this browser can make and use passkeys that give a PRF: `maybe`
/// when it can't say (no getClientCapabilities): offered, and said plainly
/// if it then doesn't.
export type Support = 'yes' | 'no' | 'maybe';

/// Why a passkey didn't work, in words for the page.
export class PasskeyError extends Error {
  constructor(
    message: string,
    /// `cancelled`: the person said no, or nothing was there (browsers
    /// don't tell which); `unsupported`: this browser or passkey can't;
    /// `exists`: this account has a passkey in that manager already.
    readonly kind: 'cancelled' | 'unsupported' | 'exists' | 'other',
  ) {
    super(message);
  }
}

type PrfResults = { prf?: { enabled?: boolean; results?: { first?: BufferSource } } };

export async function support(): Promise<Support> {
  if (!window.isSecureContext || !window.PublicKeyCredential || !navigator.credentials) return 'no';
  const caps = await (PublicKeyCredential as unknown as { getClientCapabilities?: () => Promise<Record<string, boolean>> })
    .getClientCapabilities?.()
    .catch(() => null);
  if (caps && 'extension:prf' in caps) return caps['extension:prf'] ? 'yes' : 'no';
  return 'maybe';
}

function random(n: number): Uint8Array<ArrayBuffer> {
  return crypto.getRandomValues(new Uint8Array(n));
}

function bytes(b: BufferSource): Uint8Array<ArrayBuffer> {
  return b instanceof ArrayBuffer ? new Uint8Array(b) : new Uint8Array(b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer);
}

/// A browser's DOMException as one of ours.
function plain(e: unknown, making: boolean): PasskeyError {
  if (e instanceof PasskeyError) return e;
  const name = e instanceof DOMException ? e.name : '';
  if (name === 'NotAllowedError' || name === 'AbortError') {
    return new PasskeyError(
      making ? 'No passkey was made: it was cancelled, or took too long.' : 'No passkey was used: it was cancelled, or this browser has none for Carnet here.',
      'cancelled',
    );
  }
  if (name === 'InvalidStateError') return new PasskeyError('A passkey for this account is in that passkey manager already.', 'exists');
  if (name === 'NotSupportedError') return new PasskeyError('This browser can’t make passkeys.', 'unsupported');
  if (name === 'SecurityError') return new PasskeyError('Passkeys work only at the server’s own address, over https.', 'unsupported');
  return new PasskeyError(e instanceof Error ? e.message : String(e), 'other');
}

const NO_PRF = 'That passkey can’t unlock your notes: the app that keeps it doesn’t give what Carnet needs. Try a passkey kept by this browser, your phone, or a security key.';

/// A passkey used: the one chosen (or one of `allow`): its credential id,
/// its PRF output, and the account it names.
export async function use(allow: Uint8Array<ArrayBuffer>[] = []): Promise<{ credential: Uint8Array<ArrayBuffer>; prf: Uint8Array<ArrayBuffer>; account: Uint8Array<ArrayBuffer> | null }> {
  let got: PublicKeyCredential | null;
  try {
    got = (await navigator.credentials.get({
      publicKey: {
        challenge: random(32),
        userVerification: 'required',
        allowCredentials: allow.map((id) => ({ type: 'public-key', id })),
        timeout: 120_000,
        extensions: { prf: { eval: { first: passkey_salt() } } } as AuthenticationExtensionsClientInputs,
      },
    })) as PublicKeyCredential | null;
  } catch (e) {
    throw plain(e, false);
  }
  if (!got) throw new PasskeyError('No passkey was used.', 'cancelled');
  const first = (got.getClientExtensionResults() as PrfResults).prf?.results?.first;
  if (!first) throw new PasskeyError(NO_PRF, 'unsupported');
  const handle = (got.response as AuthenticatorAssertionResponse).userHandle;
  return { credential: new Uint8Array(got.rawId), prf: bytes(first), account: handle ? new Uint8Array(handle) : null };
}

/// A new passkey for the account `account` (its id), named for the person
/// as `label`; none of `exclude` made again. Its credential id and PRF
/// output.
export async function make(
  account: Uint8Array<ArrayBuffer>,
  label: string,
  exclude: Uint8Array<ArrayBuffer>[],
): Promise<{ credential: Uint8Array<ArrayBuffer>; prf: Uint8Array<ArrayBuffer> }> {
  let made: PublicKeyCredential | null;
  try {
    made = (await navigator.credentials.create({
      publicKey: {
        rp: { name: 'Carnet' },
        user: { id: account, name: label, displayName: label },
        challenge: random(32),
        // Ed25519, P-256, RSA: whatever the passkey has. Its key signs
        // nothing anyone checks; only its PRF matters.
        pubKeyCredParams: [-8, -7, -257].map((alg) => ({ type: 'public-key' as const, alg })),
        authenticatorSelection: { residentKey: 'required', requireResidentKey: true, userVerification: 'required' },
        excludeCredentials: exclude.map((id) => ({ type: 'public-key', id })),
        timeout: 120_000,
        extensions: { prf: { eval: { first: passkey_salt() } } } as AuthenticationExtensionsClientInputs,
      },
    })) as PublicKeyCredential | null;
  } catch (e) {
    throw plain(e, true);
  }
  if (!made) throw new PasskeyError('No passkey was made.', 'cancelled');
  const credential = new Uint8Array(made.rawId);
  const ext = made.getClientExtensionResults() as PrfResults;
  const first = ext.prf?.results?.first;
  if (first) return { credential, prf: bytes(first) };
  if (!ext.prf?.enabled) {
    forget(credential);
    throw new PasskeyError(NO_PRF, 'unsupported');
  }
  // Some passkeys give the PRF only when used, not when made: used now,
  // this one alone.
  const used = await use([credential]);
  return { credential, prf: used.prf };
}

/// Tells the passkey manager this site has no such passkey any more, so it
/// can stop offering it (where the browser can: Chrome, so far).
export function forget(credential: Uint8Array<ArrayBuffer>) {
  const signal = (PublicKeyCredential as unknown as { signalUnknownCredential?: (o: { rpId: string; credentialId: string }) => Promise<void> })
    .signalUnknownCredential;
  void signal?.call(PublicKeyCredential, { rpId: location.hostname, credentialId: base64url(credential) }).catch(() => {});
}

function base64url(b: Uint8Array): string {
  let s = '';
  for (const x of b) s += String.fromCharCode(x);
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export function fromHex(h: string): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(h.slice(2 * i, 2 * i + 2), 16);
  return out;
}
