// Independent encoder for the documented Rust allocation journal; public data only.
// quais supplies Keccak; account/address pairs come from the pinned HD fixture.
// There is no claim that quais.js implements this journal format.
import {readFileSync,writeFileSync} from 'node:fs';
import {keccak256} from '../compatibility/node_modules/quais/lib/esm/index.js';
const source=JSON.parse(readFileSync(new URL('../crates/quai-wallet/tests/reference.json',import.meta.url)));
const u32=n=>{const b=Buffer.alloc(4);b.writeUInt32BE(n);return b;};
const vectors=[];
for(const row of source.grinding){
 const coin=row.coin,account=row.account,zone=Number(row.zone),index=row.index,change=row.change,accountXpub=row.accountXpub;
 const identity=Buffer.from(keccak256(Buffer.concat([Buffer.from('quai-rust/address-allocation/v1'),u32(coin),u32(account),Buffer.from(accountXpub)])).slice(2),'hex');
 const network=Buffer.concat([Buffer.from(BigInt(15000).toString(16).padStart(64,'0'),'hex'),Buffer.alloc(32,1),Buffer.from([zone])]);
 const states={};
 for(const [name,status] of [['empty',null],['pending',0],['completed',1],['abandoned',2]]){
  const next=[0,0];if(status!==null)next[Number(change)]=index+1;
  const header=Buffer.concat([Buffer.from('QADDRBK1'),network,identity,u32(0),u32(0),u32(next[0]),u32(next[1]),Buffer.from([0,status===null?0:1])]);
  const id=Buffer.alloc(16);id[15]=1;
  const record=status===null?Buffer.alloc(0):Buffer.concat([id,Buffer.from([Number(change)]),u32(0),u32(index+1),Buffer.from([status]),status===1?u32(index):Buffer.alloc(0)]);
  states[name]=Buffer.concat([header,record]).toString('hex');
 }
 vectors.push({coin,account,accountXpub,zone,index,change,address:row.address,states});
}
writeFileSync(new URL('fixtures/address-allocation.json',import.meta.url),JSON.stringify({format:'QADDRBK1',publicDataOnly:true,provenance:'Independent Node encoding of the documented Rust journal. Pinned quais@1.0.0-alpha.57 HD fixture account/address pairs and Keccak. Not a quais.js journal API or storage-format compatibility claim.',vectors},null,2)+'\n');
console.log(`Generated ${vectors.length*4} allocation-journal states`);
