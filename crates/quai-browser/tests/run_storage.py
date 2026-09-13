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


def remove_profile(profile):
    # Chromium children can still finish writes or unlink temporary files after
    # the parent exits. Retry only this owned path for both observed race forms;
    # permission failures and an exhausted retry budget remain real failures.
    for attempt in range(20):
        try:
            shutil.rmtree(profile)
            return
        except OSError as error:
            if error.errno == errno.ENOENT and not pathlib.Path(profile).exists():
                return
            if error.errno not in (errno.ENOTEMPTY, errno.ENOENT) or attempt == 19:
                raise
            time.sleep(0.1)


def main():
    root = pathlib.Path(__file__).resolve().parents[1]
    finished = threading.Event()
    results = []
    progress = []

    class Fixture(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            length = int(self.headers.get('Content-Length', '0'))
            if self.path not in ('/result', '/progress') or not 0 < length < 1024:
                self.send_error(400)
                return
            value = self.rfile.read(length).decode()
            if self.path == '/progress':
                progress.append(value)
                del progress[:-16]
            else:
                results.append(value)
            self.send_response(204)
            self.end_headers()
            if self.path == '/result':
                finished.set()

    browser = shutil.which('chromium') or shutil.which('google-chrome')
    if not browser:
        raise RuntimeError('Chromium/Chrome required')
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Fixture, directory=str(root)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix='quai-idb-browser-') as profile:
            args = [browser, '--headless', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage', '--no-first-run', '--user-data-dir=' + profile, f'http://127.0.0.1:{server.server_port}/tests/storage.html']
            with tempfile.TemporaryFile(mode='w+b') as errors, subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=errors) as process:
                try:
                    # Include cold browser startup on shared CI runners, while
                    # keeping a finite total deadline and detecting early exit.
                    deadline = time.monotonic() + 120
                    while not finished.wait(1):
                        if process.poll() is not None:
                            raise RuntimeError(f'IndexedDB browser exited: {process.returncode}')
                        if time.monotonic() >= deadline:
                            raise RuntimeError(f'IndexedDB fixture timed out; progress={progress}')
                    assert results == ['PASSED'], results
                    print('IndexedDB Chromium: concurrent CAS, scope, bounds, tombstones and reopen PASSED')
                except BaseException:
                    errors.seek(0, 2)
                    errors.seek(max(0, errors.tell() - 4096))
                    print('Disposable browser stderr tail:', errors.read().decode(errors='replace'))
                    raise
                finally:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
            remove_profile(profile)
    finally:
        server.shutdown()
        server.server_close()


if __name__ == '__main__':
    main()
