// Public toy keys only. Deterministic auxiliary bytes are fixture instrumentation,
// never a production randomness configuration. Execute the published wallet method.
import {QiHDWallet, SigningKey, computeAddress, getBytes, hexlify, keccak256, toUtf8Bytes} from 'quais';
import {schnorr} from '@noble/curves/secp256k1';
import {writeFileSync} from 'node:fs';
const keys=[];
for(let scalar=1;scalar<100000 && keys.length<2;scalar++) {
  const privateKey='0x'+scalar.toString(16).padStart(64,'0');
  const publicKey=SigningKey.computePublicKey(privateKey,true);
  const address=computeAddress(publicKey);
  if(address.slice(0,4)==='0x00' && (parseInt(address.slice(4,6),16)&128) && !keys.some(k=>k.publicKey.slice(2,4)===publicKey.slice(2,4)))keys.push({privateKey,publicKey,address});
}
if(keys.length!==2)throw Error('fixture key search exhausted');
const messages=['','hello','Quai wallet: café 🐬','e\u0301','é','\u0000inside\u0000','0x1234',new Uint8Array([0,255,128,1,0])];
const original=schnorr.sign;
const auxiliary=new Uint8Array(32).fill(7);
schnorr.sign=(message,key)=>original(message,key,auxiliary);
const vectors=[];
try {
  for(const key of keys) for(const message of messages) {
    const bytes=typeof message==='string'?toUtf8Bytes(message):message;
    const signature=await QiHDWallet.prototype.signMessage.call({getPrivateKey:()=>key.privateKey},key.address,message);
    if(!schnorr.verify(getBytes(signature),getBytes(keccak256(bytes)),getBytes(key.publicKey).slice(1)))throw Error('invalid reference signature');
    vectors.push({...key,message:hexlify(bytes),digest:keccak256(bytes),signature});
  }
} finally {schnorr.sign=original;}
writeFileSync(new URL('../fixtures/qi-messages.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',publicFixtureSecrets:true,auxiliary:hexlify(auxiliary),vectors},null,2)+'\n');
console.log(`Generated ${vectors.length} Qi message cases with both public-key parities`);
