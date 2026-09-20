import {FewestCoinSelector,UTXO} from 'quais/transaction';
import {writeFileSync} from 'node:fs';
// quais does not re-export ConversionCoinSelector from any package entry point,
// so the pinned module is loaded by path.
const {ConversionCoinSelector}=await import(new URL('../node_modules/quais/lib/esm/transaction/coinselector-conversion.js',import.meta.url));
const cases=[[[0],1,0],[[1],3,1],[[0,1,2,3],16,2],[[1,1,1,1],11,1],[[2,2,2],18,1],[[14],1,0],[[0,1],20,0],[[1,1],0,0],[[4,5,3,4],151,30]];
for(let i=0;i<60;i++){const denoms=Array.from({length:1+i%12},(_,j)=>(i*7+j*3)%10);cases.push([denoms,1+(i*113)%25000,i%17]);}
// Mixed inventories, where an aggregated conversion spend differs from the
// capped ordinary one. The first is the mainnet 15 Qi wrap that exposed it.
cases.push([[7,6,6,6,6,6,6,6,6,6,5,5],15000,0]);
cases.push([[7,6,6,6,6,6,6,6,6,6],14000,0]);
cases.push([[6,6,6,6,6,6,6,6,6,6,6,6,6,6,6],15000,0]);
cases.push([[6,6,6,6,6,6,6,6,6,6],9500,0]);
cases.push([[7,7,7],15000,0]);
cases.push([[8,6,6],12000,500]);
const VALUES=[1n,5n,10n,50n,100n,500n,1000n,5000n,10000n,20000n,100000n,1000000n,10000000n,100000000n,1000000000n];
// go-quai core/state_processor.go CheckDenominations at the pinned f3f345c:
// outputs must come from equal or larger inputs carried down, never from
// smaller ones combined. Only the first Qi transaction in a block skips it, so
// a wallet cannot rely on it. True means the node would reject this shape.
const combines=(inputs,outputs)=>{
 const have=Array(15).fill(0n),want=Array(15).fill(0n),carry=Array(15).fill(0n);
 for(const i of inputs)have[i]+=1n;
 for(const o of outputs)want[o]+=1n;
 for(let i=14;i>=1;i--){
  const total=have[i]+carry[i];
  if(want[i]>total)return true;
  carry[i-1]+=(total-want[i])*(VALUES[i]/VALUES[i-1]);
 }
 return false;
};
const select=(Selector,utxos,target,fee,denominations,checked)=>{
 let s;
 try{s=new Selector(utxos).performSelection({target:BigInt(target),fee:BigInt(fee)});}
 catch{return{error:true};}
 const result={inputs:s.inputs.map(x=>x.index),spend:s.spendOutputs.map(x=>x.denomination),change:s.changeOutputs.map(x=>x.denomination)};
 // Whether the node would reject the reference's own shape, so the Rust suite
 // asserts a deviation rather than matching a rejectable result.
 return{...result,combinesDenominations:combines(result.inputs.map(i=>denominations[i]),checked(result))};
};
const vectors=cases.map(([denominations,target,fee],index)=>{
 const utxos=()=>denominations.map((denomination,i)=>UTXO.from({txhash:'0x00800080'+'11'.repeat(28),index:i,address:'0x0088223344556677889900112233445566778899',denomination}));
 return{id:'selection-'+index,denominations,target:String(target),fee:String(fee),
  expected:select(FewestCoinSelector,utxos(),target,fee,denominations,r=>r.spend.concat(r.change)),
  // ConversionCoinSelector serves Qi->Quai conversions and wraps, whose spend
  // outputs the node aggregates into one credit, so they are not capped by the
  // input denominations and are left out of the check below.
  expectedConversion:select(ConversionCoinSelector,utxos(),target,fee,denominations,r=>r.change)};
});
writeFileSync(new URL('../fixtures/selection.json',import.meta.url),JSON.stringify({schemaVersion:2,reference:'quais@1.0.0-alpha.57',scope:'Pure fixed-fee selection; no node acceptance or spendability proof.',vectors},null,2)+'\n');
console.log(vectors.length);
