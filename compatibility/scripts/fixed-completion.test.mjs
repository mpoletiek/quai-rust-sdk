import test from 'node:test';import assert from 'node:assert/strict';import {FixedNumber} from 'quais';
test('reference negative-result wrapping can violate the declared signed range',()=>{
 assert.equal(FixedNumber.fromString('-128','fixed8x0').value,-128n);
 const a=FixedNumber.fromString('-127','fixed8x0'),b=FixedNumber.fromString('-1','fixed8x0');
 assert.equal(a.addUnsafe(b).value,128n);assert.equal(BigInt.asIntN(8,-128n),-128n);
});
test('overflow wraps in unsafe arithmetic while explicit checked methods fail',()=>{
 const a=FixedNumber.fromString('255','ufixed8x0'),b=FixedNumber.fromString('1','ufixed8x0');
 assert.equal(a.addUnsafe(b).value,0n);assert.throws(()=>a.add(b));
 assert.equal(b.subUnsafe(a).value,2n);assert.throws(()=>b.sub(a));
 const min=FixedNumber.fromString('127','fixed8x0').addUnsafe(FixedNumber.fromString('1','fixed8x0'));
 assert.equal(min.value,-128n);assert.equal(min.divUnsafe(FixedNumber.fromString('-1','fixed8x0')).value,-128n);
});
test('lossy floats are explicit and incompatible formats or zero divisors still fail',()=>{
 const a=FixedNumber.fromString('9007199254740993','fixed128x0');assert.equal(a.toUnsafeFloat(),9007199254740992);
 const b=FixedNumber.fromString('1','fixed128x18');assert.throws(()=>a.addUnsafe(b));
 assert.throws(()=>a.divUnsafe(FixedNumber.fromString('0','fixed128x0')));
});
