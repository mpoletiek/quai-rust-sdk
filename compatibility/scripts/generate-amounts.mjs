import {parseUnits,formatUnits,keccak256,concat,toBeHex,zeroPadValue,getAddress} from 'quais';
import {getContractAddress} from 'quais/address';
import {writeFileSync} from 'node:fs';
const inputs=['0','-0','1','-1','.1','-.1','1.','00001.2000','0.000000000000000001','1.0001','1.23000','-0.000','', '.', '-.', '+1',' 1','1 ','1e3','1.2.3','１２','--1'];
const vectors=[];
for(const decimals of [0,3,18,80])for(const input of inputs){
 let expected;try{const integer=parseUnits(input,decimals);expected={integer:String(integer),formatted:formatUnits(integer,decimals)};}catch{expected={error:true};}
 vectors.push({id:`amount-${vectors.length}`,input,decimals,expected});
}
for(const integer of [(1n<<511n)-1n,1n<<511n,-(1n<<511n),-(1n<<511n)-1n,(1n<<256n)-1n,1n<<256n]){
 let expected;try{const n=parseUnits(String(integer),0);expected={integer:String(n),formatted:formatUnits(n,0)};}catch{expected={error:true};}
 vectors.push({id:`amount-${vectors.length}`,input:String(integer),decimals:0,expected});
}
const contracts=[];
for(const sender of ['0x0000000000000000000000000000000000000000','0x0011223344556677889900112233445566778899'])for(const nonce of [0n,1n,(1n<<64n)-1n])for(const code of ['0x','0x00','0x0000','0x006001','0x6001']){
 const exact=getAddress('0x'+keccak256(concat([sender,zeroPadValue(toBeHex(nonce),8),code])).slice(-40));
 const salt=zeroPadValue(toBeHex(nonce),32),codeHash=keccak256(code);
 const create2=getAddress('0x'+keccak256(concat(['0xff',sender,salt,codeHash])).slice(-40));
 contracts.push({sender,nonce:String(nonce),code,exact,legacy:getContractAddress(sender,nonce,code),salt,codeHash,create2});
}
writeFileSync(new URL('../fixtures/amounts.json',import.meta.url),JSON.stringify({schemaVersion:1,reference:'quais@1.0.0-alpha.57',vectors,contracts},null,2)+'\n');
console.log(`generated ${vectors.length} amount and ${contracts.length} contract vectors`);
