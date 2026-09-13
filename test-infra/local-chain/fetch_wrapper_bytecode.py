#!/usr/bin/env python3
"""Fetch hash-pinned public WQUAI bytecode for isolated acceptance, without submitting RPCs."""
import hashlib
import json
import pathlib
import urllib.request

ADDRESS = '0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB'
SOURCE = 'https://quaiscan.io/api/v2/smart-contracts/' + ADDRESS
HASHES = {
    'creation_bytecode': 'e670f177b98bb8bd4653a78f295308e31fe70e9ec30b396fd4bbb7d1be1518a8',
    'deployed_bytecode': 'b55a87a8bbabbb5ffd44c6e17f7273c9c22fea6a47a03d58a49b45160bcbc3f6',
}

def checked(document):
    if len(document) > 65536:
        raise ValueError('wrapper artifact size')
    value = json.loads(document)
    result = {'sourceAddress': ADDRESS, 'source': SOURCE, 'sourceVerified': False}
    for field, expected in HASHES.items():
        data = value[field]
        if not isinstance(data, str) or not data.startswith('0x') or len(data) % 2:
            raise ValueError('wrapper byte encoding')
        raw = bytes.fromhex(data[2:])
        if hashlib.sha256(raw).hexdigest() != expected:
            raise ValueError('wrapper bytecode pin mismatch')
        result[field] = '0x' + raw.hex()
    result['creationSha256'] = HASHES['creation_bytecode']
    result['runtimeSha256'] = HASHES['deployed_bytecode']
    return result

if __name__ == '__main__':
    request = urllib.request.Request(SOURCE, headers={'User-Agent': 'quai-rust-sdk-qualification'})
    with urllib.request.urlopen(request, timeout=15) as response:
        artifact = checked(response.read(65537))
    output = pathlib.Path('/tmp/quai-wrapper-source/wquai.json')
    output.parent.mkdir(exist_ok=True)
    output.write_text(json.dumps(artifact, indent=2) + '\n')
    print('Saved hash-pinned observed WQUAI bytecode to', output)
