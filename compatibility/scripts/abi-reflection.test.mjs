import test from 'node:test';
import assert from 'node:assert/strict';
import {FunctionFragment,ConstructorFragment,ParamType,Result,Interface,StructFragment} from 'quais';
import {readFileSync} from 'node:fs';
const fixture=JSON.parse(readFileSync(new URL('../fixtures/abi-reflection.json',import.meta.url)));
test('published gas annotations are exact but JSON formatting throws on bigint',()=>{
 for(const row of fixture.gas){const f=(row.text.startsWith('constructor')?ConstructorFragment:FunctionFragment).from(row.text);assert.equal(f.format('full'),row.full);assert.equal(String(f.gas),row.gas);assert.throws(()=>f.format('json'),TypeError);}
});
test('published named results retain slice names, suppress duplicates and expose explicit collisions',()=>{
 const r=Result.fromItems([1,2,3,4],['same','same','length','__proto__']);assert.equal(r.getValue('same'),undefined);assert.equal(r.getValue('length'),3);assert.equal(r.getValue('__proto__'),4);assert.throws(()=>r.toObject());assert.throws(()=>r.push(5));
 const s=r.slice(2);assert.equal(s.getValue('length'),3);assert.equal(s.getValue('__proto__'),4);assert.deepEqual(s.toObject(),{length:3});
 const iface=new Interface(fixture.abi);assert.equal(iface.decodeFunctionResult('f',fixture.returns.data).getValue('then'),99n);
});
test('published tuple walks visit leaves and async supports named tuple objects',async()=>{
 const p=ParamType.from('tuple(uint count,tuple(string label,bool enabled)[] rows)');const value=[7,[['a',true],['b',false]]],seen=[];
 const result=p.walk(value,(type,v)=>{seen.push(type);return v;});assert.deepEqual(result,value);assert.deepEqual(seen,['uint256','string','bool','string','bool']);
 assert.deepEqual(await p.walkAsync({count:7,rows:[{label:'a',enabled:true},{label:'b',enabled:false}]},async(_,v)=>v),value);
 assert.throws(()=>p.walk({count:7,rows:[]},(_,v)=>v));
});
test('published parameter array JSON loses indexed metadata and struct formatter is unfinished',()=>{
 const p=ParamType.from('tuple(uint n)[] indexed rows',true);assert.equal(p.indexed,true);assert.equal(JSON.parse(p.format('json')).indexed,undefined);
 const s=StructFragment.from({name:'Record',inputs:[{name:'value',type:'uint256'}]});assert.equal(s.name,'Record');assert.throws(()=>s.format(),/@TODO/);
});
