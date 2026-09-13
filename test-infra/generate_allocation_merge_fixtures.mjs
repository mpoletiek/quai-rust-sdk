// Independent framing of sealed-history journals. PUBLIC TOY DERIVATIONS ONLY.
import {readFileSync,writeFileSync} from 'node:fs';
import {pbkdf2Sync} from 'node:crypto';
const hdPhrase='abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
const hdSeed='0x'+pbkdf2Sync(hdPhrase.normalize('NFKD'),'mnemonic',2048,64,'sha512').toString('hex');
const read=p=>JSON.parse(readFileSync(new URL(p,import.meta.url)));
const hd=read('fixtures/address-allocation.json'),pay=read('fixtures/payment-allocation.json');
const addresses=[],payments=[];
for(const row of hd.vectors){for(const [state,old] of Object.entries(row.states)){
 const bytes=Buffer.from(old,'hex');bytes.write('QADDRBK2');
 const nextReceive=bytes.readUInt32BE(113)+100,nextChange=bytes.readUInt32BE(117)+200;
 for(const offset of [105,113])bytes.writeUInt32BE(nextReceive,offset);
 for(const offset of [109,117])bytes.writeUInt32BE(nextChange,offset);
 let abandoned=0;if(bytes.length>123&&bytes[148]===0){bytes[148]=2;abandoned=1;}
 const {states,...context}=row;addresses.push({...context,state,old,merged:bytes.toString('hex'),nextReceive,nextChange,abandoned});
}}
for(const row of pay.vectors){for(const [state,old] of Object.entries(row.states)){
 const bytes=Buffer.from(old,'hex');bytes.write('QPAYABK2');const nextIndex=bytes.readUInt32BE(109)+100;
 for(const offset of [105,109])bytes.writeUInt32BE(nextIndex,offset);
 let abandoned=0;if(bytes.length>115&&bytes[139]===0){bytes[139]=2;abandoned=1;}
 const {states,...context}=row;payments.push({...context,state,old,merged:bytes.toString('hex'),nextIndex,abandoned});
}}
writeFileSync(new URL('fixtures/allocation-merge.json',import.meta.url),JSON.stringify({formats:['QADDRBK2','QPAYABK2'],provenance:'Independent Node framing from pinned quais-derived v1 fixtures. Sealed history preserves completed/abandoned IDs below the maximum imported floor; pending requests are abandoned. Rust-only storage formats.',warning:'PUBLIC TOY SEEDS; NEVER FUND',hdPhrase,hdSeed,ownerSeed:pay.ownerSeed,peerSeed:pay.peerSeed,account:pay.account,addresses,payments},null,2)+'\n');
console.log(`Generated ${addresses.length+payments.length} allocation merge states`);
