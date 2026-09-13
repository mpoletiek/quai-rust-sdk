// Published provider lifecycle semantics using local objects only, never live endpoints.
import test from 'node:test';
import assert from 'node:assert/strict';
import {AbstractProvider,JsonRpcApiProvider,JsonRpcProvider,BrowserProvider,SocketProvider,WebSocketProvider,SocketSubscriber} from 'quais';
const addresses=['0x0011223344556677889900112233445566778899','0x0011223344556677889900112233445566778800'];
test('account discovery preserves order and remote signer lookup checks listed address',async()=>{
 const p=Object.create(JsonRpcApiProvider.prototype);const calls=[];
 p.send=async(method,params)=>{calls.push([method,params]);return addresses;};p.getNetwork=async()=>({chainId:9n});
 assert.deepEqual((await p.listAccounts()).map(s=>s.address.toLowerCase()),addresses);
 assert.equal((await p.getSigner(1)).address.toLowerCase(),addresses[1]);
 assert.equal((await p.getSigner(addresses[0])).address.toLowerCase(),addresses[0]);
 await assert.rejects(p.getSigner(2),/no such account/);
 await assert.rejects(p.getSigner('0x0000000000000000000000000000000000000001'),/invalid account/);
 assert.ok(calls.every(([m,args])=>m==='quai_accounts'&&args.length===0));
 assert.equal(await BrowserProvider.prototype.hasSigner.call(p,0),true);
 assert.equal(await BrowserProvider.prototype.hasSigner.call(p,2),false);
 // Published integer lookup does not reject a negative index; Rust requires an explicit address.
 assert.equal(await BrowserProvider.prototype.hasSigner.call(p,-1),true);
});
test('local unmanaged event listeners are ordered and once removes itself after emission',async()=>{
 const p=new AbstractProvider();const received=[];
 const a=v=>received.push(['a',v]);const b=v=>received.push(['b',v]);
 await p.on('debug',a);await p.once('debug',b);assert.equal(await p.listenerCount('debug'),2);
 assert.deepEqual(await p.listeners('debug'),[a,b]);
 assert.equal(await p.emit('debug',undefined,1),true);assert.equal(await p.listenerCount('debug'),1);
 await p.emit('debug',undefined,2);assert.deepEqual(received,[['a',1],['b',1],['a',2]]);
 await p.off('debug',a);assert.equal(await p.emit('debug',undefined,3),false);
 p.pause(false);assert.equal(p.paused,true);assert.throws(()=>p.pause(true),e=>e.code==='UNSUPPORTED_OPERATION');p.resume();assert.equal(p.paused,false);
 await p.removeAllListeners();p.destroy();assert.equal(p.destroyed,true);
});
test('published socket subscribers reject buffering pause and return copied filters',()=>{
 const s=new SocketSubscriber({},['newHeads'],'0x00');const copy=s.filter;copy[0]='changed';assert.deepEqual(s.filter,['newHeads']);
 assert.throws(()=>s.pause(false),e=>e.code==='UNSUPPORTED_OPERATION');s.pause(true);s.resume();
});
test('provider families share reviewed reads, normalization and local event behavior',()=>{
 for(const Class of [JsonRpcApiProvider,JsonRpcProvider,BrowserProvider,SocketProvider,WebSocketProvider]){
  for(const name of ['calculateConversionAmount','getBlock','getOutpointDeltas','getOutpointsByAddress','getPendingHeader','_wrapBlock','_wrapLog','_wrapTransactionReceipt','_wrapTransactionResponse','on','once','off','emit','listeners','listenerCount','pause','resume']){
   assert.equal(Class.prototype[name],AbstractProvider.prototype[name],`${Class.name}.${name}`);
  }
 }
});
