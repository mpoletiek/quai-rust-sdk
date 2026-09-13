// Published JsonRpcSigner request semantics. No network, funded keys or wallet prompts.
import test from 'node:test';
import assert from 'node:assert/strict';
import {JsonRpcSigner, Wallet, TypedDataEncoder} from 'quais';
const address = new Wallet('0x'+(805n).toString(16).padStart(64,'0')).address;
const lower = address.toLowerCase();
function setup() {
  const calls=[];
  const provider={send:async(method,params)=>{calls.push([method,params]);return 'unverified';},getRpcTransaction:tx=>tx};
  return {calls,provider,signer:new JsonRpcSigner(provider,address)};
}
test('published RPC signer forwards exact message, typed and legacy parameter order',async()=>{
  const {signer,calls}=setup();
  assert.equal(await signer.signMessage('hello'),'unverified');
  await signer._legacySignMessage('hello');
  const domain={name:'Public toy',chainId:9};
  const types={Transfer:[{name:'amount',type:'uint256'}]};
  const value={amount:'9007199254740993'};
  await signer.signTypedData(domain,types,value);
  assert.deepEqual(calls,[['personal_sign',['0x68656c6c6f',lower]],['quai_sign',[lower,'0x68656c6c6f']],['quai_signTypedData_v4',[lower,JSON.stringify(TypedDataEncoder.getPayload(domain,types,value))]]]);
});
test('published unlock uses null duration and signer binding cannot reconnect',async()=>{
  const {signer,provider,calls}=setup();
  await signer.unlock('PUBLIC');
  assert.deepEqual(calls,[['personal_unlockAccount',[lower,'PUBLIC',null]]]);
  assert.equal(await signer.getAddress(),address);
  assert.equal(signer.provider,provider);
  assert.throws(()=>signer.connect(provider),e=>e.code==='UNSUPPORTED_OPERATION');
});
test('published transaction signing retains zero nonce but does not verify signed replies',async()=>{
  const {signer,calls}=setup();
  const tx={from:address,to:address,nonce:0,chainId:9n,value:7n,gasLimit:25000n,gasPrice:2n,data:'0x',accessList:[]};
  assert.equal(await signer.signTransaction(tx),'unverified');
  assert.equal(calls[0][0],'quai_signTransaction');assert.equal(calls[0][1][0].nonce,0);
  await assert.rejects(signer.signTransaction({...tx,from:'0x0000000000000000000000000000000000000001'}),e=>e.code==='INVALID_ARGUMENT');
  const {from,...missing}=tx;
  await assert.rejects(signer.signTransaction(missing),/No QI signing implementation/);
});
