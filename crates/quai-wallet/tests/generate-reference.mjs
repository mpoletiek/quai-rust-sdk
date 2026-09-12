// Regenerate only after reviewing the pinned compatibility reference. Public
// BIP39/toy fixtures: none of these secrets may ever receive real funds.
import { createRequire } from 'node:module';
import { readFileSync, writeFileSync } from 'node:fs';
const metadata = JSON.parse(readFileSync(new URL('../../../compatibility/node_modules/quais/package.json', import.meta.url)));
if (metadata.version !== '1.0.0-alpha.57') throw new Error('wrong quais reference version');
const require = createRequire(new URL('../../../compatibility/package.json', import.meta.url));
const q = require('quais');
const wordlists = Object.entries(q.wordlists).map(([language, list]) => ({
  language, words: Array.from({ length: 2048 }, (_, index) => list.getWord(index)),
}));
const mnemonics = [];
for (const language of Object.keys(q.wordlists)) {
  for (const length of [16, 20, 24, 28, 32]) {
    const entropy = `0x${Array.from({ length }, (_, index) => index.toString(16).padStart(2, '0')).join('')}`;
    const passphrase = length === 16 ? 'TREZOR' : 'é㍍ガバヴァぱばぐゞちぢ十人十色';
    const mnemonic = q.Mnemonic.fromEntropy(entropy, passphrase, q.wordlists[language]);
    mnemonics.push({ language, entropy, passphrase, phrase: mnemonic.phrase, seed: mnemonic.computeSeed() });
  }
}
const extended = [];
for (const length of [16, 17, 20, 31, 32, 33, 63, 64]) {
  const seed = `0x${Array.from({ length }, (_, index) => index.toString(16).padStart(2, '0')).join('')}`;
  for (const path of ['m', "m/0'/1/2'/2/1000000000", "m/44'/994'/0'/0/0"]) {
    const node = q.HDNodeWallet.fromSeed(seed).derivePath(path);
    extended.push({ seed, path, xprv: node.extendedKey, xpub: node.neuter().extendedKey, publicKey: node.publicKey, address: node.address });
  }
}
const phrase = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
const grinding = [];
for (const coin of [994, 969]) {
  for (const [zone, change, account, startIndex] of [['0x00', false, 0, 0], ['0x22', false, 1, 37], ['0x10', true, 0, 0]]) {
    const walletRoot = q.HDNodeWallet.fromPhrase(phrase, `m/44'/${coin}'`, '');
    const accountRoot = walletRoot.deriveChild(account + 0x80000000);
    const branch = accountRoot.deriveChild(Number(change));
    for (let index = startIndex; index < startIndex + 10000; index++) {
      const node = branch.deriveChild(index);
      if (q.getZoneForAddress(node.address) !== zone || q.isQiAddress(node.address) !== (coin === 969)) continue;
      grinding.push({ phrase, coin, zone, change, account, startIndex, index, path: node.path, address: node.address, publicKey: node.publicKey, rootXpub: walletRoot.neuter().extendedKey, accountXpub: accountRoot.neuter().extendedKey });
      break;
    }
  }
}
if (grinding.length !== 6) throw new Error('missing grinding fixture');
writeFileSync(new URL('reference.json', import.meta.url), `${JSON.stringify({ schemaVersion: 1, reference: 'quais@1.0.0-alpha.57', publicTestSecretsOnly: true, wordlists, mnemonics, extended, grinding }, null, 2)}\n`);
console.log(`Generated ${wordlists.length} complete wordlists, ${mnemonics.length} mnemonics, ${extended.length} extended keys, ${grinding.length} grinding vectors.`);
