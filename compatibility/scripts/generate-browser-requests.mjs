// Pure pinned getRpcTransaction observations: no provider construction or network.
import {JsonRpcProvider} from 'quais';
import {readFileSync,writeFileSync} from 'node:fs';
const from='0x0049cda3305ccb9cb23e7ce2528cef555e9a5b32';
const base={type:0,chainId:15000,from,nonce:7,value:9007199254740993n,gasLimit:25000,gasPrice:123456789n,data:'0x000102',accessList:[]};
const cases=[['transfer',{...base,to:from}],['creation',{...base,data:'0x60006000'}],['access_list',{...base,to:from,accessList:[{address:from,storageKeys:['0x'+'01'.repeat(32),'0x'+'01'.repeat(32)]},{address:from,storageKeys:[]}]}]];
const vectors=cases.map(([name,tx])=>({name,request:JsonRpcProvider.prototype.getRpcTransaction.call(null,tx)}));
const submissions=[];
for(const [path,ids] of [
 ['../fixtures/transactions.json',['qi-0','qi-multi-0']],
 ['../../crates/quai-consensus/tests/conversion-vectors.json',['conversion-qi-0']],
 ['../../crates/quai-consensus/tests/wrapping-vectors.json',['wrapping-0']],
]) for(const v of JSON.parse(readFileSync(new URL(path,import.meta.url))).vectors){if(ids.includes(v.id))submissions.push({id:v.id,source:path,signed:v.signed,hash:v.hash});}
writeFileSync(new URL('../fixtures/browser-requests.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',method:'quai_signTransaction',vectors,submissions},null,2)+'\n');
console.log(`Generated ${vectors.length} injected signing RPC requests`);
