// Independent encoder of the documented Rust-only journal, not a quais.js format.
// Derivation/address results come from pinned quais.js fixtures. PUBLIC TOY SEEDS.
import {readFileSync,writeFileSync} from 'node:fs';
import {keccak256} from '../compatibility/node_modules/quais/lib/esm/index.js';
const source=JSON.parse(readFileSync(new URL('../crates/quai-payments/tests/fixtures/quais-payments.json',import.meta.url)));
const u32=n=>{const b=Buffer.alloc(4);b.writeUInt32BE(n);return b;};
const pair=source.pairs[0], vectors=[];
for(const row of source.searches){
 const identity=Buffer.from(keccak256(Buffer.concat([Buffer.from('quai-rust/payment-allocation/v1'),Buffer.from(pair.senderPayload.slice(2),'hex'),u32(pair.account),Buffer.from(pair.receiverPayload.slice(2),'hex'),Buffer.from([row.direction==='send'?0:1])])).slice(2),'hex');
 const network=Buffer.concat([Buffer.from(BigInt(15000).toString(16).padStart(64,'0'),'hex'),Buffer.alloc(32,1),Buffer.from([row.zone])]);
 const states={};
 for(const [name,status] of [['empty',null],['pending',0],['completed',1],['abandoned',2]]){
  const header=Buffer.concat([Buffer.from('QPAYABK1'),network,identity,u32(0),u32(status===null?0:row.index+1),Buffer.from([0,status===null?0:1])]);
  const id=Buffer.alloc(16);id[15]=1;
  const record=status===null?Buffer.alloc(0):Buffer.concat([id,u32(0),u32(row.index+1),Buffer.from([status]),status===1?u32(row.index):Buffer.alloc(0)]);
  states[name]=Buffer.concat([header,record]).toString('hex');
 }
 vectors.push({...row,states});
}
writeFileSync(new URL('fixtures/payment-allocation.json',import.meta.url),JSON.stringify({format:'QPAYABK1',warning:'PUBLIC TOY SEEDS; NEVER FUND',provenance:'Independent Node encoding of Rust journal format using pinned quais@1.0.0-alpha.57 payment derivation fixtures. Not a quais.js journal API or storage format.',ownerSeed:pair.senderSeed,peerSeed:pair.receiverSeed,account:pair.account,ownerCode:pair.senderCode,peerCode:pair.receiverCode,vectors},null,2)+'\n');
console.log(`Generated ${vectors.length*4} payment allocation states`);
