// Public deterministic test secrets only. Never fund these identities.
import {QuaiTransaction, QiTransaction, SigningKey, computeAddress, getAddress, getBytes, getZoneForAddress, isQiAddress, hexlify} from 'quais';
import {MuSigFactory} from '@brandonblack/musig';
import {musigCrypto} from '../node_modules/quais/lib/esm/crypto/musig.js';
import {schnorr} from '@noble/curves/secp256k1';
import {writeFileSync} from 'node:fs';
let quaiKey,qiKey;const qiKeys=[];
for(let n=1; n<100000 && (!quaiKey||qiKeys.length<2); n++) {
 const key='0x'+n.toString(16).padStart(64,'0'); const a=computeAddress(key);
 if(getZoneForAddress(a)==='0x00') { if(isQiAddress(a)){qiKey??=key;qiKeys.push(key);}else quaiKey??=key; }
}
const vectors=[];
for(let i=0;i<8;i++) {
 const tx=new QuaiTransaction(computeAddress(quaiKey)); tx.type=0; tx.chainId=i===0?9n:15000n;
 tx.to=i%3===0?null:getAddress('0x0011223344556677889900112233445566778899');
 tx.nonce=[0,1,127,128,255,256,65535,9007199254740991][i]; tx.gasLimit=21000n+BigInt(i)*1000n;
 tx.gasPrice=BigInt(i)*1000000000n;tx.value=i===7?(1n<<256n)-1n:BigInt(i)*17n;
 tx.data='0x'+'ab'.repeat([0,1,2,31,32,127,128,256][i]);
 if(i===6)tx.accessList=[{address:getAddress('0x0011223344556677889900112233445566778899'),storageKeys:['0x'+'00'.repeat(32),'0x'+'11'.repeat(32)]}];
 const unsigned=tx.unsignedSerialized, digest=tx.digest;
 tx.signature=new SigningKey(quaiKey).sign(digest);
 vectors.push({id:'quai-'+i,kind:'quai',publicTestSecret:quaiKey,input:tx.toJSON(),unsigned,digest,signed:tx.serialized,hash:tx.hash});
}
for(let i=0;i<3;i++) {
 const tx=new QiTransaction();tx.chainId=15000n;
 tx.txInputs=[{txhash:'0x00800080'+'11'.repeat(28),index:i,pubkey:new SigningKey(qiKey).compressedPublicKey}];
 tx.txOutputs=[{address:getAddress('0x0088223344556677889900112233445566778899'),denomination:i,lock:'0x'}];
 tx.data=new Uint8Array(i);
 const unsigned=tx.unsignedSerialized,digest=tx.digest;
 tx.signature=hexlify(schnorr.sign(digest.slice(2),qiKey.slice(2),new Uint8Array(32)));
 vectors.push({id:'qi-'+i,kind:'qi',publicTestSecret:qiKey,input:tx.toJSON(),unsigned,digest,signed:tx.serialized,hash:tx.hash});
}
for(const [i,keys] of [[qiKeys[0],qiKeys[1]],[qiKeys[1],qiKeys[0]],[qiKeys[0],qiKeys[0],qiKeys[1]]].entries()) {
 const tx=new QiTransaction();tx.chainId=15000n;
 const pubs=keys.map(key=>new SigningKey(key).compressedPublicKey);
 tx.txInputs=pubs.map((pubkey,index)=>({txhash:'0x00800080'+'22'.repeat(28),index,pubkey}));
 tx.txOutputs=[{address:getAddress('0x0088223344556677889900112233445566778899'),denomination:0}];
 const unsigned=tx.unsignedSerialized,digest=tx.digest;
 const musig=MuSigFactory(musigCrypto), publicKeys=pubs.map(p=>getBytes(p));
 const nonces=publicKeys.map((publicKey,j)=>musig.nonceGen({publicKey,secretKey:getBytes(keys[j]),sessionId:new Uint8Array(32).fill(64+i*8+j),msg:getBytes(digest)}));
 const session=musig.startSigningSession(musig.nonceAgg(nonces),getBytes(digest),publicKeys);
 const partials=keys.map((key,j)=>musig.partialSign({secretKey:getBytes(key),publicNonce:nonces[j],sessionKey:session,verify:true}));
 tx.signature=hexlify(musig.signAgg(partials,session));
 vectors.push({id:'qi-multi-'+i,kind:'qi',publicTestSecrets:keys,input:tx.toJSON(),unsigned,digest,signed:tx.serialized,hash:tx.hash});
}
writeFileSync(new URL('../fixtures/transactions.json', import.meta.url),JSON.stringify({schemaVersion:1,reference:'quais@1.0.0-alpha.57',warning:'PUBLIC TEST KEYS. Never fund. Offline wire vectors, not node acceptance. qi-1 and qi-2 have node-invalid ordinary-transfer data lengths.',vectors},null,2)+'\n');
console.log('generated',vectors.length,'transaction vectors');
