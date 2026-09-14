import {FixedNumber} from 'quais';import {writeFileSync} from 'node:fs';
const display=(n,d)=>{const sign=n<0n?'-':'';let s=String(n<0n?-n:n);if(!d)return sign+s;s=s.padStart(d+1,'0');return sign+s.slice(0,-d)+'.'+(s.slice(-d).replace(/0+$/,'')||'0');};
const bits=value=>{const view=new DataView(new ArrayBuffer(8));view.setFloat64(0,value);return view.getBigUint64(0).toString(16).padStart(16,'0');};
const vectors=[];
for(const [signed,width,decimals] of [[true,8,0],[false,8,0],[true,16,2],[true,128,18],[false,256,80]]){
 const limit=1n<<BigInt(width-(signed?1:0));const format=`${signed?'':'u'}fixed${width}x${decimals}`;
 const values=signed?[-limit,-limit+1n,-1n,0n,1n,limit-1n]:[0n,1n,2n,limit-2n,limit-1n];
 for(const a of values)for(const b of values){
  const aa=FixedNumber.fromString(display(a,decimals),format),bb=FixedNumber.fromString(display(b,decimals),format);
  const outputs={};
  for(const [name,op] of [['add','addUnsafe'],['sub','subUnsafe'],['mul','mulUnsafe'],['div','divUnsafe']]){
   let correct,source;
   if(name==='div'&&b===0n)correct={error:true};else{
    const raw=name==='add'?a+b:name==='sub'?a-b:name==='mul'?a*b/(10n**BigInt(decimals)):a*(10n**BigInt(decimals))/b;
    correct={units:String(signed?BigInt.asIntN(width,raw):BigInt.asUintN(width,raw))};
   }
   try{const result=aa[op](bb);source={units:String(result.value),display:result.toString()};}catch{source={error:true};}
   outputs[name]={correct,source};
  }
  vectors.push({format,a:display(a,decimals),b:display(b,decimals),sourceA:String(aa.value),sourceB:String(bb.value),floatBits:bits(Number(display(a,decimals))),sourceFloatBits:bits(aa.toUnsafeFloat()),outputs});
 }
}
writeFileSync(new URL('../fixtures/fixed-completion.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',scope:'Reference methods plus independent BigInt.asIntN/asUintN modular expectations; source signed-minimum defects retained explicitly.',vectors},null,2)+'\n');
console.log(JSON.stringify({vectors:vectors.length,operations:vectors.length*4,sourceDifferences:vectors.reduce((n,v)=>n+Object.values(v.outputs).filter(o=>JSON.stringify(o.correct)!==JSON.stringify(o.source.error?{error:true}:{units:o.source.units})).length,0)}));
