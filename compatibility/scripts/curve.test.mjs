import test from 'node:test';
import assert from 'node:assert/strict';
import {musigCrypto as c,hexlify} from 'quais';
import {createHash} from 'node:crypto';
test('scalar boundaries and U256 writes are explicitly different from arbitrary JS integers',()=>{
 const n=0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n;
 assert(c.isScalar(c.write32b(0n)));assert(!c.isSecret(c.write32b(0n)));
 assert(!c.isScalar(c.write32b(n)));assert.throws(()=>c.read32b(new Uint8Array(31)));
 assert.equal(c.read32b(c.write32b(1n<<256n)),0n);
 assert.equal(c.read32b(c.scalarAdd(c.write32b(n-1n),c.write32b(1n))),0n);
});
test('source liftX expects SEC1; structural point helpers do not validate curve membership',()=>{
 const point=c.getPublicKey(c.write32b(1n),true);
 assert.equal(c.liftX(point.subarray(1)),null);
 assert.deepEqual(c.liftX(point),c.pointCompress(point,false));
 const bad=new Uint8Array(33);bad[0]=2;
 assert.equal(c.isPoint(bad),false);assert.equal(c.hasEvenY(bad),true);
 assert.equal(c.pointNegate(bad)[0],3);
 assert.deepEqual(c.pointX(new Uint8Array(32)),new Uint8Array(32));
 assert.equal(c.pointMultiplyUnsafe(point,c.write32b(0n),true),null);
 assert.equal(c.pointAdd(point,c.pointNegate(point),true),null);
});
test('multipart hashing is SHA256; non-ASCII tags use truncated first UTF16 units',()=>{
 const tag='PUBLIC π',parts=[Uint8Array.of(0,1),Uint8Array.of(2)];
 const hash=(...p)=>createHash('sha256').update(Buffer.concat(p)).digest();
 const t=hash(Uint8Array.from(tag,c=>c.charCodeAt(0)));
 assert.notDeepEqual(t,hash(Buffer.from(tag))); 
 assert.equal(hexlify(c.sha256(...parts)),hexlify(hash(...parts)));
 assert.equal(hexlify(c.taggedHash(tag,...parts)),hexlify(hash(t,t,...parts)));
});
