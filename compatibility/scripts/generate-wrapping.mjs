// Public deterministic test keys only. Offline envelopes, not node acceptance.
import {QiTransaction, SigningKey, computeAddress, getAddress, getBytes, hexlify, Interface} from 'quais';
import {MuSigFactory} from '@brandonblack/musig';
import {musigCrypto} from '../node_modules/quais/lib/esm/crypto/musig.js';
import {schnorr} from '@noble/curves/secp256k1';
import {readFileSync, writeFileSync} from 'node:fs';
const existing=JSON.parse(readFileSync(new URL('../fixtures/transactions.json',import.meta.url)));
const keys=existing.vectors.find(v=>v.id==='qi-multi-0').publicTestSecrets;
const beneficiary=computeAddress(existing.vectors.find(v=>v.kind==='quai').publicTestSecret);
const owner=getAddress('0x002b2596EcF05C93a31ff916E8b456DF6C77c750');
const vectors=[];
for(const [i, secrets] of [[keys[0]], keys, [...keys].reverse(), [keys[0],keys[0]]].entries()) {
 const tx=new QiTransaction(); tx.chainId=15000n; tx.data=getBytes(owner);
 const pubs=secrets.map(key=>new SigningKey(key).compressedPublicKey);
 tx.txInputs=pubs.map((pubkey,index)=>({txhash:'0x00800080'+'33'.repeat(28),index,pubkey}));
 tx.txOutputs=[{address:beneficiary,denomination:2},{address:beneficiary,denomination:1},{address:getAddress('0x0088223344556677889900112233445566778877'),denomination:0}];
 const unsigned=tx.unsignedSerialized,digest=tx.digest;
 if(secrets.length===1) tx.signature=hexlify(schnorr.sign(digest.slice(2),secrets[0].slice(2),new Uint8Array(32)));
 else {
  const musig=MuSigFactory(musigCrypto), publicKeys=pubs.map(getBytes);
  const nonces=publicKeys.map((publicKey,j)=>musig.nonceGen({publicKey,secretKey:getBytes(secrets[j]),sessionId:new Uint8Array(32).fill(50+i*8+j),msg:getBytes(digest)}));
  const session=musig.startSigningSession(musig.nonceAgg(nonces),getBytes(digest),publicKeys);
  tx.signature=hexlify(musig.signAgg(secrets.map((key,j)=>musig.partialSign({secretKey:getBytes(key),publicNonce:nonces[j],sessionKey:session,verify:true})),session));
 }
 vectors.push({id:`wrapping-${i}`,kind:'qi',publicTestSecrets:secrets,unsigned,digest,signed:tx.serialized,hash:tx.hash,input:tx.toJSON()});
}
writeFileSync(new URL('../../crates/quai-consensus/tests/wrapping-vectors.json',import.meta.url),JSON.stringify({schemaVersion:1,reference:'quais@1.0.0-alpha.57',warning:'PUBLIC TEST KEYS. Never fund.',vectors},null,2)+'\n');
const abi=new Interface(['function deposit() payable','function withdraw(uint256)','function claimDeposit() returns (uint256)','function unwrapQi(address,uint256,uint64)']);
const calls=[['deposit',[]],['withdraw',['1000000000000000000']],['claimDeposit',[]],['unwrapQi',['0x0080000000000000000000000000000000000001','1000000000000000000','1000000']]].map(([method,args])=>({method,args,data:abi.encodeFunctionData(method,args)}));
writeFileSync(new URL('../../crates/quai-sdk/tests/wrapper-calls.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',calls},null,2)+'\n');
console.log(`Generated ${vectors.length} wrapping envelopes and ${calls.length} contract calls`);
