import { test } from 'node:test';
import assert from 'node:assert/strict';
import { QuaiHDWallet, QiHDWallet, Mnemonic, wordlists, Zone, computeAddress, getZoneForAddress, isQiAddress } from 'quais';

// Published BIP39 test mnemonic and deterministic toy keys only. Never fund them.
const phrase = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';

// These assertions preserve evidence of upstream defects. They define behavior
// Rust MUST FIX, not compatibility behavior Rust should reproduce.
test('A02: legacy serialization loses BIP39 passphrase and changes identity', async () => {
  const original = QuaiHDWallet.fromPhrase(phrase, 'audit-public-passphrase');
  const restored = await QuaiHDWallet.deserialize(original.serialize());
  assert.notEqual(original.xPub(), restored.xPub());
});

test('A02: French mnemonic cannot be restored through legacy English-only import', async () => {
  const mnemonic = Mnemonic.fromEntropy(`0x${'00'.repeat(16)}`, '', wordlists.fr);
  const original = QuaiHDWallet.fromMnemonic(mnemonic);
  await assert.rejects(() => QuaiHDWallet.deserialize(original.serialize()), { code: 'INVALID_ARGUMENT' });
});

test('A02: seed-only wallet has no supported legacy export', () => {
  const wallet = QuaiHDWallet.fromSeed(`0x${'01'.repeat(32)}`);
  assert.throws(() => wallet.serialize(), TypeError);
});

test('A01: imported private key leaks through nominal address metadata and export', async () => {
  let key;
  for (let value = 1n; value < 10000n; value++) {
    const candidate = `0x${value.toString(16).padStart(64, '0')}`;
    const address = computeAddress(candidate);
    if (getZoneForAddress(address) && isQiAddress(address)) { key = candidate; break; }
  }
  assert.ok(key, 'deterministic public toy Qi key must be found');
  const wallet = QiHDWallet.fromPhrase(phrase);
  const metadata = await wallet.importPrivateKey(key);
  assert.equal(metadata.derivationPath, key);
  assert.ok(wallet.serialize().addresses.some(record => record.derivationPath === key));
});

test('A04: metadata-only restore retains checkpoint with no outpoint snapshot', async () => {
  const wallet = QiHDWallet.fromPhrase(phrase);
  await wallet.getNextAddress(0, Zone.Cyprus1);
  const serialized = wallet.serialize();
  const checkpoint = { hash: `0x${'11'.repeat(32)}`, number: 100 };
  serialized.addresses[0].lastSyncedBlock = checkpoint;
  const restored = await QiHDWallet.deserialize(serialized);
  assert.deepEqual(restored.getAddressesForZone(Zone.Cyprus1)[0].lastSyncedBlock, checkpoint);
  assert.deepEqual(restored.getOutpoints(Zone.Cyprus1), []);
  // Synthetic checkpoint only: this does not claim a pre-checkpoint funded
  // output was lost on a running node. That requires the planned live test.
});
