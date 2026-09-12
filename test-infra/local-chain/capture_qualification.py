#!/usr/bin/env python3
"""Read-only assertions/capture for the documented isolated multi-input/replacement run."""
import argparse
import json
import pathlib
import re
import harness

ADDRESSES = [
    '0x00f47c27f86b20e1BD18a0cAFf71b2eC5e23389a',
    '0x0093F3c023941CB49cb19e76C465168e19d2C428',
    '0x00e532Dd55c1AF571da5acaF8729D55278bd5907',
    '0x00d7BfBbdD71A5Ac547956D138a72CE7F9527369',
    '0x009cFEbAB1cC20D08e23eb7eA89DcEfAB3349E45',
    '0x00916958977b54C8B4403DBca74dC434ccd49d28',
]

def require(condition, message):
    if not condition:raise ValueError(message)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ['split','fund-a','fund-b','multi','original','replacement']:
        parser.add_argument('--'+name,required=True)
    parser.add_argument('--output',type=pathlib.Path,required=True)
    args=parser.parse_args()
    for name in ['split','fund_a','fund_b','multi','original','replacement']:
        require(re.fullmatch('0x[0-9a-f]{64}',getattr(args,name)) is not None,'invalid hash')
    state=harness.check()
    result={'profile':'explicitly-patched-disposable-local','state':state,'transactions':{},'receipts':{}}
    for name in ['split','fund_a','fund_b','multi','original','replacement']:
        txhash=getattr(args,name)
        tx=harness.rpc('quai_getTransactionByHash',[txhash])
        receipt=harness.rpc('quai_getTransactionReceipt',[txhash])
        result['transactions'][name]=tx;result['receipts'][name]=receipt
        if name=='original':
            require(receipt is None,'replaced original unexpectedly has receipt')
        else:
            require(receipt is not None and receipt['status']=='0x1','missing successful receipt '+name)
            require(receipt['transactionHash']==txhash and tx['hash']==txhash,'transaction ID mismatch')
            header=harness.rpc('quai_getHeaderByNumber',[receipt['blockNumber']])
            require(header['woHeader']['hash']==receipt['blockHash'],'noncanonical receipt '+name)
    multi=result['transactions']['multi'];inputs=multi['inputs']
    require(len(inputs)==3,'expected three inputs')
    require(inputs[0]['pubKey']==inputs[1]['pubKey'] and inputs[1]['pubKey']!=inputs[2]['pubKey'],'expected ordered duplicate signer keys')
    expected=[(value,'0x0') for value in sorted([args.fund_a,args.fund_b])]+[(args.split,'0x2')]
    actual=[(item['previousOutPoint']['txHash'],item['previousOutPoint']['index']) for item in inputs]
    require(actual==expected,'unexpected consumed outpoints/order')
    outputs=multi['outputs']
    require([(item['address'],item['denomination']) for item in outputs]==[(ADDRESSES[4],'0xd'),(ADDRESSES[5],'0xc')],'unexpected recipient/change')
    points={address:harness.rpc('quai_getOutpointsByAddress',[address]) for address in ADDRESSES}
    result['outpoints']=points
    require(all(not points[address] for address in ADDRESSES[:4]),'spent fixture outpoint remains indexed')
    for index,address,denomination in [(0,ADDRESSES[4],13),(1,ADDRESSES[5],12)]:
        entries=points[address];require(len(entries)==1,'unexpected output inventory')
        entry=entries[0]
        require(entry['txHash']==args.multi and int(entry['index'],16)==index and int(entry['denomination'],16)==denomination and entry['lock']=='0x0','wrong indexed output')
    receiver='0x0011223344556677889900112233445566778899'
    result['replacementRecipientBalance']=harness.rpc('quai_getBalance',[receiver,'latest'])
    require(int(result['replacementRecipientBalance'],16)==222,'replacement receiver amount mismatch')
    require(int(state['nonce'],16)==1,'replacement must consume exactly one account nonce')
    result['assertions']={'multiInputQiCanonical':True,'duplicateSignerKeys':True,'spentInputsAbsent':True,'recipientQits':100000000,'changeQits':10000000,'fixtureFeeQits':10000000,'originalReceiptAbsent':True,'replacementCanonical':True,'reorgQualified':False,'persistentWalletReconciliationQualified':False}
    args.output.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result['assertions'],indent=2))

if __name__=='__main__':main()
