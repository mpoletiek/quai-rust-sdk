#!/usr/bin/env python3
"""Capture zone headers whose hashes the SDK recomputes, from a public endpoint.

v1 `quai_getHeaderByNumber` results cover the genesis, blocks before the
KawPoW fork (full block hash recomputable) and the head (header hash only).
One `newHeadsV2` work object, trimmed to its two headers, covers the
post-fork block hash, which needs the v2 `auxpow`. Public RPC results only.

    pip install websockets   # for the v2 capture
    python3 test-infra/capture_header_hashes.py [https-endpoint]
"""
import asyncio
import datetime
import json
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'test-infra/fixtures/header-hashes-mainnet.json'
ENDPOINT = sys.argv[1] if len(sys.argv) > 1 else 'https://rpc.quai.network/cyprus1'
# Genesis; two ProgPoW-era blocks; the head, which is past the KawPoW fork.
HEIGHTS = ['0x0', hex(1_048_576), hex(3_000_000), 'latest']


def call(method, params):
    body = json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params}).encode()
    request = urllib.request.Request(ENDPOINT, body, {'content-type': 'application/json'})
    with urllib.request.urlopen(request, timeout=60) as response:
        reply = json.loads(response.read())
    if 'error' in reply:
        raise SystemExit(f'{method}: {reply["error"]}')
    return reply['result']


async def work_object_v2():
    import websockets

    url = ENDPOINT.replace('https://', 'wss://', 1)
    async with websockets.connect(url, max_size=None) as socket:
        subscribe = {'jsonrpc': '2.0', 'id': 1, 'method': 'quai_subscribe', 'params': ['newHeadsV2']}
        await socket.send(json.dumps(subscribe))
        await socket.recv()
        message = json.loads(await asyncio.wait_for(socket.recv(), 120))
    work = message['params']['result']
    return {'woHeader': work['woHeader'], 'woBody': {'header': work['woBody']['header']}}


def main():
    headers = [call('quai_getHeaderByNumber', [height]) for height in HEIGHTS]
    fixture = {
        'schemaVersion': 1,
        'purpose': 'Zone headers whose headerHash, seal hash and block hash the SDK recomputes',
        'capturedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds'),
        'endpoint': ENDPOINT,
        'clientVersion': call('quai_clientVersion', []),
        'v1': headers,
        'v2': asyncio.run(work_object_v2()),
    }
    OUT.write_text(json.dumps(fixture, indent=1) + '\n')
    print(f'wrote {OUT.relative_to(ROOT)}')


if __name__ == '__main__':
    main()
