import {AggregateCoinSelector,UTXO,denominations} from 'quais/transaction';
import {writeFileSync} from 'node:fs';
const cases=[[[1,1,1],0,1,14],[[1,1,1,7,8],1,6,14],[[1,1,1,7,8],7,6,14],[[1,1,1],6,6,14],[[1],1,6,14],[[1,1,1],15,6,14],[[7,8],0,6,14],[[0,0,0,0,0,1],0,0,0],[[3,3,3,3,3,7],1100,6,14]];
for(let i=0;i<120;i++)cases.push([Array.from({length:1+i%18},(_,j)=>(i*7+j*3)%8),(i*53)%3000,i%7,14]);
const previous=console.warn;console.warn=()=>{};
const vectors=cases.map(([denoms,fee,maximumInput,maximumOutput],id)=>{
 const input=denoms.map((denomination,i)=>UTXO.from({txhash:'0x00800080'+'11'.repeat(28),index:i,address:'0x0088223344556677889900112233445566778899',denomination}));
 let expected;
 try{
  const s=new AggregateCoinSelector(input).performSelection({fee:BigInt(fee),maxDenominationAggregate:maximumInput,maxDenominationOutput:maximumOutput});
  const actualFee=s.inputs.reduce((sum,x)=>sum+denominations[x.denomination],0n)-s.spendOutputs.reduce((sum,x)=>sum+denominations[x.denomination],0n);
  expected={inputs:s.inputs.map(x=>x.index),spend:s.spendOutputs.map(x=>x.denomination),change:s.changeOutputs.map(x=>x.denomination),actualFee:String(actualFee)};
 }catch{expected={error:true};}
 return{id,denominations:denoms,fee:String(fee),maximumInput,maximumOutput,expected};
});
console.warn=previous;
writeFileSync(new URL('../fixtures/aggregation.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',vectors},null,2)+'\n');
console.log(JSON.stringify({vectors:vectors.length,underpaid:vectors.filter(v=>!v.expected.error&&v.expected.actualFee!==v.fee).map(v=>v.id)}));
