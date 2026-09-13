// Exact pre-grinding constructor payloads from the published ContractFactory.
import {ContractFactory} from 'quais';
import {writeFileSync} from 'node:fs';
const abi=[{type:'constructor',inputs:[{name:'count',type:'uint8'},{name:'note',type:'bytes'}],stateMutability:'nonpayable'}];
const vectors=[];
async function add(name,output,args,strictReject=false){
 let reference;try {const factory=ContractFactory.fromSolidity(output);const tx=await factory.getDeployTransaction(...args);reference={bytecode:factory.bytecode,data:tx.data};}catch {reference={error:true};}
 vectors.push({name,output,args,strictReject,reference});
}
for(const [name,code] of [['hex','0x00006000'],['unprefixed','00006000'],['object',{object:'00006000',sourceMap:'0:1:0'}]]) await add(name,{abi,bytecode:code},['255','0x000102']);
await add('evm',{abi,evm:{bytecode:{object:'00006000'}}},['1','0x']);
await add('matching_locations',{abi:[],bytecode:'6000',evm:{bytecode:{object:'0x6000'}}},[]);
await add('conflicting_locations',{abi:[],bytecode:'6000',evm:{bytecode:{object:'0x6001'}}},[],true);
await add('empty',{abi:[],bytecode:'0x'},[],true);
await add('missing_code',{abi:[]},[],true);
await add('invalid_hex',{abi:[],bytecode:'0x0g'},[]);
await add('link_placeholder',{abi:[],bytecode:'60__$1234567890123456789012345678901234$__'},[]);
await add('bad_argument',{abi,bytecode:'6000'},['256','0x']);
await add('extra_argument',{abi:[],bytecode:'6000'},['5']);
writeFileSync(new URL('../fixtures/artifacts.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',note:'Payloads precede Quai grinding. Rust explicitly rejects empty/missing or conflicting creation bytecode; selection from full compiler output is explicit.',vectors},null,2)+'\n');
console.log(`Generated ${vectors.length} Solidity artifact/constructor cases`);
