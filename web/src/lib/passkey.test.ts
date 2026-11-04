import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../wasm/carnet_web.js', () => ({ passkey_salt: () => new Uint8Array(32).fill(7) }));
import { make, use } from './passkey';

const create = vi.fn();
const get = vi.fn();
const credential = new Uint8Array([1, 2, 3]);
const secret = new Uint8Array(32).fill(42);

function result(prf?: object) {
  return {
    rawId: credential.buffer,
    response: { userHandle: new Uint8Array([4, 5]).buffer },
    getClientExtensionResults: () => (prf ? { prf } : {}),
  };
}

beforeEach(() => {
  vi.stubGlobal('navigator', { credentials: { create, get } });
  vi.stubGlobal('location', { hostname: 'notes.example' });
  vi.stubGlobal('PublicKeyCredential', {});
});
afterEach(() => {
  vi.resetAllMocks();
  vi.unstubAllGlobals();
});

describe('passkeys used to decrypt notes', () => {
  it('accepts PRF output from any provider without requesting a native authenticator', async () => {
    create.mockResolvedValue(result({ enabled: true, results: { first: secret.buffer } }));
    await expect(make(new Uint8Array([4, 5]), 'Notes', [])).resolves.toEqual({ credential, prf: secret });
    const options = create.mock.calls[0][0].publicKey;
    expect(options.authenticatorSelection.authenticatorAttachment).toBeUndefined();
    expect(options.extensions.prf.eval.first).toEqual(new Uint8Array(32).fill(7));
    expect(get).not.toHaveBeenCalled();
  });

  it('uses the new credential once when the provider evaluates PRF only after creation', async () => {
    create.mockResolvedValue(result({ enabled: true }));
    get.mockResolvedValue(result({ results: { first: secret.buffer } }));
    await expect(make(new Uint8Array([4, 5]), 'Notes', [])).resolves.toEqual({ credential, prf: secret });
    expect(get.mock.calls[0][0].publicKey.allowCredentials).toEqual([{ type: 'public-key', id: credential }]);
  });

  it.each([undefined, { enabled: false }])('explains missing PRF at creation as compatibility (%j)', async (prf) => {
    create.mockResolvedValue(result(prf));
    await expect(make(new Uint8Array([4, 5]), 'Notes', [])).rejects.toMatchObject({
      kind: 'unsupported',
      message: expect.stringContaining('compatibility limitation'),
    });
    expect(get).not.toHaveBeenCalled();
  });

  it('cannot unlock notes with an ordinary authentication-only passkey', async () => {
    get.mockResolvedValue(result());
    await expect(use()).rejects.toMatchObject({ kind: 'unsupported', message: expect.stringContaining('PRF') });
  });

  it('unlocks with the same PRF salt and accepts a provider’s typed array output', async () => {
    get.mockResolvedValue(result({ results: { first: secret } }));
    await expect(use()).resolves.toEqual({ credential, prf: secret, account: new Uint8Array([4, 5]) });
    expect(get.mock.calls[0][0].publicKey.extensions.prf.eval.first).toEqual(new Uint8Array(32).fill(7));
  });
});
