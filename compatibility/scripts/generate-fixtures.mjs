import { writeFileSync } from 'node:fs';
import { outcome, routing } from './reference.mjs';

const vectors = [];
function add(id, operation, input) { vectors.push({ id, operation, input, expected: outcome({ operation, input }) }); }
for (const zone of ['00', '01', '02', '10', '11', '12', '20', '21', '22', '03', 'ff']) {
  for (const ledger of ['00', '80']) {
    add(`address-${zone}-${ledger}`, 'address', { address: `0x${zone}${ledger}aabbccddeeff00112233445566778899aabb` });
  }
}
for (const [id, address] of Object.entries({
  zero: `0x${'00'.repeat(20)}`,
  uppercase: '0x0080AABBCCDDEEFF00112233445566778899AABB',
  short: '0x00',
  nonhex: `0x${'gg'.repeat(20)}`,
  'bad-checksum': '0x0080AaBbCCDDEEFF00112233445566778899AABB',
})) add(`address-${id}`, 'address', { address });
for (const message of ['', 'Hello Quai', '0x4243', 'Quai 🌍', 'é']) {
  add(`message-text-${vectors.length}`, 'hashMessage', { encoding: 'utf8', message });
}
for (const message of ['0x', '0x4243', '0x00ff']) add(`message-bytes-${message}`, 'hashMessage', { encoding: 'hex', message });
for (const nonce of ['0', '1', '255', '256', '18446744073709551615']) {
  for (const data of [null, '0x', '0x60006000']) add(`create-${nonce}-${data}`, 'getCreateAddress', {
    from: '0x0000aabbccddeeff00112233445566778899aabb', nonce, data,
  });
}
for (const data of ['0x', '0x00', '0x4243', `0x${'ff'.repeat(32)}`]) add(`keccak-${data}`, 'keccak256', { data });
for (const [value, decimals] of [['1', 18], ['1.23456', 5], ['0.00001', 5], ['-1.5', 18], ['0', 0], ['1.000001', 5]]) {
  add(`parse-${value}-${decimals}`, 'parseUnits', { value, decimals });
}
for (const [value, decimals] of [['1000000000000000000', 18], ['123456', 5], ['1', 5], ['-1500000000000000000', 18], ['0', 0]]) {
  add(`format-${value}-${decimals}`, 'formatUnits', { value, decimals });
}
const document = (vectors, extra = {}) => ({ schemaVersion: 1, reference: 'quais@1.0.0-alpha.57', ...extra, vectors });
writeFileSync(new URL('../fixtures/primitives.json', import.meta.url), `${JSON.stringify(document(vectors), null, 2)}\n`);
const routes = [];
for (const usePathing of [false, true]) {
  for (const url of ['http://127.0.0.1:9002', 'https://example.invalid/rpc', 'https://example.invalid/rpc/?key=public-fixture']) {
    for (const shard of ['0x', '0x0', '0x00', '0x22']) {
      const input = { url, usePathing, shard };
      routes.push({ id: `route-${routes.length}`, input, expected: routing(input) });
    }
  }
}
const input = { url: 'https://example.invalid/api', usePathing: true, shard: '0x00', shardPaths: { cyprus1: '/custom/zone' } };
routes.push({ id: 'route-custom-path', input, expected: routing(input) });
writeFileSync(new URL('../fixtures/routing.json', import.meta.url), `${JSON.stringify(document(routes, {
  scope: 'Offline explicit-shard URL construction; does not exercise discovery, authentication, transport or node acceptance.',
}), null, 2)}\n`);
console.log(`Generated ${vectors.length} primitive and ${routes.length} routing vectors.`);
