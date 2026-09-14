import test from 'node:test';import assert from 'node:assert/strict';
import {getBigInt,getNumber,toBeArray,toBeHex,toQuantity,isHexString,isBytesLike,getBytes,makeError,isError,isCallException,isAddressable,copyRequest,checkResultErrors,AbiCoder,lock,sha256} from 'quais';
import {assertArgumentCount,defineProperties,resolveProperties} from 'quais/utils';
test('numeric grammar and zero byte/quantity forms differ deliberately',()=>{
 assert.equal(getBigInt('-'),0n);assert.equal(getBigInt(' '),0n);assert.equal(getBigInt('+1'),1n);assert.equal(getBigInt('-0xff'),-255n);assert.throws(()=>getNumber('-0xff'));
 assert.equal(toBeArray(0n).length,0);assert.equal(toBeHex(0n),'0x00');assert.equal(toQuantity(0n),'0x0');assert.throws(()=>toBeHex(0n,0));
 assert.equal(getBigInt(1n<<512n),1n<<512n);assert.throws(()=>getNumber(1n<<53n));
});
test('hex predicates distinguish digits, byte shape and getBytes uppercase-prefix coercion',()=>{
 assert(isHexString('0x0'));assert(!isBytesLike('0x0'));assert(isBytesLike('0x'));
 assert(!isHexString('0Xab'));assert.deepEqual(getBytes('0Xab'),Uint8Array.of(171));
});
test('error guards inspect code only; typed Rust variants do not trust arbitrary shape',()=>{
 assert(isCallException({code:'CALL_EXCEPTION'}));assert(isError({code:'TIMEOUT'},'TIMEOUT'));
 const e=makeError('public','INVALID_ARGUMENT',{argument:'value',value:'PUBLIC'});
 assert(e instanceof TypeError);assert.equal(e.argument,'value');assert(e.message.includes('PUBLIC'));
 assert.throws(()=>assertArgumentCount(0,1),e=>e.code==='MISSING_ARGUMENT');
 assert.throws(()=>assertArgumentCount(2,1),e=>e.code==='UNEXPECTED_ARGUMENT');
});
test('property/Addressable helpers are JS runtime structure; values can be awaited explicitly',async()=>{
 const value={};defineProperties(value,{x:1});assert.throws(()=>{value.x=2;},TypeError);
 assert.deepEqual(await resolveProperties({x:Promise.resolve(1),y:2}),{x:1,y:2});
 assert(isAddressable({getAddress(){return 'not actually an address';}}));
});
test('copyRequest coerces and drops unknown fields without deeply cloning arbitrary custom data',()=>{
 const customData={public:1};const request={nonce:'7',value:'12',data:'0Xab',customData,unknown:'dropped'};
 const copied=copyRequest(request);assert.equal(copied.nonce,7);assert.equal(copied.value,12n);assert.equal(copied.data,'0xab');assert(!('unknown'in copied));assert.equal(copied.customData,customData);
});
test('deferred ABI failures are reported by path whereas Rust results eagerly validate',()=>{
 const result=AbiCoder.defaultAbiCoder().decode(['string'],'0x'+'00'.repeat(31)+'20'+'00'.repeat(31)+'01'+'ff'+'00'.repeat(31));
 const errors=checkResultErrors(result);assert.equal(errors.length,1);assert.deepEqual(errors[0].path,['0']);
});
test('crypto backend lock blocks subsequent global registration',()=>{
 lock();assert.throws(()=>sha256.register(()=>new Uint8Array(32)));
});
