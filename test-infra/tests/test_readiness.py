import contextlib
import http.server
import json
from pathlib import Path
import sys
import threading
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import readiness


HEADER = {"gasLimit": "0x100", "stateLimit": "0x100", "expansionNumber": "0x0",
          "woHeader": {"hash": "0x" + "ab" * 32, "number": "0x10",
                       "primeTerminusNumber": "0x4", "location": "0x0000"}}


class ValidationTests(unittest.TestCase):
    def test_strict_quantities(self):
        self.assertEqual(readiness.quantity("0x3a98"), 15000)
        for value in (True, 1, None, "0x", "0x01", "0X1", "0xA", "-1", "0x" + "f" * 65):
            with self.subTest(value=value), self.assertRaises(readiness.ProbeError):
                readiness.quantity(value)

    def test_envelope_validation(self):
        for value in ({"jsonrpc": "2.0", "id": True, "result": 1},
                      {"jsonrpc": "2.0", "id": 2, "result": 1},
                      {"jsonrpc": "2.0", "id": 1, "result": 1, "error": {}},
                      {"jsonrpc": "2.0", "id": 1}, []):
            with self.subTest(value=value), self.assertRaises(readiness.ProbeError):
                readiness.decode_response(json.dumps(value), 1)
        self.assertIsNone(readiness.decode_response('{"jsonrpc":"2.0","id":1,"result":null}', 1))

    def test_duplicate_and_non_json_numbers_rejected(self):
        for text in ('{"jsonrpc":"2.0","id":1,"id":2,"result":null}',
                     '{"jsonrpc":"2.0","id":1,"result":NaN}'):
            with self.assertRaises(readiness.ProbeError):
                readiness.decode_response(text, 1)

    def test_error_message_is_redacted(self):
        with self.assertRaisesRegex(readiness.ProbeError, r"^rpc_error_code_-32000$"):
            readiness.decode_response(json.dumps({"jsonrpc": "2.0", "id": 1, "error": {
                "code": -32000, "message": "secret path/token", "data": "private"}}), 1)

    def test_urls_preserved_but_redacted(self):
        value = "https://example.com:9443/custom/cyprus1?token=secret"
        self.assertEqual(readiness.validate_url(value), value)
        self.assertEqual(readiness.endpoint_label(value), "https://example.com:9443/<redacted>")
        for value in ("ftp://example.com", "https://user:pass@example.com", "https://example.com/#secret",
                      "https://example.com:invalid", "https://example.com/\nsecret"):
            with self.assertRaises(readiness.ProbeError):
                readiness.validate_url(value)

    def test_write_method_rejected_without_network(self):
        with self.assertRaisesRegex(readiness.ProbeError, "read_only_allowlist"):
            readiness.rpc("http://127.0.0.1:1", "quai_sendRawTransaction", [])

    def test_chain_mismatch_stops_before_other_calls(self):
        with patch.object(readiness, "rpc", return_value="0x1") as rpc:
            result = readiness.probe("http://127.0.0.1:9200", 1337)
        self.assertEqual(result["error"], "chain_id_mismatch")
        self.assertEqual(rpc.call_count, 1)

    def test_readiness_does_not_claim_tx_or_index_acceptance(self):
        with patch.object(readiness, "rpc", side_effect=["0x539", "0x10", HEADER, []]) as rpc:
            result = readiness.probe("http://127.0.0.1:9200", 1337,
                                     qi_address="0x0080" + "00" * 18)
        self.assertEqual(result["readiness"], "reads_validated")
        self.assertEqual(result["transaction_acceptance"], "unproven")
        self.assertEqual(result["index_readiness"], "unproven")
        self.assertEqual(rpc.call_args_list[2].args[2], ["0x10"])

    def test_wrong_header_height_rejected(self):
        with patch.object(readiness, "rpc", side_effect=["0x539", "0x11", HEADER]):
            result = readiness.probe("http://127.0.0.1:9200", 1337)
        self.assertEqual(result["error"], "header_height_mismatch")

    def test_outpoint_witness_required_and_bounds_checked(self):
        item = {"txHash": "0x" + "ab" * 32, "index": "0x1", "denomination": "0xe", "lock": "0x0"}
        witness = (item["txHash"], 1)
        with patch.object(readiness, "rpc", side_effect=["0x539", "0x10", HEADER, [item]]):
            result = readiness.probe("http://127.0.0.1:9200", 1337,
                                     qi_address="0x0080" + "00" * 18, expected_outpoint=witness)
        self.assertEqual(result["index_readiness"], "known_outpoint_observed_current_head")
        for items in ([item, item], [{**item, "denomination": "0xf"}], [{**item, "index": "0x10000"}]):
            with self.assertRaises(readiness.ProbeError):
                readiness.validate_outpoints(items)


@contextlib.contextmanager
def server(mode):
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            self.server.path_seen = self.path
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            self.server.request_seen = request
            if mode == "slow":
                time.sleep(2)
            self.send_response(302 if mode == "redirect" else 200)
            if mode == "redirect":
                self.send_header("Location", "http://127.0.0.1:1/private")
            self.end_headers()
            data = (b" " * (readiness.MAX_BYTES + 1) if mode == "large" else
                    json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": "0x539"}).encode())
            try:
                self.wfile.write(data)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def log_message(self, *args):
            pass

    instance = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=instance.serve_forever, daemon=True)
    worker.start()
    try:
        yield instance, f"http://127.0.0.1:{instance.server_port}/custom?test=public"
    finally:
        instance.shutdown()
        instance.server_close()
        worker.join(timeout=2)


class HttpTests(unittest.TestCase):
    def test_actual_http_preserves_path_and_request(self):
        with server("ok") as (instance, url):
            self.assertEqual(readiness.rpc(url, "quai_chainId", [], 3), "0x539")
            self.assertEqual(instance.path_seen, "/custom?test=public")
            self.assertEqual(instance.request_seen["method"], "quai_chainId")

    def test_oversize_response_rejected(self):
        with server("large") as (_, url), self.assertRaisesRegex(readiness.ProbeError, "too_large"):
            readiness.rpc(url, "quai_chainId", [], 3)

    def test_redirect_rejected(self):
        with server("redirect") as (_, url), self.assertRaisesRegex(readiness.ProbeError, "redirect_rejected"):
            readiness.rpc(url, "quai_chainId", [], 3)

    def test_wall_deadline(self):
        with server("slow") as (_, url):
            start = time.monotonic()
            with self.assertRaises(readiness.ProbeError):
                readiness.rpc(url, "quai_chainId", [], 0.5)
            self.assertLess(time.monotonic() - start, 1.5)


if __name__ == "__main__":
    unittest.main()
