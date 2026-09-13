#!/usr/bin/env python3
"""Run the IndexedDB CAS fixture in a disposable real headless browser profile."""
import functools
import errno
import http.server
import pathlib
import shutil
import subprocess
import tempfile
import threading
import time


def main():
    root = pathlib.Path(__file__).resolve().parents[1]
    finished = threading.Event()
    results = []

    class Fixture(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            length = int(self.headers.get('Content-Length', '0'))
            if self.path != '/result' or not 0 < length < 1024:
                self.send_error(400)
                return
            results.append(self.rfile.read(length).decode())
            self.send_response(204)
            self.end_headers()
            finished.set()

    browser = shutil.which('chromium') or shutil.which('google-chrome')
    if not browser:
        raise RuntimeError('Chromium/Chrome required')
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Fixture, directory=str(root)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix='quai-idb-browser-') as profile:
            args = [browser, '--headless', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage', '--no-first-run', '--user-data-dir=' + profile, f'http://127.0.0.1:{server.server_port}/tests/storage.html']
            with subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) as process:
                try:
                    if not finished.wait(40):
                        raise RuntimeError('IndexedDB fixture timed out')
                    assert results == ['PASSED'], results
                    print('IndexedDB Chromium: concurrent CAS, scope, bounds, tombstones and reopen PASSED')
                finally:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
            # Chromium's child processes can finish profile writes just after
            # the browser process exits. Retry only this owned temporary path.
            for attempt in range(20):
                try:
                    shutil.rmtree(profile)
                    break
                except OSError as error:
                    if error.errno != errno.ENOTEMPTY or attempt == 19:
                        raise
                    time.sleep(0.1)
    finally:
        server.shutdown()
        server.server_close()


if __name__ == '__main__':
    main()
