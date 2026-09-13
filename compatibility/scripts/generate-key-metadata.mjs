// Public BIP32 metadata from the pinned SDK; no new secrets or network input.
import {HDNodeWallet} from 'quais';
import {readFileSync,writeFileSync} from 'node:fs';
const source=JSON.parse(readFileSync(new URL('../../crates/quai-wallet/tests/reference.json',import.meta.url)));
const inputs=[...source.extended,...source.extended.filter(v=>v.path==='m').flatMap(v=>["m/0'", "m/2147483647'"].map(path=>({...v,path})))];
const vectors=inputs.map(v=>{const n=HDNodeWallet.fromSeed(v.seed).derivePath(v.path);return {seed:v.seed,xpub:n.neuter().extendedKey,path:v.path,depth:n.depth,index:n.index,fingerprint:n.fingerprint,parentFingerprint:n.parentFingerprint,chainCode:n.chainCode};});
writeFileSync(new URL('../fixtures/key-metadata.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',publicTestFixturesOnly:true,vectors},null,2)+'\n');
console.log(`Generated ${vectors.length} BIP32 metadata cases`);
