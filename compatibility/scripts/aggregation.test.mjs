import test from 'node:test';
import assert from 'node:assert/strict';
import {AggregateCoinSelector,UTXO,denominations} from 'quais/transaction';
const coins=ds=>ds.map((denomination,index)=>UTXO.from({txhash:'0x00800080'+'11'.repeat(28),index,address:'0x0088223344556677889900112233445566778899',denomination}));
const select=(ds,config)=>{const warn=console.warn;console.warn=()=>{};try{return new AggregateCoinSelector(coins(ds)).performSelection(config);}finally{console.warn=warn;}};
test('threshold preserves large coins without a fee and puts fee inputs first',()=>{
 const s=select([1,1,1,7,8],{maxDenominationAggregate:1,fee:0n});
 assert.deepEqual(s.inputs.map(x=>x.index),[0,1,2]);assert.deepEqual(s.spendOutputs.map(x=>x.denomination),[2,1]);
 const paid=select([1,1,1,7,8],{maxDenominationAggregate:1,fee:1n});
 assert.deepEqual(paid.inputs.map(x=>x.index),[3,0,1,2]);assert.deepEqual(paid.changeOutputs,[]);
});
test('reference underpays a requested six-Qit fee with three five-Qit coins',()=>{
 const s=select([1,1,1],{fee:6n});
 const sum=xs=>xs.reduce((n,x)=>n+denominations[x.denomination],0n);
 assert.equal(sum(s.inputs)-sum(s.spendOutputs),5n);
 assert.deepEqual(s.inputs.map(x=>x.index),[2,0,1]);
});
test('non-reducing reference output is warning-only and empty eligibility fails',()=>{
 const s=select([1],{fee:0n});assert.equal(s.inputs.length,s.spendOutputs.length);
 assert.throws(()=>select([7,8],{maxDenominationAggregate:6}));
 assert.throws(()=>select([1,1,1],{fee:15n}));
});
