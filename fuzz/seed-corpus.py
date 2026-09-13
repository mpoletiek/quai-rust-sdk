#!/usr/bin/env python3
"""Freeze public reference bytes as starter corpus; never read live wallets or nodes."""
import hashlib,json,pathlib
root=pathlib.Path(__file__).resolve().parent.parent
counts={}
def put(target,data):
 if len(data)>65536:return
 directory=root/'fuzz'/'corpus'/target;directory.mkdir(parents=True,exist_ok=True)
 (directory/hashlib.sha256(data).hexdigest()).write_bytes(data)
 counts[target]=counts.get(target,0)+1
def load(path):return json.loads((root/path).read_text())
def hexbytes(s):return bytes.fromhex(s.removeprefix('0x'))
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
for row in load('crates/quai-keystore/tests/fixtures/keystores.json')['vectors']:put('wallet_import',json.dumps(row['json']).encode())
for row in load('crates/quai-crypto/tests/fixtures/quais-crypto.json')['vectors']:
 for key in ['compressed','uncompressed','signature']:
  if isinstance(row.get(key),str):put('wallet_import',hexbytes(row[key]))
for row in load('compatibility/fixtures/encoding.json')['bytes']:
 put('encoding',hexbytes(row['input']))
 for field in ['base64']:
  put('encoding',row[field].encode())
for row in load('compatibility/fixtures/encoding.json')['strings']:put('encoding',row['input'].encode())
for row in load('compatibility/fixtures/fixed.json')['parse']:put('fixed',row['input'].encode())
for scale in [0,6,18,80]:
 for raw in [bytes(32),bytes([255])*32,bytes([128])+bytes(31)]:put('fixed',bytes([63,scale])+raw)
for target in ['transactions','abi','wallet_import','encoding','fixed']:
 for data in [b'',b'\x00',b'\xff'*64,b'{}',b'{"version":3,"version":3}',b'\x08\x80\x80\x80\x80\x80\x80\x80\x80\x80\x80\x01']:put(target,data)
print(json.dumps({target:len(list((root/'fuzz'/'corpus'/target).iterdir())) for target in counts}))
