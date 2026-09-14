import {getBigInt,getNumber,toBigInt,toBeArray,toBeHex,toQuantity,isHexString,isBytesLike,SigningKey,hashMessage,verifyMessage,verifyTypedData,TypedDataEncoder,hexlify,MaxInt256,MinInt256,quaisymbol} from 'quais';
import {ShardData,ZoneData} from 'quais/constants';
import {writeFileSync} from 'node:fs';
const attempt=fn=>{try{return {value:String(fn())};}catch{return {error:true};}};
const texts=['0','-0','0012','-12','0xff','-0xff','0b101','-0b101','0o777','-0o777',String(MaxInt256),String(MinInt256),String((1n<<511n)-1n),String(-(1n<<511n)),String(1n<<511n),String(-(1n<<511n)-1n),'','-','0x','0b2','1.0','1e3','1_000','--1','+1',' 1 ',' ','0Xff'];
const integers=texts.map(text=>({text,source:attempt(()=>getBigInt(text)),rustReject:['-','+1',' 1 ',' ','0Xff',String(1n<<511n),String(-(1n<<511n)-1n)].includes(text)}));
const numbers=[0,-0,1,-1,9007199254740991,-9007199254740991,9007199254740992,-9007199254740992,1.5].map(value=>({input:value,source:attempt(()=>getNumber(value))}));
const unsigned=[0n,1n,15n,16n,255n,256n,9007199254740993n,(1n<<256n)-1n].map(value=>({value:String(value),array:hexlify(toBeArray(value)),quantity:toQuantity(value),widths:[null,0,1,2,32,33].map(width=>({width,source:attempt(()=>toBeHex(value,width??undefined))}))}));
const hex=['0x','0x0','0x00','0xABcd','0Xab','0xgg',' 0x00','0x000001'].map(text=>({text,any:isHexString(text),bytes:isBytesLike(text),exact:[0,1,2,3].map(n=>isHexString(text,n))}));
const key=new SigningKey('0x'+'00'.repeat(31)+'01');
const messages=['','PUBLIC π','0x4243',Uint8Array.of(0x42,0x43),Uint8Array.of(0,255)].map(message=>{
 const signature=key.sign(hashMessage(message)).serialized;
 return {bytes:hexlify(typeof message==='string'?new TextEncoder().encode(message):message),signature,address:verifyMessage(message,signature)};
});
const typed=[];const types={Mail:[{name:'message',type:'string'},{name:'count',type:'uint256'}]};
for(const chainId of [9,15000]){
 const domain={name:'PUBLIC SDK test',chainId};const value={message:'PUBLIC π',count:(1n<<255n)+7n};const signature=key.sign(TypedDataEncoder.hash(domain,types,value)).serialized;
 typed.push({document:TypedDataEncoder.getPayload(domain,types,value),signature,address:verifyTypedData(domain,types,value,signature)});
}
writeFileSync(new URL('../fixtures/final-utilities.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',integers,numbers,unsigned,hex,messages,typed,shards:ShardData,zones:ZoneData,constants:{maxInt256:String(MaxInt256),minInt256:String(MinInt256),symbol:quaisymbol}},null,2)+'\n');
