// Source response semantics; fixtures are public and no provider performs network I/O.
import test from 'node:test';
import assert from 'node:assert/strict';
import {Block, TransactionReceipt, JsonRpcApiProvider, ContractTransactionReceipt, Interface, EventLog, UndecodedEventLog, Log} from 'quais';
const hash=n=>'0x'+n.toString(16).padStart(64,'0');
function block() {
 const transaction=n=>({hash:hash(n),from:'0x0011223344556677889900112233445566778899',nonce:0,gasLimit:21000n,gasPrice:2n,value:1n,data:'0x',accessList:[]});
 const ext=n=>({...transaction(n),originatingTxHash:hash(99),etxIndex:0,etxType:1});
 return new Block({hash:hash(88),header:{},woHeader:{number:16,timestamp:'0x10'},transactions:[transaction(1),transaction(2)],outboundEtxs:[ext(3),ext(4)],uncles:[],workShares:[],subManifest:[],interlinkHashes:[],size:1n,totalEntropy:2n},{});
}
test('published prefetched async hash lookup skips matching transaction for both lists',async()=>{
 const b=block();assert.equal(b.getPrefetchedTransaction(hash(1)).hash,hash(1));
 assert.equal((await b.getTransaction(hash(1))).hash,hash(2));
 assert.equal((await b.getExtTransaction(hash(3))).hash,hash(4));
 assert.equal((await b.getTransaction(hash(999))).hash,hash(1));
 assert.deepEqual([...b],[hash(1),hash(2)]);assert.equal(b.date.getTime(),16000);
 assert.equal(b.toJSON().transactions[0],hash(1));assert.equal(b.prefetchedExtTransactions.length,2);
});
test('receipt fee/iteration/status are data while confirmations only subtract reported heights',async()=>{
 const r=new TransactionReceipt({hash:hash(1),blockHash:hash(88),blockNumber:16,index:0,from:'0x0011223344556677889900112233445566778899',to:null,contractAddress:null,gasUsed:21000n,cumulativeGasUsed:21000n,effectiveGasPrice:2n,status:2,type:0,logs:[],logsBloom:'0x'}, {getBlockNumber:async()=>15});
 assert.equal(r.fee,42000n);assert.deepEqual([...r],[]);assert.equal(r.status,2);assert.equal(await r.confirmations(),0);
 const doc=r.toJSON();assert.equal(doc.gasUsed,'21000');assert.equal(doc.gasPrice,'2');
});
test('published JSON RPC backend has no transaction-result action despite receipt helper',async()=>{
 const p=Object.create(JsonRpcApiProvider.prototype);
 assert.equal(p.getRpcRequest({method:'getTransactionResult'}),null);
 await assert.rejects(JsonRpcApiProvider.prototype._perform.call(p,{method:'getTransactionResult',hash:hash(1),zone:'0x00'}),e=>e.code==='UNSUPPORTED_OPERATION');
});

test('contract receipts retain unknown logs and decoding errors while matching every emitter',()=>{
 const iface=new Interface(['event Tagged(uint256 amount)']);
 const encoded=iface.encodeEventLog(iface.getEvent('Tagged'),[7n]);
 const base={address:'0x0011223344556677889900112233445566778899',topics:encoded.topics,data:encoded.data,transactionHash:hash(1),blockHash:hash(88),blockNumber:16,transactionIndex:0,index:0,removed:false};
 const r=new ContractTransactionReceipt(iface,{}, {hash:hash(1),blockHash:hash(88),blockNumber:16,index:0,gasUsed:1n,effectiveGasPrice:2n,logs:[base,{...base,index:1,topics:[]},{...base,index:2,data:'0x01'},{...base,index:3,address:'0x0011223344556677889900112233445566778800'}]});
 assert.ok(r.logs[0] instanceof EventLog);assert.equal(r.logs[0].args[0],7n);
 assert.equal(r.logs[1].constructor,Log);assert.ok(r.logs[2] instanceof UndecodedEventLog);assert.ok(r.logs[3] instanceof EventLog);
});
