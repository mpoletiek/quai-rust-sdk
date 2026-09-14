#!/usr/bin/env python3
"""Freeze public reference bytes as starter corpus; never read live wallets or nodes."""
import hashlib,json,pathlib
root=pathlib.Path(__file__).resolve().parent.parent
counts={}
def put(target,data):
 if len(data)>(163887 if target=='head_state' else 65536):return
 directory=root/'fuzz'/'corpus'/target;directory.mkdir(parents=True,exist_ok=True)
 (directory/hashlib.sha256(data).hexdigest()).write_bytes(data)
 counts[target]=counts.get(target,0)+1
def load(path):return json.loads((root/path).read_text())
def hexbytes(s):return bytes.fromhex(s.removeprefix('0x'))
for name in ['orchard.json','lan-mainnet.json']:
 for row in load('crates/quai-provider/tests/fixtures/'+name)['records']:
  for field in ['transaction','receipt']:
   if row.get(field) is not None:put('transactions',json.dumps(row[field],separators=(',',':')).encode())
  for log in row.get('receipt',{}).get('logs',[]):put('transactions',json.dumps(log,separators=(',',':')).encode())
for row in load('compatibility/fixtures/transactions.json')['vectors']:
 for key,value in row.items():
  if key.lower() in ['unsigned','signed','unsignedserialized','serialized','unsignedbytes','signedbytes'] and isinstance(value,str):put('transactions',hexbytes(value))
for row in load('crates/quai-consensus/tests/conversion-vectors.json')['vectors']:
 for key,value in row.items():
  if key in ['unsigned','signed'] and isinstance(value,str):put('transactions',hexbytes(value))
for row in load('crates/quai-consensus/tests/wrapping-vectors.json')['vectors']:
 for key in ['unsigned','signed']:put('transactions',hexbytes(row[key]))
abi=load('crates/quai-abi/tests/fixtures/abi.json')
for row in abi['vectors']:put('abi',hexbytes(row['encoded']))
put('abi',json.dumps(abi['interface']).encode())
for row in load('crates/quai-abi/tests/fixtures/typed-data.json')['vectors']:
 put('abi',json.dumps(dict(types=row['types'],primaryType=row['primaryType'],domain=row['domain'],message=row['value'])).encode())
for row in load('compatibility/fixtures/legacy-wallets.json')['vectors']:put('wallet_import',json.dumps(row['document']).encode())
for row in load('crates/quai-keystore/tests/fixtures/keystores.json')['vectors']:put('wallet_import',json.dumps(row['json']).encode())
for row in load('crates/quai-crypto/tests/fixtures/quais-crypto.json')['vectors']:
 for key in ['compressed','uncompressed','signature']:
  if isinstance(row.get(key),str):put('wallet_import',hexbytes(row[key]))
for row in load('test-infra/fixtures/payment-allocation.json')['vectors']:
 for state in row['states'].values():put('wallet_import',hexbytes(state))
for kind in ['addresses','payments']:
 for row in load('test-infra/fixtures/allocation-merge.json')[kind]:put('wallet_import',hexbytes(row['merged']))
for row in load('test-infra/fixtures/qi-custody.json')['vectors']:
 for state in row['states'].values():put('wallet_import',hexbytes(state))
for state in load('test-infra/fixtures/account-custody.json')['states'].values():put('wallet_import',hexbytes(state))
for row in load('test-infra/fixtures/address-allocation.json')['vectors']:
 for state in row['states'].values():put('wallet_import',hexbytes(state))
for name in ['full-backup-vector.json','full-backup-v2-vector.json','full-backup-v3-portable.json','full-backup-v4-portable.json','full-backup-v5-portable.json']:
 put('wallet_import',hexbytes(load('crates/quai-wallet/tests/'+name)['envelope']))
for row in load('crates/quai-wallet/tests/reference.json')['grinding']:
 public=b'QADDR001'+hexbytes(row['publicKey'])
 put('wallet_import',public+bytes([0]))
 put('wallet_import',public+bytes([1,int(row['coin']==969),int(row['change'])])+row['account'].to_bytes(4,'big')+row['index'].to_bytes(4,'big'))
for row in load('compatibility/fixtures/encoding.json')['bytes']:
 put('encoding',hexbytes(row['input']))
 for field in ['base64']:
  put('encoding',row[field].encode())
for row in load('compatibility/fixtures/encoding.json')['strings']:put('encoding',row['input'].encode())
for row in load('compatibility/fixtures/text-crypto.json')['texts']:put('encoding',row['text'].encode())
for row in load('compatibility/fixtures/text-crypto.json')['decoding']:put('encoding',hexbytes(row['hex']))
for row in load('compatibility/fixtures/artifacts.json')['vectors']:put('abi',json.dumps(row['output']).encode())
for row in load('compatibility/fixtures/human-abi.json')['vectors']:
 put('abi',row['text'].encode())
 put('abi',json.dumps([row['abi']]).encode())
for text in load('compatibility/fixtures/human-abi.json')['reject']:put('abi',text.encode())
for row in load('compatibility/fixtures/typed-values.json')['vectors']:put('abi',row['type'].encode())
for row in load('compatibility/fixtures/typed-values.json')['defaults']:put('abi','\n'.join(row['types']).encode())
packed=load('compatibility/fixtures/packed.json')['vectors']
for row in packed[::17]+packed[-28:]:put('abi',json.dumps({'types':row['types'],'values':row['values']}).encode())
workflows=load('compatibility/fixtures/abi-workflows.json')
for row in workflows['filters'][::11]+workflows['filters'][-10:]:put('abi',json.dumps({key:row[key] for key in ['abi','criteria']}).encode())
for group in ['calls','reverts','logs']:
 for row in workflows[group]:put('abi',json.dumps({'abi':workflows['abi'],**row}).encode())
