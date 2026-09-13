import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {AbstractProvider,JsonRpcApiProvider} from 'quais';
const fixture=JSON.parse(readFileSync(new URL('../fixtures/provider-basics.json',import.meta.url)));
test('basic RPC mappings remain equal to pinned source without initialization',()=>{
 const provider=Object.create(JsonRpcApiProvider.prototype);
 assert.equal(fixture.networkAccess,false);
 for(const row of fixture.rpcActions)assert.deepEqual(provider.getRpcRequest(row.action),row.rpc);
 assert.equal(provider.getRpcRequest({method:'getTransactionResult'}),null);
});
test('pinned placeholders stay explicit unsupported operations',async()=>{
 const provider=Object.create(JsonRpcApiProvider.prototype);
 for(const row of fixture.unsupported){
  const run=row.method==='waitForBlock'?()=>AbstractProvider.prototype.waitForBlock.call({},'0x00','latest'):
   ()=>JsonRpcApiProvider.prototype._perform.call(provider,{method:row.method,hash:'0x'+'00'.repeat(32),zone:'0x00'});
  await assert.rejects(run,error=>error.code===row.code&&error.operation===row.operation);
 }
});
