// Public Unicode/UUID/hash reference observations; never reads wallets or nodes.
import {toUtf8Bytes,toUtf8CodePoints,toUtf8String,uuidV4,ripemd160,id,hexlify} from 'quais';
import {writeFileSync} from 'node:fs';
const texts=[];
for(const text of ['', 'Qi Quai', 'é','e\u0301','Å','A\u030a','ﬃ','①','ＡＢＣ','日本語 🍊','가','가','a\u0315\u0300','\u0000\ufffd\u{10ffff}','ﷺ']){
 for(const form of [null,'NFC','NFD','NFKC','NFKD'])texts.push({text,form,bytes:hexlify(toUtf8Bytes(text,form??undefined)),points:toUtf8CodePoints(text,form??undefined)});
}
const decoding=[];
for(const hex of ['0x','0x00','0x616263','0xc3a9','0xf09f8d8a','0x80','0xff','0xc080','0xc1bf','0xe08080','0xf0808080','0xeda080','0xedb080','0xf4908080','0xf888808080','0xc2','0xe282','0xf09f8d','0xc220','0xe24180','0x61628063']){
 const row={hex};try{row.text=toUtf8String(hex);}catch{row.error=true;}decoding.push(row);
}
const surrogates=[[0xd800],[0xdc00],[0x61,0xdc00,0x62],[0xd800,0x61],[0xd83c,0xdf4a]].map(units=>{const row={units};try{row.bytes=hexlify(toUtf8Bytes(String.fromCharCode(...units)));}catch{row.error=true;}return row;});
const uuids=[];
for(const bytes of [new Uint8Array(16),new Uint8Array(16).fill(255),Uint8Array.from({length:16},(_,i)=>i),Uint8Array.from({length:16},(_,i)=>(i*17)^0xa5)]){
 const input=hexlify(bytes);const uuid=uuidV4(bytes);uuids.push({input,uuid,referenceMutatedInput:hexlify(bytes)});
}
const hashes=[];
for(const hex of ['0x','0x00','0x616263','0x000102ff',...texts.filter(v=>v.form==null).map(v=>v.bytes)])hashes.push({hex,ripemd160:ripemd160(hex)});
const ids=texts.filter(v=>v.form==null).map(v=>({text:v.text,id:id(v.text)}));
writeFileSync(new URL('../fixtures/text-crypto.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',normalizationNote:'Known stable normalization cases; Rust exposes its normalization table version independently of JS engine.',note:'Rust strings exclude unpaired UTF-16 surrogates; strict decoding rejects ill-formed UTF-8. UUID input has an exact 16-byte type and is not mutated.',texts,decoding,surrogates,uuids,hashes,ids},null,2)+'\n');
console.log(JSON.stringify({texts:texts.length,decoding:decoding.length,surrogates:surrogates.length,uuids:uuids.length,hashes:hashes.length,ids:ids.length}));
