import {FewestCoinSelector,UTXO} from 'quais/transaction';
import {writeFileSync} from 'node:fs';
const cases=[[[0],1,0],[[1],3,1],[[0,1,2,3],16,2],[[1,1,1,1],11,1],[[2,2,2],18,1],[[14],1,0],[[0,1],20,0],[[1,1],0,0],[[4,5,3,4],151,30]];
for(let i=0;i<60;i++){const denoms=Array.from({length:1+i%12},(_,j)=>(i*7+j*3)%10);cases.push([denoms,1+(i*113)%25000,i%17]);}
const vectors=cases.map(([denominations,target,fee],index)=>{
 const input=denominations.map((denomination,i)=>UTXO.from({txhash:'0x00800080'+'11'.repeat(28),index:i,address:'0x0088223344556677889900112233445566778899',denomination}));
 let expected;try{const s=new FewestCoinSelector(input).performSelection({target:BigInt(target),fee:BigInt(fee)});expected={inputs:s.inputs.map(x=>x.index),spend:s.spendOutputs.map(x=>x.denomination),change:s.changeOutputs.map(x=>x.denomination)};}catch{expected={error:true};}
 return{id:'selection-'+index,denominations,target:String(target),fee:String(fee),expected};
});
writeFileSync(new URL('../fixtures/selection.json',import.meta.url),JSON.stringify({schemaVersion:1,reference:'quais@1.0.0-alpha.57',scope:'Pure fixed-fee selection; no node acceptance or spendability proof.',vectors},null,2)+'\n');
console.log(vectors.length);
