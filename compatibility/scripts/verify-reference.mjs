import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const base = fileURLToPath(new URL('../', import.meta.url));
const json = file => JSON.parse(readFileSync(path.join(base, file), 'utf8'));
const hash = (bytes, algorithm = 'sha256', encoding = 'hex') => createHash(algorithm).update(bytes).digest(encoding);
const reference = json('reference-lock.json');
const lock = json('package-lock.json');
const manifest = json('reference-source-manifest.json');
assert.equal(hash(readFileSync(path.join(base, 'package-lock.json'))), reference.dependencyLockSha256);
assert.equal(lock.packages['node_modules/quais'].version, reference.quais.version);
assert.equal(lock.packages['node_modules/quais'].integrity, reference.quais.integrity);
assert.equal(lock.packages['node_modules/typescript'].version, reference.toolchain.typescript);
assert.equal(lock.packages['node_modules/typescript'].integrity, reference.toolchain.typescriptIntegrity);
assert.equal(json('node_modules/quais/package.json').version, reference.quais.version);
assert.equal(json('node_modules/typescript/package.json').version, reference.toolchain.typescript);
const sourceRoot = path.join(base, 'node_modules/quais/src');
const installed = readdirSync(sourceRoot, { recursive: true }).filter(file => statSync(path.join(sourceRoot, file)).isFile()).map(file => `src/${file}`).sort();
assert.deepEqual(installed, manifest.files.map(entry => entry.file));
for (const entry of manifest.files) {
  assert.equal(hash(readFileSync(path.join(base, 'node_modules/quais', entry.file))), entry.sha256, entry.file);
}
for (let index = 2; index < process.argv.length; index += 2) {
  const option = process.argv[index], argument = process.argv[index + 1];
  assert.ok(argument, `${option} needs a path`);
  if (option === '--tarball') {
    const bytes = readFileSync(argument);
    assert.equal(hash(bytes), reference.quais.sha256);
    assert.equal(`sha512-${hash(bytes, 'sha512', 'base64')}`, reference.quais.integrity);
  } else if (option === '--source') {
    for (const entry of manifest.files) assert.equal(hash(readFileSync(path.join(argument, entry.file))), entry.sha256, entry.file);
  } else {
    throw new Error(`Unknown option: ${option}`);
  }
}
console.log(`Verified dependency lock, installed versions, and ${manifest.files.length} published source hashes.`);
