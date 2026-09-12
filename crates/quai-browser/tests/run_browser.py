#!/usr/bin/env python3
"""Run browser wasm tests against an isolated public-data-only loopback RPC fixture."""
import http.server
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
        self.send_header('Access-Control-Allow-Headers', 'content-type')
        self.send_header('Access-Control-Allow-Methods', 'POST, OPTIONS')
    def do_OPTIONS(self):
        self.send_response(204); self.cors(); self.send_header('Content-Length', '0'); self.end_headers()
    def do_POST(self):
        length = int(self.headers.get('Content-Length', '0'))
        if not 0 < length < 65536:
            self.send_error(400); return
        request = json.loads(self.rfile.read(length))
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

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--suite', choices=['browser', 'worker'], default='browser')
    arguments = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[3]
    server = http.server.ThreadingHTTPServer(('127.0.0.1',0), Fixture)
    thread = threading.Thread(target=server.serve_forever,daemon=True); thread.start()
    env = os.environ.copy()
    env['QUAI_BROWSER_FIXTURE_URL'] = 'http://127.0.0.1:%d' % server.server_address[1]
    env.setdefault('WASM_BINDGEN_USE_BROWSER','1')
    env.setdefault('WASM_BINDGEN_TEST_TIMEOUT','30')
    env.setdefault('CHROMEDRIVER','/usr/bin/chromedriver')
    env.setdefault('WASM_BINDGEN_TEST_WEBDRIVER_JSON',str(pathlib.Path(__file__).with_name('webdriver.json')))
    try:
        result = subprocess.run(['cargo','test','-p','quai-browser','--target','wasm32-unknown-unknown','--test',arguments.suite,'--offline'],cwd=root,env=env,timeout=240)
        raise SystemExit(result.returncode)
    finally: server.shutdown(); server.server_close()
