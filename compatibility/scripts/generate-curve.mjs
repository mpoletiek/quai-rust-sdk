import {musigCrypto as c,hexlify,getBytes} from 'quais';
import {writeFileSync} from 'node:fs';
const n=0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141n;
const p=0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2fn;
const b=x=>c.write32b(x),h=hexlify;
const inputs=[0n,1n,2n,3n,n/2n,n-2n,n-1n];
const scalars=[];
for(const a of inputs)for(const d of inputs)scalars.push({a:h(b(a)),b:h(b(d)),add:h(c.scalarAdd(b(a),b(d))),multiply:h(c.scalarMultiply(b(a),b(d))),negate:h(c.scalarNegate(b(a)))});
const reductions=[0n,n-1n,n,n+1n,(1n<<256n)-1n].map(a=>({input:h(b(a)),output:h(c.scalarMod(b(a)))}));
const points=[];
for(const key of [1n,2n,805n,n-1n])for(const scalar of [0n,1n,2n,n-1n]){
 const point=c.getPublicKey(b(key),true), other=c.getPublicKey(b(3n),true);
 const enc=v=>v===null?null:h(v);
 points.push({key:h(b(key)),point:h(point),uncompressed:h(c.pointCompress(point,false)),x:h(c.pointX(point)),even:c.hasEvenY(point),negate:h(c.pointNegate(point)),scalar:h(b(scalar)),other:h(other),multiply:enc(c.pointMultiplyUnsafe(point,b(scalar),true)),multiplyAdd:enc(c.pointMultiplyAndAddUnsafe(point,b(scalar),other,true)),add:enc(c.pointAdd(point,other,true)),tweak:enc(c.pointAddTweak(point,b(scalar),true))});
}
const fields=[0n,1n,2n,3n,7n,805n,p-1n].map(x=>({x:h(b(x)),rhs:h(b(c.secp256k1Right(x))),symbol:c.jacobiSymbol(x)}));
const hashes=[];
for(const tag of ['', 'BIP0340/challenge','MuSig/noncecoef','PUBLIC π','PUBLIC 🦀'])for(const parts of [[],['0x'],['0x00','0x0102'],['0x1234']])hashes.push({tag,parts,sha256:h(c.sha256(...parts.map(getBytes))),tagged:h(c.taggedHash(tag,...parts.map(getBytes)))});
writeFileSync(new URL('../fixtures/curve.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',scalars,reductions,points,fields,hashes},null,2)+'\n');
