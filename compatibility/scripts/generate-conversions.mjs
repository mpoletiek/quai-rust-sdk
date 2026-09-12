// Public deterministic test secrets only. Never fund. Offline wire/signature fixtures.
import {QuaiTransaction, QiTransaction, SigningKey, computeAddress, getAddress, getBytes, hexlify} from 'quais';
import {MuSigFactory} from '@brandonblack/musig';
import {musigCrypto} from '../node_modules/quais/lib/esm/crypto/musig.js';
import {schnorr} from '@noble/curves/secp256k1';
import {writeFileSync, readFileSync} from 'node:fs';
const existing=JSON.parse(readFileSync(new URL('../fixtures/transactions.json',import.meta.url)));
const qiKeys=existing.vectors.find(v=>v.id==='qi-multi-0').publicTestSecrets;
const quaiKey=existing.vectors.find(v=>v.kind==='quai').publicTestSecret;
const destination=computeAddress(quaiKey);
const refund=getAddress('0x0088223344556677889900112233445566778899');
const change=getAddress('0x0088223344556677889900112233445566778877');
const vectors=[];
const sets=[[qiKeys[0]],[qiKeys[1]],[qiKeys[0],qiKeys[1]],[qiKeys[1],qiKeys[0]],[qiKeys[0],qiKeys[0],qiKeys[1]],[qiKeys[0],qiKeys[0]]];
for(const [i,keys] of sets.entries()) {
 const tx=new QiTransaction();tx.chainId=15000n;
 const pubs=keys.map(key=>new SigningKey(key).compressedPublicKey);
 tx.txInputs=pubs.map((pubkey,index)=>({txhash:'0x00800080'+'33'.repeat(28),index,pubkey}));
 tx.txOutputs=[{address:destination,denomination:2},{address:destination,denomination:1},{address:change,denomination:0}];
 const slippage=[30,9000,31,100,250,8999][i];
 tx.data=getBytes('0x'+slippage.toString(16).padStart(4,'0')+refund.slice(2));
 const unsigned=tx.unsignedSerialized,digest=tx.digest;
 if(keys.length===1)tx.signature=hexlify(schnorr.sign(digest.slice(2),keys[0].slice(2),new Uint8Array(32).fill(i)));
 else {
  const musig=MuSigFactory(musigCrypto), publicKeys=pubs.map(p=>getBytes(p));
  const nonces=publicKeys.map((publicKey,j)=>musig.nonceGen({publicKey,secretKey:getBytes(keys[j]),sessionId:new Uint8Array(32).fill(160+i*8+j),msg:getBytes(digest)}));
  const session=musig.startSigningSession(musig.nonceAgg(nonces),getBytes(digest),publicKeys);
  tx.signature=hexlify(musig.signAgg(keys.map((key,j)=>musig.partialSign({secretKey:getBytes(key),publicNonce:nonces[j],sessionKey:session,verify:true})),session));
 }
 vectors.push({id:`conversion-qi-${i}`,kind:'qi',publicTestSecrets:keys,intent:{destination,refund,slippage},input:tx.toJSON(),unsigned,digest,signed:tx.serialized,hash:tx.hash});
}
for(let i=0;i<2;i++) {
 const tx=new QuaiTransaction(computeAddress(quaiKey));tx.type=0;tx.chainId=15000n;tx.nonce=i;tx.to=refund;tx.value=10n**19n+BigInt(i);tx.gasLimit=200000n;tx.gasPrice=1000000000n;
 const slippage=i?9000:30;tx.data='0x'+slippage.toString(16).padStart(4,'0');
 const unsigned=tx.unsignedSerialized,digest=tx.digest;
 tx.signature=new SigningKey(quaiKey).sign(digest);
 vectors.push({id:`conversion-quai-${i}`,kind:'quai',publicTestSecret:quaiKey,intent:{destination:refund,slippage},input:tx.toJSON(),unsigned,digest,signed:tx.serialized,hash:tx.hash});
}
writeFileSync(new URL('../../crates/quai-consensus/tests/conversion-vectors.json',import.meta.url),JSON.stringify({schemaVersion:1,reference:'quais@1.0.0-alpha.57',warning:'PUBLIC TEST KEYS. Never fund. Offline wire/signature fixtures, not node acceptance or settlement.',vectors},null,2)+'\n');
console.log(`Generated ${vectors.length} conversion wire vectors`);
