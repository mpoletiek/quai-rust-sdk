// Public toy seeds only. Never fund these payment codes or addresses.
import {writeFileSync} from 'node:fs';
import {HDNodeWallet,getBytes,hexlify,sha256,Zone} from '../../../compatibility/node_modules/quais/lib/esm/index.js';
import {generatePaymentCodePrivate} from '../../../compatibility/node_modules/quais/lib/esm/wallet/qi-wallets/bip47-self-qi-wallet.js';
import {PaymentCodePublic} from '../../../compatibility/node_modules/quais/lib/esm/wallet/payment-codes.js';
import {BIP32Factory} from '../../../compatibility/node_modules/quais/lib/esm/wallet/bip32/bip32.js';
import ecc from '../../../compatibility/node_modules/@bitcoinerlab/secp256k1/dist/index.js';
const publicCode=(p)=>new PaymentCodePublic(ecc,BIP32Factory(ecc),p.paymentCode);
const create=(seed,account,coin=969)=>generatePaymentCodePrivate(HDNodeWallet.fromSeed(seed).derivePath(`m/47'/${coin}'`),account);
const pairs=[];
for(const [i,len,account] of [[0,16,0],[1,17,1],[2,32,17],[3,64,2147483647]]) {
 const senderSeed='0x'+(i*2+1).toString(16).padStart(2,'0').repeat(len);
 const receiverSeed='0x'+(i*2+2).toString(16).padStart(2,'0').repeat(len);
 const sender=create(senderSeed,account),receiver=create(receiverSeed,account);
 const senderPublic=publicCode(sender),receiverPublic=publicCode(receiver);
 const payments=[0,1,2,7,19,2147483647].map(index=>{
  const send=receiverPublic.derivePaymentPublicKey(sender,index),receive=receiver.derivePaymentPublicKey(senderPublic,index);
  if(hexlify(send)!==hexlify(receive))throw Error('JS send/receive mismatch');
  return {index,publicKey:hexlify(send),publicTestReceiveSecret:hexlify(receiver.derivePaymentPrivateKey(senderPublic,index)),address:receiver.getPaymentAddress(senderPublic,index)};
 });
 pairs.push({id:'quai-'+i,senderSeed,receiverSeed,account,senderCode:sender.toBase58(),receiverCode:receiver.toBase58(),senderPayload:hexlify(sender.paymentCode),receiverPayload:hexlify(receiver.paymentCode),senderNotificationKey:hexlify(sender.getNotificationPublicKey()),receiverNotificationKey:hexlify(receiver.getNotificationPublicKey()),payments});
}
const pair=pairs[0],sender=create(pair.senderSeed,0),receiver=create(pair.receiverSeed,0),sp=publicCode(sender),rp=publicCode(receiver);
const searches=[];
for(const zone of [0x00,0x11,0x22]) for(const direction of ['send','receive']) {
 let found;
 for(let index=0;index<10000;index++) {
  const pub=direction==='send'?rp.derivePaymentPublicKey(sender,index):sender.derivePaymentPublicKey(rp,index);
  const address=direction==='send'?rp.getPaymentAddress(sender,index):sender.getPaymentAddress(rp,index);
  const bytes=getBytes(address);
  if(bytes[0]===zone && (bytes[1]&0x80)!==0) {found={direction,zone,index,address,publicKey:hexlify(pub)};break;}
 }
 if(!found)throw Error('search budget');searches.push(found);
}
writeFileSync(new URL('fixtures/quais-payments.json',import.meta.url),JSON.stringify({schemaVersion:1,reference:'quais@1.0.0-alpha.57',warning:'PUBLIC TOY SEEDS; NEVER FUND',pairs,searches},null,2)+'\n');
console.log(pairs.length,'pairs',searches.length,'zone/direction searches');
