import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { outcome, routing, jsonlResponse } from './reference.mjs';

for (const vector of JSON.parse(readFileSync(new URL('../fixtures/primitives.json', import.meta.url))).vectors) {
  test(`reference primitive: ${vector.id}`, () => assert.deepEqual(outcome(vector), vector.expected));
}
for (const vector of JSON.parse(readFileSync(new URL('../fixtures/routing.json', import.meta.url))).vectors) {
  test(`reference routing: ${vector.id}`, () => assert.equal(routing(vector.input), vector.expected));
}
test('JSONL oracle rejects malformed and unallowlisted requests and continues', () => {
  const input = 'bad json\n{"operation":"signTransaction","input":{}}\n{"id":"ok","operation":"parseUnits","input":{"value":"1","decimals":5}}';
  assert.deepEqual(input.split('\n').map(jsonlResponse), [
    { result: { error: { code: 'INVALID_REQUEST' } } },
    { result: { error: { code: 'UNSUPPORTED_OPERATION' } } },
    { id: 'ok', result: '100000' },
  ]);
});
