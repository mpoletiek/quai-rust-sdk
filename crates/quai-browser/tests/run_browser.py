#!/usr/bin/env python3
"""Run browser wasm tests against an isolated public-data-only loopback RPC fixture."""
import http.server
import gzip
import websocket_fixture
import argparse
import json
import os
import pathlib
import subprocess
import threading
import time

class Fixture(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *_args): pass
    def cors(self):
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Headers', 'content-type,x-public')
        self.send_header('Access-Control-Expose-Headers', 'content-encoding,location,retry-after')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, HEAD, OPTIONS')
    def do_OPTIONS(self):
        self.send_response(204); self.cors(); self.send_header('Content-Length', '0'); self.end_headers()
    def do_GET(self):
        if self.path.startswith('/resource/'):
            self.resource()
        elif self.path.startswith('/socket/'):
            websocket_fixture.serve(self)
        else:
            self.send_error(404)

    def do_POST(self):
        if self.path.startswith('/resource/'):
            self.resource(); return
        length = int(self.headers.get('Content-Length', '0'))
        if not 0 < length < 65536:
            self.send_error(400); return
        request = json.loads(self.rfile.read(length))
        if self.path in ('/account', '/account-reorg', '/account-preflight', '/qi', '/qi-hints'):
            result = self.account_result(request)
            if result is None:
                self.send_error(400); return
            body = json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}).encode()
            self.send_response(200); self.cors(); self.send_header('Content-Type','application/json')
            self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
            return
        if request.get('method') != 'quai_chainId' or request.get('params') != []:
            self.send_error(400); return
        if self.path == '/slow': time.sleep(0.3)
        if self.path == '/redirect':
            self.send_response(302); self.cors(); self.send_header('Location','/prefix/cyprus1?token=PUBLIC'); self.send_header('Content-Length','0'); self.end_headers(); return
        request_id = request['id'] + (1 if self.path == '/wrong-id' else 0)
        body = json.dumps({'jsonrpc':'2.0','id':request_id,'result':'0x3a98'}).encode()
        if self.path == '/duplicate-id': body = ('{"jsonrpc":"2.0","id":%d,"id":%d,"result":null}' % (request_id,request_id)).encode()
        if self.path == '/oversize': body = b'x' * 1000
        self.send_response(200); self.cors(); self.send_header('Content-Type','application/json')
        if self.path != '/oversize': self.send_header('Content-Length',str(len(body)))
        else: self.send_header('Connection','close')
        self.end_headers()
        try: self.wfile.write(body)
        except (BrokenPipeError,ConnectionResetError): pass
        if self.path == '/oversize': self.close_connection=True

    def resource(self):
        body = b'{"ok":true}'
        status = 200
        encoding = None
        if self.path == '/resource/echo':
            size = int(self.headers.get('Content-Length', '0'))
            if not 0 <= size <= 1048576:
                self.send_error(400); return
            body = self.rfile.read(size)
        if self.path == '/resource/error':
            status, body = 418, b'PUBLIC error'
        if self.path == '/resource/gzip':
            body, encoding = gzip.compress(body), 'gzip'
        if self.path == '/resource/bomb':
            body, encoding = gzip.compress(b'x' * 8192), 'gzip'
        if self.path == '/resource/oversize': body = b'x' * 2048
        if self.path == '/resource/slow': time.sleep(0.2)
        if self.path == '/resource/redirect': status, body = 302, b''
        self.send_response(status); self.cors()
        if status == 302: self.send_header('Location', '/resource/json')
        if encoding: self.send_header('Content-Encoding', encoding)
        self.send_header('Content-Type', 'application/octet-stream')
        self.send_header('Content-Length', str(len(body))); self.end_headers()
        try: self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError): pass

    def account_result(self, request):
        method, params = request.get('method'), request.get('params')
        if not isinstance(params, list): return None
        if self.path == '/account-preflight':
            if method == 'quai_chainId' and params == []: return '0x9'
            if method == 'quai_getHeaderByNumber' and params in [['0x0'], ['latest']]:
                if params == ['0x0']:
                    return {'woHeader':{'hash':'0x'+'01'*32,'number':'0x0','location':'0x','parentHash':'0x'+'00'*32}}
                return {'woHeader':{'hash':'0x'+'02'*32,'number':'0x10','location':'0x0000','parentHash':'0x'+'01'*32,'primeTerminusNumber':'0x4'},'gasLimit':'0x100000','stateLimit':'0x100000'}
            if method == 'quai_gasPrice' and params == []: return '0x2'
            if method in ('quai_getBalance','quai_getTransactionCount') and len(params) == 2 and params[1] == '0x10':
                return '0xf4240' if method == 'quai_getBalance' else '0x5'
            if method == 'quai_estimateGas' and len(params) == 2 and params[1] == '0x10' and params[0].get('nonce') == '0x8' and params[0].get('input') == '0x0102':
                return '0x5209'
            return None
        genesis = '0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b'
        if method == 'quai_chainId' and params == []: return '0x3a98'
        if self.path == '/qi-hints' and method == 'quai_getOutpointsByAddress' and len(params) == 1:
            return []
        if self.path == '/qi' and method == 'quai_getOutpointsByAddress' and len(params) == 1:
            count = getattr(self.server, 'qi_reads', 0)
            self.server.qi_reads = count + 1
            return [] if count else [{'txHash':'0x00000080'+'00'*28,'index':'0x0','denomination':'0x2','lock':'0x65'}]
        if method == 'quai_getHeaderByNumber' and params in [['0x0'], ['latest'], ['0x64']]:
            if params == ['0x0']:
                return {'woHeader':{'hash':genesis,'number':'0x0','location':'0x','parentHash':'0x'+'00'*32}}
            changed = self.path == '/account-reorg' and getattr(self.server, 'account_changed', False)
            return {'woHeader':{'hash':'0x'+('22' if changed else '11')*32,'number':'0x64','location':'0x0000','parentHash':genesis,'primeTerminusNumber':'0x32'},'gasLimit':'0x100000','stateLimit':'0x100000'}
        if method in ('quai_getBalance', 'quai_getTransactionCount') and len(params) == 2 and params[1] == '0x64':
            if method == 'quai_getBalance':
                if self.path == '/account-reorg': self.server.account_changed = True
                return '0x100'
            return '0x5'
        return None

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--suite', choices=['sdk-final-utilities', 'sdk-aggregation', 'sdk-curve', 'sdk-utilities', 'sdk-resources', 'browser', 'worker', 'sdk-contract-io', 'sdk-transaction-confirmations', 'sdk-transaction-documents', 'sdk-wordlists', 'sdk-rpc-signer', 'sdk-response-views', 'sdk-provider-events', 'account_wait', 'sdk-legacy-wallets', 'sdk-address-book', 'sdk-worker', 'sdk-contracts', 'sdk-events', 'sdk-keys', 'sdk-backups', 'sdk-allocations', 'sdk-payment-allocations', 'sdk-human-abi', 'sdk-receipts', 'sdk-account-custody', 'sdk-account-backup', 'sdk-contract-code', 'sdk-qi-custody', 'sdk-portable-capture', 'sdk-allocation-merge', 'sdk-account-preflight', 'sdk-recovery', 'sdk-qi-preflight'], default='browser')
    arguments = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[3]
    server = http.server.ThreadingHTTPServer(('127.0.0.1',0), Fixture)
    thread = threading.Thread(target=server.serve_forever,daemon=True); thread.start()
    env = os.environ.copy()
    env.setdefault('CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER', 'wasm-bindgen-test-runner')
    env['QUAI_BROWSER_FIXTURE_URL'] = 'http://127.0.0.1:%d' % server.server_address[1]
    env.setdefault('WASM_BINDGEN_USE_BROWSER','1')
    env.setdefault('WASM_BINDGEN_TEST_TIMEOUT','30')
    env.setdefault('CHROMEDRIVER','/usr/bin/chromedriver')
    env.setdefault('WASM_BINDGEN_TEST_WEBDRIVER_JSON',str(pathlib.Path(__file__).with_name('webdriver.json')))
    try:
        sdk_suites = {'sdk-final-utilities': ('final_utilities', 'wallet,abi,browser'),'sdk-aggregation': ('aggregation', 'wallet,browser'),'sdk-curve': ('curve', 'wallet,browser'),'sdk-utilities': ('utility_completion', 'wallet,keystore,browser'),'sdk-resources': ('resources', 'browser'),'sdk-contract-io': ('contract_io', 'wallet,abi,browser'),'sdk-transaction-confirmations': ('transaction_confirmation', 'abi,browser'),'sdk-transaction-documents': ('transaction_documents', 'abi,browser'),'sdk-wordlists': ('wordlists', 'wallet,browser'),
                      'sdk-provider-events': ('provider_events', 'browser'),
                      'sdk-response-views': ('response_views', 'browser'),
                      'sdk-rpc-signer': ('rpc_signer', 'wallet,browser'),
                      'sdk-legacy-wallets': ('legacy_wallets', 'backup,browser'),
                      'sdk-address-book': ('qi_address_book', 'wallet,payments,browser'),
                      'sdk-worker': ('browser_discovery', 'wallet,browser'),
                      'sdk-contracts': ('contracts', 'wallet,browser,abi'),
                      'sdk-events': ('events', 'wallet,browser,abi'),
                      'sdk-keys': ('key_origins', 'wallet,browser,payments'),
                      'sdk-backups': ('portable_backups', 'backup,browser,payments'),
                      'sdk-allocations': ('address_allocation', 'backup,browser'),
                      'sdk-payment-allocations': ('payment_allocation', 'backup,browser,payments'),
                      'sdk-human-abi': ('human_abi', 'abi,browser'),
                      'sdk-receipts': ('receipt_confirmation', 'browser'),
                      'sdk-account-custody': ('account_custody', 'backup,browser'),
                      'sdk-account-backup': ('account_backup', 'backup,browser'),
                      'sdk-contract-code': ('contract_code', 'abi,browser'),
                      'sdk-qi-custody': ('qi_custody', 'backup,browser'),
                      'sdk-portable-capture': ('portable_capture', 'backup,browser'),
                      'sdk-allocation-merge': ('allocation_merge', 'backup,browser'),
                      'sdk-account-preflight': ('account_preflight', 'backup,browser,abi'),
                      'sdk-recovery': ('browser_recovery', 'backup,browser'),
                      'sdk-qi-preflight': ('qi_preflight', 'backup,browser')}
        sdk_suite = sdk_suites.get(arguments.suite)
        command = ['cargo','test','-p','quai-sdk' if sdk_suite else 'quai-browser','--target','wasm32-unknown-unknown','--test',sdk_suite[0] if sdk_suite else arguments.suite,'--offline','--locked']
        if sdk_suite: command += ['--no-default-features','--features',sdk_suite[1]]
        result = subprocess.run(command,cwd=root,env=env,timeout=240)
        raise SystemExit(result.returncode)
    finally: server.shutdown(); server.server_close()
