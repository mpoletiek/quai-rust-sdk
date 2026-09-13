import {Interface} from 'quais';
import {writeFileSync} from 'node:fs';
const filters=[];
const field=(type,indexed=true)=>({name:'',type,indexed});
function add(type,criteria,rust='match'){
 const abi=[{type:'event',name:'Filtered',inputs:[field(type),field('uint256',false),field('bytes2')]}];
 const iface=new Interface(abi), row={abi,criteria,rust};
 const js=criteria.map(c=>c===null?null:('exact'in c?c.exact:c.anyOf));
 try{row.reference=iface.encodeFilterTopics('Filtered',js);}catch{row.referenceError=true;}
 if(rust==='match'){
  const topics=[iface.getEvent('Filtered').topicHash];
  for(const [i,c] of criteria.entries()){
   if(i===1)continue;
   const one=value=>iface.encodeEventLog('Filtered',[i===0?value:zero(type),'0',i===2?value:'0x0000']).topics[i===0?1:2];
   topics.push(c===null?null:('exact'in c?one(c.exact):c.anyOf.map(one)));
  }
  while(topics.at(-1)===null)topics.pop();
  row.topics=topics;
 }
 filters.push(row);
}
function zero(type){if(type==='bool')return false;if(type==='address')return '0x0000000000000000000000000000000000000000';if(type==='string')return '';if(type==='bytes')return '0x';if(type.startsWith('bytes'))return '0x'+'00'.repeat(Number(type.slice(5)));return '0';}
for(let n=8;n<=256;n+=8)for(const signed of [false,true]){
 const type=(signed?'int':'uint')+n,min=signed?-(1n<<BigInt(n-1)):0n,max=(1n<<BigInt(signed?n-1:n))-1n;
 add(type,[{exact:String(min)}]);add(type,[{anyOf:[String(min),String(max)]},null,{exact:'0xabcd'}]);
 add(type,[{exact:String(max+1n)}],'reject');
}
for(let n=1;n<=32;n++)add('bytes'+n,[{exact:'0x'+'ab'.repeat(n)}]);
for(const [type,values] of [['string',['','Qi 日本語 🍊']],['bytes',['0x','0x000123']],['bool',[true,false]],['address',['0x002b2596EcF05C93a31ff916E8b456DF6C77c750','0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB']]]){
 add(type,[{anyOf:values}]);add(type,[null,null,{exact:'0xabcd'}]);add(type,[null,null,null]);
}
add('uint8',[]);add('uint8',[{exact:'1'},null]);
for(const criteria of [[{anyOf:[]}],[{anyOf:Array(129).fill('1')}],[null,{exact:'1'}],[null,null,null,null],[{exact:'-1'}],[{exact:true}]])add('uint8',criteria,'reject');
add('bool',[{exact:'false'}],'reject');add('bytes2',[{exact:'0x01'}],'reject');
const abi=[
 {type:'function',name:'move',stateMutability:'payable',inputs:[{name:'to',type:'address'},{name:'amount',type:'uint256'},{name:'memo',type:'string'}],outputs:[{name:'',type:'bool'}]},
 {type:'error',name:'Denied',inputs:[{name:'who',type:'address'},{name:'why',type:'string'}]},
 {type:'event',name:'Tagged',inputs:[field('string'),field('uint256',false),field('int16')]}
];
const iface=new Interface(abi),address='0x002b2596EcF05C93a31ff916E8b456DF6C77c750';
const normalize=value=>JSON.parse(JSON.stringify(value,(_,v)=>typeof v==='bigint'?String(v):v));
const calls=[];
for(const amount of ['0','1',(2n**256n-1n).toString()]){
 const data=iface.encodeFunctionData('move',[address,amount,'Qi 日本語 🍊']),parsed=iface.parseTransaction({data,value:amount});
 calls.push({data,signature:parsed.signature,arguments:normalize(parsed.args),value:amount});
}
const reverts=[];
for(const [name,args] of [['Error',['']],['Error',['Qi 日本語 🍊']],['Panic',['0']],['Panic',['17']],['Panic',[(2n**256n-1n).toString()]],['Denied',[address,'拒否']]]){
 const data=iface.encodeErrorResult(name,args),parsed=iface.parseError(data);reverts.push({data,signature:parsed.signature,arguments:normalize(parsed.args)});
}
const logs=[];
for(const tag of ['','Qi 日本語 🍊']){
 const log=iface.encodeEventLog('Tagged',[tag,'123','-32768']),parsed=iface.parseLog(log);
 logs.push({...log,signature:parsed.signature,values:parsed.args.map(v=>v?.hash?{hash:v.hash}:{value:normalize(v)})});
}
writeFileSync(new URL('../fixtures/abi-workflows.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',note:'Filter topics independently use pinned encodeEventLog for canonical words, including negative integers rejected by pinned encodeFilterTopics. Rust rejects empty/oversized OR sets, coercion and declared-width overflow.',filters,abi,calls,reverts,logs},null,2)+'\n');
console.log(`Generated ${filters.length} filters, ${calls.length} calls, ${reverts.length} reverts and ${logs.length} logs`);
