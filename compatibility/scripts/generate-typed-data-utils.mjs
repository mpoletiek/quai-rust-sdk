import {TypedDataEncoder} from 'quais';
import {writeFileSync} from 'node:fs';
const types={Message:[{name:'owner',type:'Person'},{name:'members',type:'Person[]'},{name:'matrix',type:'uint256[2][]'}],Person:[{name:'name',type:'string'},{name:'wallet',type:'address'}]};
const person={name:'Alice',wallet:'0x0011223344556677889900112233445566778899'},message={owner:person,members:[person,{name:'Bob',wallet:'0x0011223344556677889900112233445566778800'}],matrix:[[1,2],[3,4]]};
const encoder=new TypedDataEncoder(types);
const inputs=[['Message',message],['Person',person],['Person[]',[person]],['Person[2][]',[[person,person]]],['uint256[][]',[[1,2],[]]],['bytes','0x1234'],['string','snow ☃'],['bool',true],['address',person.wallet],['uint256[0]',[]],['Person[]',[]]];
for(let bits=8;bits<=256;bits+=8) {inputs.push([`uint${bits}`,String((1n<<BigInt(bits))-1n)]);inputs.push([`int${bits}`,String(-(1n<<BigInt(bits-1)))]);}
for(let n=1;n<=32;n++)inputs.push([`bytes${n}`,'0x'+'a5'.repeat(n)]);
const vectors=inputs.map(([type,value])=>({type,value,encoding:encoder.getEncoder(type)(value),hash:encoder.hashStruct(type,value)}));
const visited=[];const transformed=encoder.visit(message,(type,value)=>{visited.push({type,value});return type==='address'?value.toLowerCase():value;});
const fixture={reference:'quais@1.0.0-alpha.57',types,message,vectors,visited,transformed};
writeFileSync(new URL('../fixtures/typed-data-utils.json',import.meta.url),JSON.stringify(fixture,null,2)+'\n');console.log(`Generated ${vectors.length} exact EIP-712 type encodings and a nested visitation vector`);