for row in load('compatibility/fixtures/fixed.json')['parse']:put('fixed',row['input'].encode())
for scale in [0,6,18,80]:
 for raw in [bytes(32),bytes([255])*32,bytes([128])+bytes(31)]:put('fixed',bytes([63,scale])+raw)
for count in [1,2,16,4096]:
 header=b'QHEAD001'+bytes([0])+bytes([1])*32+(4096).to_bytes(2,'big')+(256).to_bytes(2,'big')+count.to_bytes(2,'big')
 anchors=b''.join(n.to_bytes(8,'big')+(bytes([1])*32 if n==0 else (n+1).to_bytes(32,'big')) for n in range(count))
 put('head_state',header+anchors)
for target in ['transactions','abi','wallet_import','encoding','fixed','head_state']:
 for data in [b'',b'\x00',b'\xff'*64,b'{}',b'{"version":3,"version":3}',b'\x08\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x01']:put(target,data)

reflection=load('compatibility/fixtures/abi-reflection.json')
for row in reflection['parameters']:
 put('abi',row['text'].encode());put('abi',json.dumps(row['json']).encode())
for row in reflection['gas']:put('abi',row['text'].encode())
for parameter,value in [({'type':'tuple[]','components':[{'type':'uint','name':'n'},{'type':'string','name':'label'}]},[{'n':7,'label':'a'}]),({'type':'uint[2]'},[1,2]),({'type':'bytes'},'0x1234')]:put('abi',json.dumps({'parameter':parameter,'value':value}).encode())
put('abi',json.dumps({'items':[1,2,3,4],'names':['same','same','__proto__','then']}).encode())



typed_utils=load('compatibility/fixtures/typed-data-utils.json')
for row in typed_utils['vectors']:put('abi',json.dumps({'types':typed_utils['types'],'type':row['type'],'value':row['value']}).encode())


crypto_utils=load('compatibility/fixtures/crypto-utils.json')
for row in crypto_utils['signatures']:
 put('wallet_import',hexbytes(row['compact']));put('wallet_import',hexbytes(row['serialized']));put('wallet_import',hexbytes(row['r'])+hexbytes(row['s'])+int(row['networkV']).to_bytes(32,'big'))
for row in crypto_utils['pairs']:put('wallet_import',hexbytes(crypto_utils['keys'][row['a']]['privateKey'])+hexbytes(crypto_utils['keys'][row['b']]['privateKey']))
wordlists=load('compatibility/fixtures/wordlists.json')
for row in wordlists['languages']:
 if row['owl']:put('wallet_import',json.dumps({k:row[k] for k in ['owl','accents','checksum']}).encode())
 put('wallet_import',row['phrase'].encode())
 if row['locale'].startswith('zh_'):put('wallet_import',row['phrase'].replace(' ','').encode())
for source in ['compatibility/fixtures/transactions.json','crates/quai-consensus/tests/conversion-vectors.json','crates/quai-consensus/tests/wrapping-vectors.json']:
 for row in load(source)['vectors']:put('transactions',json.dumps(row['input']).encode())
address='0x0011223344556677889900112233445566778899';slot='0x'+'ab'*32
for value in [[[address,[slot,slot]]],{address:[slot,slot]},[{'address':address,'storageKeys':[slot]}]]:put('transactions',json.dumps(value).encode())
for source in ['compatibility/fixtures/transactions.json','crates/quai-consensus/tests/conversion-vectors.json','crates/quai-consensus/tests/wrapping-vectors.json']:
 for row in load(source)['vectors']:
  if row['kind']!='qi':continue
  v=row['input'];rpc={'hash':row['hash'],'type':'0x2','blockHash':None,'blockNumber':None,'transactionIndex':None,'chainId':hex(int(v['chainId'])),'gas':'0x0','nonce':'0x0','input':v['data'] or '0x','utxoSignature':v['signature'],'inputs':[{'previousOutPoint':{'txHash':i['txhash'],'index':hex(i['index'])},'pubKey':i['pubkey']} for i in v['txInputs']],'outputs':[{'address':o['address'],'denomination':hex(o['denomination']),'lock':None} for o in v['txOutputs']]}
  put('transactions',json.dumps(rpc).encode())
for row in load('compatibility/fixtures/contract-io.json')['vectors']:put('abi',json.dumps({k:row[k] for k in ['abi','data','value']}).encode())
for row in workflows['logs'] if 'logs' in workflows else []:
 if 'topics' not in row or 'data' not in row:continue
 log={'address':address,'blockHash':'0x'+'11'*32,'blockNumber':'0xa','transactionHash':'0x'+'22'*32,'transactionIndex':'0x0','logIndex':'0x0','removed':True,'topics':row['topics'],'data':row['data']}
 put('abi',json.dumps({'abi':workflows['abi'],'log':log}).encode())
for row in load('compatibility/fixtures/fetch.json')['data']:put('encoding',row['uri'].encode())
for uri in ['https://example.invalid/public','ipfs://QmPublic/a.json','data:;base64,AAH/','https://user:pw@example.invalid/']:put('encoding',uri.encode())
for v in [{'header':'Authorization','value':'Bearer PUBLIC-TOY'},{'header':'x-public','value':'bad\r\nheader'},{'n':'9007199254740993'}]:put('encoding',json.dumps(v).encode())
put('encoding',b'266666666666666666666666666666666626666666666666222242')
print(json.dumps({target:len(list((root/'fuzz'/'corpus'/target).iterdir())) for target in counts}))
