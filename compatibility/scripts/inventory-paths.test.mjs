import { test } from 'node:test';
import assert from 'node:assert/strict';
import { portableInventory } from './inventory-paths.mjs';
test('compiler namespace names and imports are independent of checkout location', () => {
  const make = root => ({ target: { name: `"${root}/lib/esm/quais"`, type: `typeof import("${root}/lib/esm/quais")` }, signatures: ['(path: string): void'], count: 1 });
  const local = '/home/developer/sdk/compatibility/node_modules/quais';
  const ci = '/home/runner/work/sdk/sdk/compatibility/node_modules/quais';
  assert.deepEqual(portableInventory(make(local), local + '/'), portableInventory(make(ci), ci + '/'));
  assert.equal(portableInventory(make(ci), ci).target.name, '"quais/lib/esm/quais"');
});
