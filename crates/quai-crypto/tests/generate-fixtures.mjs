// Public test material only. Run from repository root after installing the pinned
// compatibility package: node crates/quai-crypto/tests/generate-fixtures.mjs.
import { writeFileSync } from 'node:fs';
import { SigningKey, computeAddress, hashMessage, getBytes } from '../../../compatibility/node_modules/quais/lib/esm/index.js';
const samples = [1n, 2n, 3n, 805n];
const order = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n;
const digests = ['00'.repeat(32), 'ff'.repeat(32), '0123456789abcdef'.repeat(4),
  ...[order - 1n, order, order + 1n].map(n => n.toString(16).padStart(64, '0'))];
const vectors = [];
for (const scalar of samples) {
  const publicTestSecret = '0x' + scalar.toString(16).padStart(64, '0');
  const signingKey = new SigningKey(publicTestSecret);
  for (const digest of digests) {
    const digestHex = '0x' + digest;
    vectors.push({ publicTestSecret, digest: digestHex, compressed: signingKey.compressedPublicKey,
      uncompressed: signingKey.publicKey, address: computeAddress(signingKey),
      signature: signingKey.sign(digestHex).serialized });
  }
}
const messages = ['', 'Hello Quai', '0x4243', '🦀 Quai e\u0301', 'x'.repeat(256)].map(text => ({
  encoding: 'utf8', input: text, expected: hashMessage(text)
}));
messages.push({ encoding: 'hex', input: '0x4243', expected: hashMessage(getBytes('0x4243')) });
writeFileSync(new URL('./fixtures/quais-crypto.json', import.meta.url), JSON.stringify({
  schemaVersion: 1, reference: 'quais@1.0.0-alpha.57',
  warning: 'PUBLIC TEST KEYS. Never fund. These are not node acceptance fixtures.', vectors, messages
}, null, 2) + '\n');
