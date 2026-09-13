// Pinned packed layout and hash observations, including reference-only extensions.
import {solidityPacked,solidityPackedKeccak256,solidityPackedSha256} from 'quais';
import {writeFileSync} from 'node:fs';
const vectors=[];
function add(types,values,rust='match',note){
 const row={types,values,rust};if(note)row.note=note;
 try{row.packed=solidityPacked(types,values);row.keccak256=solidityPackedKeccak256(types,values);row.sha256=solidityPackedSha256(types,values);}catch{row.error=true;}
 vectors.push(row);
}
add([],[]);
add(['int16','bytes1','uint16','string'],['-1','0x42','3','Hello, world!']);
for(let bits=8;bits<=256;bits+=8)for(const signed of [false,true]){
 const type=`${signed?'':'u'}int${bits}`,min=signed?-(1n<<BigInt(bits-1)):0n,max=(1n<<BigInt(signed?bits-1:bits))-1n;
 for(const value of [min,0n,max])add([type],[String(value)]);
 add([type+'[]'],[[String(min),String(max)]]);
 add([type],[String(max+1n)],'reject');
 add([type+'[]'],[[String(max+1n)]],'reject','Rust checks the declared element width; JS widens array integers to 256 bits before range checking.');
}
for(let n=1;n<=32;n++){
 const value='0x'+'ab'.repeat(n);
 add([`bytes${n}`],[value]);add([`bytes${n}[2]`],[[value,value]]);
 add([`bytes${n}`],['0x'+'00'.repeat(n-1)],'reject');
}
const address='0x002b2596EcF05C93a31ff916E8b456DF6C77c750';
for(const [type,value] of [['address',address],['address[]',[address,address]],['bool',false],['bool',true],['bool[]',[true,false]],['string','Qi 日本語 🍊'],['bytes','0x000102'],['bytes','0x'],['uint8[0]',[]],['uint', '42'],['int','-42'],['uint8[2][2]',[['0','255'],['3','4']]],['uint8[][]',[[],['1','2']]],['bytes[]',['0x01','0x0002']],['string[]',['Qi','日本語']],['bytes[][2]',[['0x0001'],[]]]])add([type],[value]);
add(['string','string'],['a','bc']);add(['string','string'],['ab','c']);
add(['address','uint8[]','bytes3','bool','string'],[address,['1','2'],'0x000102',false,'Qi']);
for(const [type,value] of [['bool','false'],['bool',1],['uint8[2]',['1']],['bytes2','0x0'],['address','0x1234'],['uint7','1'],['uint08','1'],['bytes0','0x'],['uint8[01]',['1']],['(uint8)',['1']],['(uint8)[]',[]]])add([type],[value],'reject','Strict typed Rust input/schema policy.');
add(['uint8'],[],'reject');
writeFileSync(new URL('../fixtures/packed.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',note:'Exact pinned JS packed bytes and both hashes. Nested arrays and raw unpadded string/bytes array elements are reference-helper extensions; not Solidity compiler qualification. Rust rejects JS boolean coercion and out-of-range declared array element widths.',vectors},null,2)+'\n');
console.log(`Generated ${vectors.length} packed encoding/hash observations`);
