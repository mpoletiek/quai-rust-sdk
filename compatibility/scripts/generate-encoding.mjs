// Deterministic published-reference byte and integer behavior; no network or keys.
import * as q from 'quais';
import {writeFileSync} from 'node:fs';
const capture = fn => { try { return {value: fn()}; } catch { return {error: true}; } };
const bytes = ['0x', '0x00', '0x0000', '0x01', '0x0001', '0xabcdef', '0x'+ 'ff'.repeat(32), '0x'+Array.from({length:64},(_,i)=>i.toString(16).padStart(2,'0')).join('')].map(input => ({input, hex:q.hexlify(input), base58:capture(()=>q.encodeBase58(input)), base58Integer:capture(()=>q.decodeBase58(q.encodeBase58(input)).toString()), base64:q.encodeBase64(input), stripped:q.stripZerosLeft(input), left:q.zeroPadValue(input,Math.max(8,q.dataLength(input))), right:q.zeroPadBytes(input,Math.max(8,q.dataLength(input)))}));
const strings = ['', 'Quai', 'Qi 日本語', '😀', 'a\0b', 'a'.repeat(31), 'a'.repeat(32), '😀'.repeat(8)].map(input=>({input,encoded:capture(()=>q.encodeBytes32(input)),decoded:capture(()=>q.decodeBytes32(q.encodeBytes32(input)))}));
const twos=[];
for(const bits of [1,8,16,64,128,255,256]) {
 const boundary=1n<<BigInt(bits-1);
 for(const value of [-boundary-1n,-boundary,-1n,0n,1n,boundary-1n,boundary]) twos.push({bits,input:value.toString(),encoded:capture(()=>q.toTwos(value,bits).toString()),decoded:capture(()=>q.fromTwos(q.toTwos(value,bits),bits).toString())});
}
const masks=[];for(const bits of [0,1,8,255,256]) for(const input of ['0','1',(2n**256n-1n).toString()]) masks.push({bits,input,expected:q.mask(BigInt(input),bits).toString()});
writeFileSync(new URL('../fixtures/encoding.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',bytes,strings,twos,masks},null,2)+'\n');
console.log(`Generated ${bytes.length+strings.length+twos.length+masks.length} encoding/boundary vectors`);
