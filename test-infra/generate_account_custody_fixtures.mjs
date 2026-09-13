// Independent encoding of Rust public custody, using pinned quais.js signatures.
// PUBLIC TOY KEY ONLY. Never fund this account on any public network.
import {readFileSync,writeFileSync} from 'node:fs';
import {SigningKey,QuaiTransaction} from '../compatibility/node_modules/quais/lib/esm/index.js';
const source=JSON.parse(readFileSync(new URL('../compatibility/fixtures/transactions.json',import.meta.url))).vectors[0];
const key=new SigningKey(source.publicTestSecret), hex=s=>Buffer.from(s.replace(/^0x/,''),'hex');
const uint=(n,len)=>hex(BigInt(n).toString(16).padStart(len*2,'0'));
const blob=b=>Buffer.concat([uint(b.length,4),b]);
const root=hex(source.signed),hash=hex(source.hash);
const tx=new QuaiTransaction(source.input.from);tx.type=0;tx.chainId=9n;tx.nonce=0;tx.to=null;tx.value=0n;tx.gasLimit=21000n;tx.gasPrice=1n;tx.data='0x';tx.signature=key.sign(tx.digest);
const replacement=hex(tx.serialized),states={};
for(const [name,status] of [['empty',null],['reserved',0],['released',4],['signed',1],['submitted',2],['confirmed',3],['replacement',2],['hashOnly',1]]){
 const signed=status!==null&&status>0&&status<4,confirmed=status===3,edge=name==='replacement';
 const header=Buffer.concat([Buffer.from('QACCTBK1'),uint(9,32),Buffer.alloc(32,1),uint(0,1),hex(key.compressedPublicKey),uint(status===null?0:1,8),uint(status===null?0:1,2)]);
 const record=status===null?Buffer.alloc(0):Buffer.concat([uint(1,16),uint(0,8),uint(status,1),uint(signed?1:0,1),signed?hash:Buffer.alloc(0),uint(confirmed?1:0,1),confirmed?Buffer.concat([hash,Buffer.alloc(32,2),uint(100,32)]):Buffer.alloc(0),blob(signed&&name!=='hashOnly'?root:Buffer.alloc(0)),uint(edge?1:0,1),edge?Buffer.concat([hash,blob(replacement)]):Buffer.alloc(0)]);
 states[name]=Buffer.concat([header,record]).toString('hex');
}
writeFileSync(new URL('fixtures/account-custody.json',import.meta.url),JSON.stringify({format:'QACCTBK1',warning:'PUBLIC TOY KEY; NEVER FUND',provenance:'Independent Node encoding of Rust journal. Signatures from pinned quais@1.0.0-alpha.57. Not a quais.js storage API.',publicKey:key.compressedPublicKey,sourceId:source.id,root:source.signed,hash:source.hash,replacement:tx.serialized,replacementHash:tx.hash,states},null,2)+'\n');
console.log(`Generated ${Object.keys(states).length} account custody states`);
