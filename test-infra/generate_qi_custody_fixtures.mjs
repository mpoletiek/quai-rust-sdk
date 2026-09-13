// Independent encoder of Rust custody with pinned reference signatures. PUBLIC TOY KEYS.
import {readFileSync,writeFileSync} from 'node:fs';
import {computeAddress} from '../compatibility/node_modules/quais/lib/esm/index.js';
const load=p=>JSON.parse(readFileSync(new URL(p,import.meta.url))), hex=s=>Buffer.from(s.replace(/^0x/,''),'hex');
const uint=(n,len)=>hex(BigInt(n).toString(16).padStart(len*2,'0')), blob=b=>Buffer.concat([uint(b.length,4),b]);
const transactions=load('../compatibility/fixtures/transactions.json').vectors;
const roots=[transactions.find(v=>v.id==='qi-0'),transactions.find(v=>v.id==='qi-multi-0'),load('../crates/quai-consensus/tests/conversion-vectors.json').vectors.find(v=>v.id==='conversion-qi-0'),load('../crates/quai-consensus/tests/wrapping-vectors.json').vectors[0]];
const vectors=[];
for(const root of roots){
 const owners=[...new Map(root.input.txInputs.map(i=>[computeAddress(i.pubkey).toLowerCase(),i.pubkey])).entries()].sort(([a],[b])=>a.localeCompare(b));
 const claims=root.input.txInputs.map(i=>({hash:i.txhash,index:i.index,owner:computeAddress(i.pubkey)})).sort((a,b)=>a.hash.localeCompare(b.hash)||a.index-b.index);
 const states={};
 for(const [name,status] of [['empty',null],['reserved',0],['released',4],['signed',1],['submitted',2],['confirmed',3],['hashOnly',1]]){
  const signed=status!==null&&status>0&&status<4,active=status!==null&&status!==4,confirmed=status===3;
  const metadata=status===null?Buffer.alloc(0):Buffer.concat(owners.map(([,key])=>blob(Buffer.concat([Buffer.from('QADDR001'),hex(key),uint(0,1)]))));
  const header=Buffer.concat([Buffer.from('QQICUBK1'),uint(15000,32),Buffer.alloc(32,1),uint(0,1),Buffer.alloc(32,7),uint(status===null?0:owners.length,2),metadata,uint(status===null?0:1,2)]);
  const record=status===null?Buffer.alloc(0):Buffer.concat([uint(1,16),uint(status,1),uint(1,1),Buffer.alloc(32,2),uint(10,32),uint(active?claims.length:0,2),active?Buffer.concat(claims.map(c=>Buffer.concat([hex(c.hash),uint(c.index,2),hex(c.owner)]))):Buffer.alloc(0),uint(signed?1:0,1),signed?hex(root.hash):Buffer.alloc(0),uint(confirmed?1:0,1),confirmed?Buffer.concat([hex(root.hash),Buffer.alloc(32,3),uint(11,32)]):Buffer.alloc(0),blob(signed&&name!=='hashOnly'?hex(root.signed):Buffer.alloc(0)),uint(0,1)]);
  states[name]=Buffer.concat([header,record]).toString('hex');
 }
 vectors.push({id:root.id,publicTestSecrets:root.publicTestSecrets??[root.publicTestSecret],signed:root.signed,hash:root.hash,states});
}
writeFileSync(new URL('fixtures/qi-custody.json',import.meta.url),JSON.stringify({format:'QQICUBK1',warning:'PUBLIC TOY KEYS; NEVER FUND',provenance:'Independent Node encoding of Rust journal, not a quais.js storage format. Uses pinned quais@1.0.0-alpha.57 transaction/conversion/wrapping fixtures. Synthetic source coins are not spendability evidence.',vectors},null,2)+'\n');
console.log(`Generated ${vectors.length*7} Qi custody states`);
