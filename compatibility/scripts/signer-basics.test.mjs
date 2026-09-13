// Published signer bindings and population behavior, with public toy keys and no RPC.
import test from 'node:test';
import assert from 'node:assert/strict';
import {AbstractSigner, VoidSigner, Wallet, HDNodeWallet, HDNodeVoidWallet} from 'quais';
const key = '0x' + (805n).toString(16).padStart(64, '0');
test('local and watch-only classes inherit the reviewed provider conveniences', () => {
  for (const Class of [VoidSigner, Wallet, HDNodeWallet, HDNodeVoidWallet]) {
    for (const method of ['getNonce', 'populateCall', 'populateQuaiTransaction',
      'estimateGas', 'createAccessList', 'call', '_getAddress', 'zoneFromAddress', 'sendTransaction']) {
      assert.equal(Class.prototype[method], AbstractSigner.prototype[method], `${Class.name}.${method}`);
    }
  }
});
test('published population replaces explicit nonce zero with pending nonce', async () => {
  const requests = [];
  const provider = {
    getNetwork: async () => ({chainId:9n}),
    getTransactionCount: async (address, block) => {requests.push([address,block]); return 5;},
  };
  const wallet = new Wallet(key, provider);
  const tx = {from:wallet.address,to:wallet.address,nonce:0,chainId:9n,
    gasLimit:21000n,gasPrice:2n,value:7n,data:'0x',accessList:[]};
  assert.equal((await wallet.populateQuaiTransaction(tx)).nonce,5);
  assert.equal(tx.nonce,0);
  assert.deepEqual(requests,[[wallet.address,'pending']]);
  await assert.rejects(wallet.populateQuaiTransaction({...tx,chainId:10n}),
    error=>error.code==='INVALID_ARGUMENT');
});
test('published local signing rejects a mismatched sender and void signing rejects', async () => {
  const wallet = new Wallet(key);
  const tx = {from:'0x0000000000000000000000000000000000000001',to:wallet.address,
    nonce:0,chainId:9n,gasLimit:21000n,gasPrice:2n,value:7n};
  await assert.rejects(wallet.signTransaction(tx),error=>error.code==='INVALID_ARGUMENT');
  const watch = new VoidSigner(wallet.address);
  for (const [method,args] of [['signTransaction',[tx]],['signMessage',['test']],['signTypedData',[{}, {}, {}]]]) {
    await assert.rejects(watch[method](...args),error=>error.code==='UNSUPPORTED_OPERATION');
  }
});
