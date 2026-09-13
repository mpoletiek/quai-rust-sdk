// Pinned FixedNumber observations, including known reference rounding defects.
import {FixedNumber} from 'quais';
import {writeFileSync} from 'node:fs';
const capture = fn => {try {return {value:fn().toString()};} catch {return {error:true};}};
const parse=[];
for(const format of ['fixed8x0','ufixed8x0','fixed16x2','fixed128x18','ufixed256x80']) {
 for(const input of ['0','-0','1','-1','127','128','-128','-129','255','256','1.25','-1.25','0.000000000000000001','1.001','1.2500']) {
  parse.push({format,input,result:capture(()=>FixedNumber.fromString(input,format))});
 }
}
const arithmetic=[];
for(const [format,a,b] of [['fixed16x2','1.25','2.50'],['fixed16x2','-1.25','2.50'],['fixed16x2','1.00','3.00'],['fixed16x2','1','0'],['fixed8x0','-128','-1'],['ufixed8x0','255','1'],['fixed16x2','327.67','0.01'],['ufixed256x80','0.'+'0'.repeat(40)+'1','0.'+'0'.repeat(38)+'1']]) {
 for(const operation of ['add','sub','mul','mulSignal','div','divSignal']) arithmetic.push({format,a,b,operation,result:capture(()=>FixedNumber.fromString(a,format)[operation](FixedNumber.fromString(b,format)))});
}
const rounding=[];
for(const input of ['-2.6','-2.5','-2.4','-1.6','-1.5','-1.4','-0.1','0','0.1','1.4','1.5','1.6','2.5']) {
 const n=FixedNumber.fromString(input,'fixed16x2');
 rounding.push({input,referenceFloor:n.floor().toString(),referenceCeiling:n.ceiling().toString(),referenceRound:n.round().toString(),correctFloor:Math.floor(Number(input)).toFixed(1),correctCeiling:Math.ceil(Number(input)).toFixed(1),correctRound:Math.round(Number(input)).toFixed(1)});
}
const bytes=[];
for(const format of ['fixed8x0','ufixed8x0','fixed16x2','ufixed16x2']) for(const input of ['0x','0x00','0x7f','0x80','0xff','0x0080','0x8000','0xffff','0x010000']) bytes.push({format,input,result:capture(()=>FixedNumber.fromBytes(input,format))});
writeFileSync(new URL('../fixtures/fixed.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',note:'Floor, ceiling and negative rounding observations intentionally differ from correct Rust mathematical rounding; small exact tenths use independent Math integer rounding expectations.',parse,arithmetic,rounding,bytes},null,2)+'\n');
console.log(`Generated ${parse.length+arithmetic.length+rounding.length+bytes.length} fixed-point vectors`);
