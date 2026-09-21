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
// The published package ships two halves and nothing here executes the one the
// hashes above cover. `src/` is TypeScript; the export map resolves `quais` to
// `lib/`, so the oracle, the fixtures and the declaration inventory all run and
// read the compiled build. In the pinned release those halves are not the same
// version, so the runtime half is pinned on its own terms: its recorded version
// and one aggregate digest over the whole tree. A future release that fixes the
// mismatch will fail here rather than silently move the behavioral reference.
const runtimeVersion = file => {
  const text = readFileSync(path.join(base, 'node_modules/quais', file), 'utf8');
  const found = text.match(/version\s*=\s*'([^']+)'/);
  assert.ok(found, `${file} declares no version`);
  return found[1];
};
const esmVersion = runtimeVersion('lib/esm/_version.js');
assert.equal(esmVersion, runtimeVersion('lib/commonjs/_version.js'), 'ESM and CommonJS builds disagree');
assert.equal(esmVersion, reference.quais.runtime.version, 'compiled runtime version changed');
assert.equal(
  esmVersion === reference.quais.version,
  reference.quais.runtime.matchesPublishedSource,
  'agreement between the compiled runtime and the published source changed',
);
const libRoot = path.join(base, 'node_modules/quais/lib');
const libFiles = readdirSync(libRoot, { recursive: true })
  .filter(file => statSync(path.join(libRoot, file)).isFile())
  .map(file => file.split(path.sep).join('/'))
  .sort();
assert.equal(libFiles.length, reference.quais.runtime.files, 'compiled runtime file count changed');
const aggregate = createHash('sha256');
for (const file of libFiles) {
  aggregate.update(file);
  aggregate.update('\0');
  aggregate.update(readFileSync(path.join(libRoot, file)));
  aggregate.update('\0');
}
assert.equal(aggregate.digest('hex'), reference.quais.runtime.sha256, 'compiled runtime tree changed');
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
console.log(
  `Compiled runtime pinned at ${esmVersion} over ${libFiles.length} files` +
    (reference.quais.runtime.matchesPublishedSource
      ? '.'
      : `; the published source is ${reference.quais.version}, so behavior evidence reflects ${esmVersion}.`),
);
