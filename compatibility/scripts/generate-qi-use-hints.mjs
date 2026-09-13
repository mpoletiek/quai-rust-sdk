// Exercise the published checker without a provider, wallet secrets or network.
import {QiHDWallet} from 'quais';
import {writeFileSync} from 'node:fs';
const address='0x0080000000000000000000000000000000000001';
const vectors=[];
for(const outputs of [0,1]) for(const checker of [null,false,true,'error']) {
  let calls=0;
  const context={getOutpointsByAddress:async()=>outputs?[{}]:[]};
  if(checker!==null) context._addressUseChecker=async()=>{calls++;if(checker==='error')throw Error('public fixture');return checker;};
  let used=null,error=false;
  try {used=(await QiHDWallet.prototype.checkAddressUse.call(context,address)).isUsed;} catch {error=true;}
  vectors.push({outputs,checker,calls,used,error});
}
writeFileSync(new URL('../fixtures/qi-use-hints.json',import.meta.url),JSON.stringify({reference:'quais@1.0.0-alpha.57',vectors},null,2)+'\n');
console.log(`Generated ${vectors.length} optional address-use cases`);
