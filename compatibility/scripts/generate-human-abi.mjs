import {Fragment,Interface} from 'quais';
import {writeFileSync} from 'node:fs';
const declarations=[
 'function transfer(address to, uint amount) returns (bool success)',
 'function transfer(address to, uint amount, bytes memo) payable returns (bool success)',
 'function nested(tuple(uint n, tuple(address who, string label)[] rows)[2][] input) view returns (tuple(bytes32 id, uint count) result)',
 'function blank() pure',
 'function locations(bytes calldata input, address payable receiver) external payable returns (bytes memory output)',
 'function legacy(uint value) constant returns (uint answer)',
 'event Transfer(address indexed from, address indexed to, uint value)',
 'event Nested(tuple(uint n, address who)[] indexed rows, bytes32 indexed tag, string value)',
 'error Denied(address who, tuple(uint code, string reason) detail)',
 'constructor(address owner, tuple(uint limit, bytes data) options) payable',
 'constructor()', 'fallback()', 'fallback() payable', 'fallback(bytes data) payable returns (bytes result)', 'receive() payable',
 'function unnamed((uint,address)[], bytes32[0]) returns (int)',
 'event Empty()', 'error Empty()', 'function emptyTuple(()) returns (())',
 'function names(uint _value, address $owner) public',
];
for(let bits=8;bits<=256;bits+=8)for(const sign of ['int','uint']){
 declarations.push(`function widths(${sign}${bits}[2][] values) view returns (${sign}${bits} result)`);
 declarations.push(`event Width(${sign}${bits} indexed value)`);
}
for(let n=1;n<=32;n++)declarations.push(`function bytesWidth(bytes${n} data) returns (bytes${n} result)`);
const vectors=declarations.map(text=>{
 const f=Fragment.from(text), full=f.format('full'),minimal=f.format('minimal'), abi=JSON.parse(f.format('json'));
 // Pinned constructor JSON prints the string "undefined" for nonpayable; use the
 // valid standard ABI value as an explicit reference normalization.
 const normalizedConstructor=abi.type==='constructor'&&abi.stateMutability==='undefined';
 if(normalizedConstructor)abi.stateMutability='nonpayable';
 const normalizedIndexedArrays=[];
 if(f.type==='event')for(const [i,input] of f.inputs.entries()){
  if(input.indexed!==null && abi.inputs[i].indexed!==input.indexed){abi.inputs[i].indexed=input.indexed;normalizedIndexedArrays.push(i);}
 }
 const iface=new Interface([abi]);
 let signature=null,selector=null;
 if(f.type==='function'){signature=f.format('sighash');selector=iface.getFunction(signature).selector;}
 if(f.type==='event'){signature=f.format('sighash');selector=iface.getEvent(signature).topicHash;}
 if(f.type==='error'){signature=f.format('sighash');selector=iface.getError(signature).selector;}
 return {text,full,minimal,abi,signature,selector,normalizedConstructor,normalizedIndexedArrays};
});
const reject=[
 '', 'function', 'function a(uint x,)', 'function a(,uint x)', 'function a(uint x) garbage',
 'function a(uint x) payable view', 'function a() public external', 'function a() returns () returns ()',
 'function a(uint x,uint x)', 'function a(uint indexed x)', 'event A(uint indexed indexed x)',
 'event A(uint a) payable', 'error A(uint a) view', 'constructor() view', 'receive(uint x) payable',
 'receive() nonpayable', 'fallback(uint x)', 'fallback(bytes) returns (uint)', 'fallback() view',
 'function a(uint7 x)', 'function a(bytes33 x)', 'function a(uint[01] x)', 'function a(uint[-1] x)',
 'function a(tuple(uint x, uint x) t)', 'function a(uint x) @100',
 'struct Thing(uint x)', 'function a(uint x) { }', 'function a(uint x);', 'function a(uint x) // comment',
 'function a(string 名称)', 'event A(uint indexed a,uint indexed b,uint indexed c,uint indexed d)',
];
writeFileSync(new URL('../fixtures/human-abi.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',note:'Formatting/selector vectors from pinned Fragment/Interface; malformed or unsupported source declarations explicitly reject in Rust. Constructor JSON undefined string normalized to nonpayable; event array indexed flags restored from parsed Fragment metadata because pinned JSON formatting drops them. Gas annotations and source bodies are outside this ABI parser.',vectors,reject},null,2)+'\n');
console.log(`Generated ${vectors.length} human ABI formatting cases and ${reject.length} Rust rejection cases`);
