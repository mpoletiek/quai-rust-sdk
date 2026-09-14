#!/usr/bin/env python3
"""Read current isolated conversion receipts and destination state without writes."""
import argparse,json,pathlib,re
import harness
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--quai');p.add_argument('--qi');p.add_argument('--output',type=pathlib.Path,required=True)
a=p.parse_args()
origins={h for h in [a.quai,a.qi] if h is not None}
if not origins:p.error("at least one conversion origin is required")
for txhash in origins:
    if re.fullmatch('0x[0-9a-f]{64}',txhash) is None:raise ValueError('invalid origin hash')
harness.GENESIS='0xff38a93744ee5aae738addc88da4f6b171528244e81d34aa4b25579fa3f44ed2'
result={'state':harness.check(),'transactions':{},'originReceipts':{},'destinationReceipts':{}}
for kind,txhash in [('quaiToQi',a.quai),('qiToQuai',a.qi)]:
    if txhash is None:continue
    result['transactions'][kind]=harness.rpc('quai_getTransactionByHash',[txhash])
    receipt=harness.rpc('quai_getTransactionReceipt',[txhash]);result['originReceipts'][kind]=receipt
    if receipt:
        for etx in receipt.get('outboundEtxs',[]):
            etxhash=etx['hash'];result['destinationReceipts'][etxhash]=harness.rpc('quai_getTransactionReceipt',[etxhash])
# Prime mutates ETX value/type/hash. Correlate actual destination inclusions.
result['settlements']=[]
head=int(result['state']['height'],16)
starts=[int(r['blockNumber'],16) for r in result['originReceipts'].values() if r]
start=min(starts,default=head)
if head-start>128:raise ValueError('capture span exceeds128 blocks')
for height in range(start,head+1):
    block=harness.rpc('quai_getBlockByNumber',[hex(height),True])
    for tx in block['transactions']:
        if tx.get('originatingTxHash') in origins:
            receipt=harness.rpc('quai_getTransactionReceipt',[tx['hash']])
            if receipt is None or receipt['blockHash']!=block['hash']:raise ValueError('inconsistent settlement receipt')
            result['settlements'].append({'transaction':tx,'receipt':receipt})
if harness.check()['headHash']!=result['state']['headHash']:raise ValueError('head changed during capture')
result['lockedQuaiBalance']=harness.rpc('quai_getLockedBalance',['0x0049CdA3305ccB9cB23E7Ce2528ceF555E9a5B32'])
result['recipientQiOutpoints']=harness.rpc('quai_getOutpointsByAddress',['0x00f47c27f86b20e1BD18a0cAFf71b2eC5e23389a'])
result['refundQiOutpoints']=harness.rpc('quai_getOutpointsByAddress',['0x00d7BfBbdD71A5Ac547956D138a72CE7F9527369'])
a.output.write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({'height':result['state']['height'],'origins':{k:{'status':v.get('status'),'outboundEtxs':v.get('outboundEtxs')} if v else None for k,v in result['originReceipts'].items()},'destinations':{k:{'status':v.get('status'),'height':v.get('blockNumber')} if v else None for k,v in result['destinationReceipts'].items()},'settlements':[{'hash':s['transaction']['hash'],'origin':s['transaction']['originatingTxHash'],'etxType':s['transaction']['etxType'],'value':s['transaction']['value'],'status':s['receipt']['status'],'height':s['receipt']['blockNumber']} for s in result['settlements']],'lockedQuaiBalance':result['lockedQuaiBalance'],'quaiBalance':result['state']['balance'],'qiOutpoints':result['recipientQiOutpoints'],'refundOutpoints':result['refundQiOutpoints']},indent=2))
