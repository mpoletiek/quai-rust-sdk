// Offline calls on prototype methods only: never constructs/connects a provider.
import {AbstractProvider,BrowserProvider,JsonRpcApiProvider,JsonRpcProvider,SocketProvider,WebSocketProvider} from 'quais';
import {writeFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const provider=Object.create(JsonRpcApiProvider.prototype);
const actions=[{method:'chainId'},{method:'getBlockNumber'},{method:'getGasPrice',zone:'0x00',txType:true},{method:'getRunningLocations'},
 {method:'estimateFeeForQi',transaction:{txType:2,txIn:[],txOut:[]}},
 {method:'broadcastTransaction',signedTransaction:'0x00'}];
const rpcActions=actions.map(action=>({action,rpc:provider.getRpcRequest(action)}));
const unsupported=[];
for(const [method,run,expected] of [
 ['waitForBlock',()=>AbstractProvider.prototype.waitForBlock.call({},'0x00','latest'),'NOT_IMPLEMENTED'],
 ['getTransactionResult',()=>JsonRpcApiProvider.prototype._perform.call(provider,{method:'getTransactionResult',hash:'0x'+'00'.repeat(32),zone:'0x00'}),'UNSUPPORTED_OPERATION'],
]){
 let result;
 try {await run();throw new Error('unexpected reference implementation');}
 catch(error){assert.equal(error.code,expected);assert.equal(error.operation,method);result={method,code:error.code,operation:error.operation};}
 unsupported.push(result);
}
assert.equal(provider.getRpcRequest({method:'getTransactionResult'}),null);
const methods=['getBlockNumber','getFeeData','getNetwork','getRunningLocations','estimateFeeForQi','broadcastTransaction','zoneFromAddress','_getAddress','_getBlockTag','validateUrl','waitForBlock','getTransactionResult'];
const inheritance=[];
for(const Class of [AbstractProvider,BrowserProvider,JsonRpcApiProvider,JsonRpcProvider,SocketProvider,WebSocketProvider]){
 for(const method of methods){
  let prototype=Class.prototype;
  while(prototype&&!Object.hasOwn(prototype,method))prototype=Object.getPrototypeOf(prototype);
  assert.ok(prototype&&typeof prototype[method]==='function');
  inheritance.push({class:Class.name,method,definedOn:prototype.constructor.name});
 }
}
writeFileSync(new URL('../fixtures/provider-basics.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',networkAccess:false,note:'RPC mapping and unsupported-action results from pinned prototypes without provider initialization. Empty Qi and 0x00 broadcast are mapping-only sentinels, never valid transactions or submitted payloads.',rpcActions,unsupported,inheritance},null,2)+'\n');
console.log(`Generated ${rpcActions.length} RPC mappings, ${unsupported.length} unsupported actions and ${inheritance.length} inheritance bindings`);
