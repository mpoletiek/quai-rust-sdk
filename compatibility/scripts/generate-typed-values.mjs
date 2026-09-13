// Pinned constructor and encoder observations; includes constructor validation gaps.
import {Typed,AbiCoder} from 'quais';
import {writeFileSync} from 'node:fs';
const coder=AbiCoder.defaultAbiCoder();
const capture=fn=>{try{return {value:fn()};}catch{return {error:true};}};
const vectors=[];
function add(type,value){const typed=capture(()=>Typed.from(type,value).format());vectors.push({type,value,constructor:typed,encoded:capture(()=>coder.encode([type],[value]))});}
const bounds=[];
for(let bits=8;bits<=256;bits+=8)for(const signed of [false,true]){
 const type=`${signed?'':'u'}int${bits}`;
 const min=signed?-(1n<<BigInt(bits-1)):0n;
 const max=(1n<<BigInt(signed?bits-1:bits))-1n;
 for(const n of [min-1n,min,0n,max,max+1n])add(type,n.toString());
 const t=Typed.from(type,'0');bounds.push({type,min:min.toString(),max:max.toString(),referenceDefault:t.defaultValue(),referenceMin:t.minValue(),referenceMax:t.maxValue()});
}
for(let size=1;size<=32;size++)for(const n of [size-1,size,size+1])add(`bytes${size}`,'0x'+'a5'.repeat(n));
for(const [type,value] of [['bool',false],['bool',true],['address','0x0000000000000000000000000000000000000000'],['bytes','0x0001'],['string','Qi 日本語'],['uint16[]',['1','65535']],['uint8[2]',['0','255']],['(bool,string,uint8[])',[true,'memo',['1','2']]],['bytes[][2]',[['0x01'],[]]],['uint8[0]',[]]])add(type,value);
writeFileSync(new URL('../fixtures/typed-values.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',note:'Typed constructors may accept invalid values; Rust validates immediately using the encoder contract. JS default/min/max helpers are stubs returning zero; Rust supplies type-correct defaults and integer bounds.',vectors,bounds},null,2)+'\n');
console.log(`Generated ${vectors.length} typed-value encoder vectors and ${bounds.length} integer bounds`);
