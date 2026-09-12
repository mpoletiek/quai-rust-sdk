// Public test keys only. Reproduces ordered key aggregation through pinned JS.
import {writeFileSync} from 'node:fs';
import {MuSigFactory} from '../../../compatibility/node_modules/@brandonblack/musig/lib/index.js';
import {musigCrypto} from '../../../compatibility/node_modules/quais/lib/esm/crypto/musig.js';
import {SigningKey, getBytes, hexlify, keccak256, toUtf8Bytes} from '../../../compatibility/node_modules/quais/lib/esm/index.js';
const sets=[[1,2],[2,1],[1,1],[1,1,1],[1,2,1],[1,1,2],[2,1,1],[1,2,3],[3,2,1],[1,2,2],[2,2,1]];
const vectors=sets.map((set,i)=>{
 const musig=MuSigFactory(musigCrypto);
 const secrets=set.map(n=>getBytes('0x'+BigInt(n).toString(16).padStart(64,'0')));
 const keys=secrets.map(s=>getBytes(new SigningKey(s).compressedPublicKey));
 const context=musig.keyAgg(keys);
 const digest=getBytes(keccak256(toUtf8Bytes('quai-local-key-aggregation-'+i)));
 const nonces=keys.map((publicKey,j)=>musig.nonceGen({publicKey,secretKey:secrets[j],sessionId:new Uint8Array(32).fill(i*8+j+1),msg:digest}));
 const session=musig.startSigningSession(musig.nonceAgg(nonces),digest,keys);
 const partials=secrets.map((secretKey,j)=>musig.partialSign({secretKey,publicNonce:nonces[j],sessionKey:session,verify:true}));
 return {id:'ordered-'+i,publicTestSecrets:secrets.map(hexlify),publicKeys:keys.map(hexlify),aggregatePublicKey:hexlify(musig.getPlainPubkey(context)),digest:hexlify(digest),jsSignature:hexlify(musig.signAgg(partials,session))};
});
writeFileSync(new URL('./fixtures/ordered-musig.json', import.meta.url),JSON.stringify({schemaVersion:1,reference:'quais@1.0.0-alpha.57 + @brandonblack/musig@0.0.1-alpha.1',warning:'PUBLIC TEST KEYS; NEVER FUND; LOCAL KEY AGGREGATION ONLY',vectors},null,2)+'\n');
console.log('generated',vectors.length,'ordered key aggregation vectors');
