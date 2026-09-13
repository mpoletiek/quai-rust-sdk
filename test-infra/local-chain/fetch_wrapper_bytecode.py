#!/usr/bin/env python3
"""Fetch hash-pinned public WQUAI/WQI bytecode for isolated acceptance, without submitting RPCs."""
import argparse
import hashlib
import json
import pathlib
import urllib.request

PROFILES = {
    'wquai': ('0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB', {
        'creation_bytecode': 'e670f177b98bb8bd4653a78f295308e31fe70e9ec30b396fd4bbb7d1be1518a8',
        'deployed_bytecode': 'b55a87a8bbabbb5ffd44c6e17f7273c9c22fea6a47a03d58a49b45160bcbc3f6',
    }),
    'wqi': ('0x002b2596EcF05C93a31ff916E8b456DF6C77c750', {
        'creation_bytecode': '8add7fd6b6a88b107b95eb04ede0872388f2a6634a00e58b7741f295a15b7cdc',
        'deployed_bytecode': 'bc933c0661d3995cdcf53db5bf0424778a2b35b11a8c5a08999f53d33cf08df2',
    }),
}

def source(address):
    return 'https://quaiscan.io/api/v2/smart-contracts/' + address

def checked(document, profile="wquai"):
    address, hashes = PROFILES[profile]
    if len(document) > 65536:
        raise ValueError('wrapper artifact size')
    value = json.loads(document)
    result = {'sourceAddress': address, 'source': source(address), 'sourceVerified': False}
    for field, expected in hashes.items():
        data = value[field]
        if not isinstance(data, str) or not data.startswith('0x') or len(data) % 2:
            raise ValueError('wrapper byte encoding')
        raw = bytes.fromhex(data[2:])
        if hashlib.sha256(raw).hexdigest() != expected:
            raise ValueError('wrapper bytecode pin mismatch')
        result[field] = '0x' + raw.hex()
    result['creationSha256'] = hashes['creation_bytecode']
    result['runtimeSha256'] = hashes['deployed_bytecode']
    return result

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wrapper', choices=PROFILES, default='wquai')
    args = parser.parse_args()
    request = urllib.request.Request(source(PROFILES[args.wrapper][0]), headers={'User-Agent': 'quai-rust-sdk-qualification'})
    with urllib.request.urlopen(request, timeout=15) as response:
        artifact = checked(response.read(65537), args.wrapper)
    output = pathlib.Path('/tmp/quai-wrapper-source') / (args.wrapper + '.json')
    output.parent.mkdir(exist_ok=True)
    output.write_text(json.dumps(artifact, indent=2) + '\n')
    print('Saved hash-pinned observed', args.wrapper.upper(), 'bytecode to', output)
