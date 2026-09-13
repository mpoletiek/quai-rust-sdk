// Published HD wallet behavior with public toy seeds, no RPC or funded keys.
import test from 'node:test';
import assert from 'node:assert/strict';
import {QiHDWallet, QuaiHDWallet, Zone} from 'quais';
const seed = '0x'+'07'.repeat(32);
const peerSeed = '0x'+'08'.repeat(32);

test('reopening a published payment channel repeats its first receive address', async () => {
  const wallet = QiHDWallet.fromSeed(seed);
  const code = QiHDWallet.fromSeed(peerSeed).getPaymentCode();
  wallet.openChannel(code);
  assert.equal(wallet.channelIsOpen(code), true);
  const first = await wallet.getNextReceiveAddress(code, Zone.Cyprus1);
  const second = await wallet.getNextReceiveAddress(code, Zone.Cyprus1);
  assert.notEqual(first.address, second.address);
  wallet.openChannel(code);
  assert.equal((await wallet.getNextReceiveAddress(code, Zone.Cyprus1)).address, first.address);
  assert.deepEqual(wallet.openChannels, [code]);
  // Evidence of a bug Rust must not reproduce: allocated indexes must not rewind.
});

test('Qi address getters separate external, change, imported and unobserved metadata', async () => {
  const wallet = QiHDWallet.fromSeed(seed);
  const external = await wallet.getNextAddress(0, Zone.Cyprus1);
  const change = await wallet.getNextChangeAddress(0, Zone.Cyprus1);
  const imported = await wallet.importPrivateKey('0x'+(130n).toString(16).padStart(64,'0'));
  assert.deepEqual(wallet.getAddressesForZone(Zone.Cyprus1), [external]);
  assert.deepEqual(wallet.getChangeAddressesForZone(Zone.Cyprus1), [change]);
  assert.deepEqual(wallet.getImportedAddresses(Zone.Cyprus1), [imported]);
  assert.deepEqual(new Set(wallet.getAddressesForAccount(0).map(a=>a.address)),
    new Set([external.address, change.address, imported.address]));
  assert.deepEqual(wallet.getGapAddressesForZone(Zone.Cyprus1), []);
  assert.deepEqual(wallet.getGapChangeAddressesForZone(Zone.Cyprus1), []);
  assert.deepEqual(wallet.getPaymentChannelAddressesForZone('unopened', Zone.Cyprus1), []);
  for (const address of [external,change,imported]) {
    assert.equal(wallet.getAddressInfo(address.address), address);
  }
  const exact = QiHDWallet.fromSeed(seed);
  assert.equal(exact.addAddress(0,external.index).address,external.address);
  assert.equal(exact.addChangeAddress(0,change.index).address,change.address);
  assert.throws(()=>exact.addAddress(0,external.index));
  assert.equal(exact.getAddressInfo(imported.address),null);
});

test('HD provider connection is mutable and propagates to an existing payment channel', () => {
  const provider = {marker:'no network'};
  const quai = QuaiHDWallet.fromSeed(seed);
  quai.connect(provider);
  assert.equal(quai.provider,provider);
  const qi = QiHDWallet.fromSeed(seed);
  const code = QiHDWallet.fromSeed(peerSeed).getPaymentCode();
  qi.openChannel(code);
  qi.connect(provider);
  assert.equal(qi.provider,provider);
  for (const child of [qi.externalBip44,qi.changeBip44,qi.privatekeyWallet,
    qi.paymentChannels.get(code).selfWallet]) assert.equal(child.provider,provider);
});
