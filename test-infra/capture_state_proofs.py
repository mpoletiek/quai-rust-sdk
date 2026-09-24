#!/usr/bin/env python3
"""Capture read-only `quai_getProof` vectors from a zone endpoint at one block.

Every read names the same block by hash, so the header and the proofs describe
one state. The output holds public RPC results only: no keys, no submissions.
Run it again to refresh the vectors; tests assert shapes and verify the proofs,
never a particular balance.

    python3 test-infra/capture_state_proofs.py [endpoint]
"""
import datetime
import json
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'test-infra/fixtures/state-proofs-mainnet.json'
ENDPOINT = sys.argv[1] if len(sys.argv) > 1 else 'https://rpc.quai.network/cyprus1'
# WQUAI: a contract with storage. The absent address had no state when chosen.
CONTRACT = '0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB'
ABSENT = '0x002360Bc8E2A359bE7335B06De43F1c7F040f15a'
# Slots 0-3 hold values; the last is a key no contract writes, so it proves absence.
SLOTS = ['0x' + f'{n:064x}' for n in range(4)] + ['0x' + 'ab' * 32]


def call(method, params):
    body = json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params}).encode()
    request = urllib.request.Request(ENDPOINT, body, {'content-type': 'application/json'})
    with urllib.request.urlopen(request, timeout=60) as response:
        reply = json.loads(response.read())
    if 'error' in reply:
        raise SystemExit(f'{method}: {reply["error"]}')
    return reply['result']


def main():
    chain_id = call('quai_chainId', [])
    header = call('quai_getHeaderByNumber', ['latest'])
    at = {'blockHash': header['woHeader']['hash']}
    # The block's miner: an account with a balance and no code.
    funded = header['woHeader']['primaryCoinbase']
    requests = [
        (CONTRACT, SLOTS),
        (ABSENT, [SLOTS[0]]),
        (funded, []),
    ]
    proofs = [
        {'request': {'method': 'quai_getProof', 'params': [address, slots, at]},
         'result': call('quai_getProof', [address, slots, at])}
        for address, slots in requests
    ]
    fixture = {
        'schemaVersion': 1,
        'purpose': 'Read-only quai_getProof vectors at one block, for the state-proof verifier',
        'capturedAt': datetime.datetime.now(datetime.UTC).strftime('%Y-%m-%dT%H:%M:%SZ'),
        'endpoint': ENDPOINT,
        'chainId': chain_id,
        'clientVersion': call('quai_clientVersion', []),
        'genesis': call('quai_getHeaderByNumber', ['0x0']),
        'header': header,
        'proofs': proofs,
        'scope': 'Public state at one block. Balances and storage change; tests verify '
                 'proofs against this header and never treat a value as current.',
    }
    OUT.write_text(json.dumps(fixture, indent=2) + '\n')
    print(f'wrote {OUT.relative_to(ROOT)} at block {int(header["woHeader"]["number"], 16)}')


if __name__ == '__main__':
    main()
