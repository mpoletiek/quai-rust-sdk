// Offline execution of the published response waiter; no node or provider connection.
import test from 'node:test';
import assert from 'node:assert/strict';
import {ContractTransactionResponse} from 'quais';
// QuaiTransactionResponse is exposed as a type-only export; runtime responses
// use this exact superclass of the published contract response constructor.
const QuaiTransactionResponse = Object.getPrototypeOf(ContractTransactionResponse);
const hash = n => '0x00' + n.toString(16).padStart(62, '0');
const from = '0x0049CdA3305ccB9cB23E7Ce2528ceF555E9a5B32';
function original(provider, overrides = {}) {
  return new QuaiTransactionResponse({hash: hash(1), from, to: from,
    nonce: 5, data: '0x', value: 7n, gasLimit: 21000n, gasPrice: 2n,
    chainId: 9n, index: 0, type: 0, ...overrides}, provider).replaceableTransaction(16);
}
for (const reason of ['repriced', 'cancelled', 'replaced']) {
  test(`published waiter detects unregistered ${reason} candidate`, async () => {
    const calls = [];
    let replacement;
    const receipt = {blockNumber: 16, hash: hash(2), status: 1};
    const provider = {
      getTransactionReceipt: async h => h === hash(1) ? null : receipt,
      getBlockNumber: async () => 17,
      getTransactionCount: async () => 6,
      getTransaction: async () => null,
      getBlock: async (_shard, n, full) => {
        calls.push([n, full]);
        return {length: 1, *[Symbol.iterator]() { yield hash(2); },
          getTransaction: async () => replacement};
      },
    };
    const tx = original(provider);
    replacement = original(provider, {hash:hash(2), gasPrice:3n,
      ...(reason === 'cancelled' ? {value:0n} : {}),
      ...(reason === 'replaced' ? {to:'0x0000000000000000000000000000000000000001'} : {})});
    await assert.rejects(tx.wait(2, 100), error => {
      assert.equal(error.code, 'TRANSACTION_REPLACED');
      assert.equal(error.reason, reason);
      assert.equal(error.cancelled, reason !== 'repriced');
      assert.equal(error.replacement.hash, hash(2));
      assert.equal(error.receipt, receipt);
      return true;
    });
    assert.deepEqual(calls, [[16, true]]);
  });
}
test('published zero-confirmation wait returns absence without scanning', async () => {
  const tx = original({getTransactionReceipt: async () => null});
  assert.equal(await tx.wait(0, 100), null);
});
test('published getTransaction checks a Promise before awaiting it', async () => {
  const provider = {getTransaction: async () => original(provider)};
  assert.equal(await original(provider).getTransaction(), null);
});
