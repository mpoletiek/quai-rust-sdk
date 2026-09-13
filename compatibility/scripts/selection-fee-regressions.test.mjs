// Published fee-adjustment defects are evidence to correct, never reproduce.
import test from 'node:test';
import assert from 'node:assert/strict';
import {FewestCoinSelector, UTXO, denominations} from 'quais/transaction';
const coin = (denomination, index) => UTXO.from({denomination,index,
  txhash:'0x00800080'+'11'.repeat(28),address:'0x0088223344556677889900112233445566778899'});
const value = coins => coins.reduce((total, c) => total + denominations[c.denomination], 0n);
const fee = result => value(result.inputs) - value(result.spendOutputs) - value(result.changeOutputs);
test('published increaseFee can return success with an uncovered fee shortfall', () => {
  const selector = new FewestCoinSelector([coin(2,0),coin(0,1)]);
  assert.equal(fee(selector.performSelection({target:5n,fee:1n})),1n);
  const increased = selector.increaseFee(6n);
  assert.equal(fee(increased),6n);
  assert.notEqual(fee(increased),7n);
});
test('published decreaseFee overwrites existing change and can increase the paid fee', () => {
  const selector = new FewestCoinSelector([coin(2,0)]);
  assert.equal(fee(selector.performSelection({target:5n,fee:3n})),3n);
  assert.equal(fee(selector.decreaseFee(1n)),4n);
});
test('published UTXO from/toJSON omit lock metadata', () => {
  const input = {denomination:2,index:0,txhash:'0x00800080'+'11'.repeat(28),
    address:'0x0088223344556677889900112233445566778899',lock:100};
  const parsed = UTXO.from(input);
  assert.equal(parsed.lock,null);
  parsed.lock = 100;
  assert.equal(Object.hasOwn(parsed.toJSON(),'lock'),false);
});
