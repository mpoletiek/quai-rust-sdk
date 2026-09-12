// Offline ABI fixtures through pinned quais.js; no wallets or RPC.
import { writeFileSync } from 'node:fs';
import { AbiCoder, Interface, ParamType } from '../../../compatibility/node_modules/quais/lib/esm/index.js';
const coder = AbiCoder.defaultAbiCoder();
const vectors=[];
const normalize = (value) => (typeof value === 'bigint' || typeof value === 'number') ? value.toString() : Array.isArray(value) ? Array.from(value,normalize) : value;
function add(id,types,values) {
 const encoded = coder.encode(types,values);
 let decoded, referenceDecodeError;
 try { decoded=normalize(coder.decode(types,encoded)); } catch(e) { if(!id.startsWith('zero-')) throw e; decoded=normalize(values); referenceDecodeError=e.code; }
 vectors.push({id,types,canonicalTypes:types.map(t=>ParamType.from(t).format('sighash')),values,encoded,decoded,referenceDecodeError});
}
add('empty',[],[]);
add('aliases',['uint','int'],['42','-17']);
add('primitives',['bool','bool','address','string','bytes'],[true,false,'0x0000000000000000000000000000000000000001','🦆 é\u0000','0x0012ff']);
add('empty-dynamics',['string','bytes','uint8[]'],['','0x',[]]);
for(let bits=8;bits<=256;bits+=8) add('integers-'+bits,['uint'+bits,'int'+bits,'int'+bits],[((1n<<BigInt(bits))-1n).toString(),(-(1n<<BigInt(bits-1))).toString(),((1n<<BigInt(bits-1))-1n).toString()]);
add('bytes1-through32',Array.from({length:32},(_,i)=>'bytes'+(i+1)),Array.from({length:32},(_,i)=>'0x'+'a5'.repeat(i+1)));
add('nested-fixed',['uint8[2][3]'],[[[1,2],[3,4],[5,6]]]);
add('nested-dynamic',['uint256[][]','string[]'],[[[1,2],[],[3]],['a','longer text','']]);
add('mixed-array',['string[][2]','uint8[2][]'],[[['one','two'],[]],[[1,2],[3,4]]]);
add('tuples',['(uint8,string,(bytes,bool))','(int16,bool)[2]'],[[1,'a',['0xff',true]],[[-1,false],[2,true]]]);
add('tuple-array',['(string,uint8[])[]'],[[['one',[1,2]],['two',[]]]]);
add('zero-tuples',['()','()[2]','()[]'],[[],[[],[]],[[],[],[]]]);
add('zero-fixed',['uint8[0]','uint8[0][]','string[0]'],[[],[[],[],[]],[]]);
add('zero-nested',['(()[],uint8)[2]'],[[[[[],[]],3],[[],4]]]);
const rejection=[];
for(const [types,values] of [[['uint7'],[1]],[['bytes33'],['0x']],[['uint8'],[256]],[['int8'],[-129]],[['bytes2'],['0x01']],[['uint8[2]'],[[1]]],[['(uint8,bool)'],[[1]]],[['address'],['0x01']],[['uint8'],[]],[['uint8[]'],[2]]]) {
 try { coder.encode(types,values); throw Error('unexpected acceptance'); } catch(e) { if(e.message==='unexpected acceptance') throw e; rejection.push({types,values}); }
}
const decodePolicy=[];
function strictDecode(id,types,encoded) {
 let referenceAccepts;
 try { normalize(coder.decode(types,encoded)); referenceAccepts=true; } catch { referenceAccepts=false; }
 decodePolicy.push({id,types,encoded,referenceAccepts});
}
strictDecode('bool-two',['bool'],'0x'+'00'.repeat(31)+'02');
strictDecode('uint8-dirty-high',['uint8'],'0x'+'00'.repeat(30)+'0100');
strictDecode('bytes2-dirty-padding',['bytes2'],'0x000001'+'00'.repeat(29));
const single=coder.encode(['bytes'],['0xaa']);
strictDecode('trailing-word',['bytes'],single+'00'.repeat(32));
const pair=getHexBytes(coder.encode(['bytes','bytes'],['0xaa','0xbb']));pair[63]=64;
strictDecode('aliased-tail',['bytes','bytes'],toHex(pair));
function getHexBytes(v){return Uint8Array.from(v.slice(2).match(/../g).map(x=>parseInt(x,16)));}
function toHex(v){return '0x'+Array.from(v,b=>b.toString(16).padStart(2,'0')).join('');}
const ifaceJson=[
 {type:'function',name:'transfer',stateMutability:'nonpayable',inputs:[{name:'to',type:'address'},{name:'amount',type:'uint256'}],outputs:[{name:'ok',type:'bool'}]},
 {type:'function',name:'lookup',stateMutability:'view',inputs:[{name:'id',type:'uint256'}],outputs:[{name:'record',type:'tuple',components:[{name:'name',type:'string'},{name:'values',type:'uint8[]'}]}]},
 {type:'function',name:'lookup',stateMutability:'view',inputs:[{name:'name',type:'string'}],outputs:[{name:'id',type:'uint256'}]},
 {type:'error',name:'Unauthorized',inputs:[{name:'caller',type:'address'}]},
 {type:'event',name:'Transfer',anonymous:false,inputs:[{name:'from',type:'address',indexed:true},{name:'to',type:'address',indexed:true},{name:'value',type:'uint256',indexed:false}]},
 {type:'event',name:'Note',anonymous:false,inputs:[{name:'text',type:'string',indexed:true},{name:'raw',type:'bytes',indexed:true},{name:'message',type:'string',indexed:false}]},
 {type:'event',name:'Anonymous',anonymous:true,inputs:[{name:'id',type:'uint256',indexed:true},{name:'ok',type:'bool',indexed:false}]},
 {type:'constructor',stateMutability:'nonpayable',inputs:[{name:'owner',type:'address'}]},
 {type:'fallback',stateMutability:'payable'},
 {type:'receive',stateMutability:'payable'}
];
const iface=new Interface(ifaceJson);
const calls=[['transfer',['0x'+'01'.repeat(20),'100'],[true]],['lookup(uint256)',['42'],[['alice',[1,2,3]]]],['lookup(string)',['bob'],['7']]].map(([name,args,result])=>({name,args,result,signature:iface.getFunction(name).format('sighash'),selector:iface.getFunction(name).selector,call:iface.encodeFunctionData(name,args),returns:iface.encodeFunctionResult(name,result)}));
const errors=[{name:'Unauthorized',args:['0x'+'02'.repeat(20)]}].map(v=>({...v,selector:iface.getError(v.name).selector,encoded:iface.encodeErrorResult(v.name,v.args)}));
const events=[['Transfer',['0x'+'01'.repeat(20),'0x'+'02'.repeat(20),'9']],['Note',['hello','0x0012','world']],['Anonymous',['7',true]]].map(([name,args])=>({name,args,topic:iface.getEvent(name).topicHash,log:iface.encodeEventLog(name,args)}));
writeFileSync(new URL('fixtures/abi.json',import.meta.url),JSON.stringify({schemaVersion:1,reference:'quais@1.0.0-alpha.57',vectors,rejection,decodePolicy,interface:ifaceJson,calls,errors,events},null,2)+'\n');
console.log(vectors.length,'ABI cases',rejection.length,'rejections');
