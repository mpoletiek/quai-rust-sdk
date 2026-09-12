#!/usr/bin/env python3
"""Read-only, bounded HTTP JSON-RPC qualification. Never signs or submits."""

import argparse
import datetime
import json
import math
import multiprocessing
import re
import urllib.error
import urllib.parse
import urllib.request

MAX_BYTES = 1024 * 1024
READ_METHODS = frozenset({"quai_chainId", "quai_blockNumber", "quai_getHeaderByNumber",
                          "quai_getBalance", "quai_getOutpointsByAddress"})
QUANTITY = re.compile(r"0x(?:0|[1-9a-f][0-9a-f]*)\Z")
HASH = re.compile(r"0x[0-9a-fA-F]{64}\Z")
ADDRESS = re.compile(r"0x[0-9a-fA-F]{40}\Z")


class ProbeError(Exception):
    """An intentionally sanitized diagnostic, safe to include in reports."""


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ProbeError("http_redirect_rejected")


def validate_url(value):
    try:
        url = urllib.parse.urlsplit(value)
        if (url.scheme not in ("http", "https") or not url.hostname or
                url.username is not None or url.password is not None or
                url.fragment or any(ord(c) < 33 or ord(c) > 126 for c in value)):
            raise ValueError()
        _ = url.port
    except ValueError:
        raise ProbeError("invalid_endpoint: HTTP(S), ASCII, no userinfo or fragment required") from None
    return value


def endpoint_label(value):
    """Never disclose path, query, or userinfo, even in an exception."""
    url = urllib.parse.urlsplit(validate_url(value))
    host = f"[{url.hostname}]" if ":" in url.hostname else url.hostname
    port = f":{url.port}" if url.port is not None else ""
    return f"{url.scheme}://{host}{port}/<redacted>"


def quantity(value):
    if not isinstance(value, str) or not QUANTITY.fullmatch(value) or len(value) > 66:
        raise ProbeError("invalid_hex_quantity")
    return int(value, 16)


def decode_response(body, request_id):
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ProbeError("duplicate_json_key")
            result[key] = value
        return result

    def invalid_constant(_):
        raise ProbeError("invalid_json_constant")

    try:
        value = json.loads(body, object_pairs_hook=unique_object, parse_constant=invalid_constant)
    except (ValueError, UnicodeError, RecursionError):
        raise ProbeError("invalid_json") from None
    if (not isinstance(value, dict) or value.get("jsonrpc") != "2.0" or
            type(value.get("id")) is not int or value["id"] != request_id or
            (("result" in value) == ("error" in value))):
        raise ProbeError("invalid_rpc_envelope")
    if "error" in value:
        error = value["error"]
        if (not isinstance(error, dict) or type(error.get("code")) is not int or
                not isinstance(error.get("message"), str)):
            raise ProbeError("invalid_rpc_error")
        # Server messages/data may echo URL credentials or supplied addresses.
        raise ProbeError(f"rpc_error_code_{error['code']}")
    return value["result"]


def _worker(connection, url, method, params, timeout):
    try:
        payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method,
                              "params": params}).encode("utf-8")
        request = urllib.request.Request(url, data=payload, headers={
            "Content-Type": "application/json", "Accept": "application/json",
            "User-Agent": "quai-rust-sdk-readiness/0.1"})
        # Do not inherit proxy credentials or route loopback through a proxy.
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        with opener.open(request, timeout=timeout) as response:
            if response.status != 200:
                raise ProbeError(f"http_status_{response.status}")
            if response.headers.get("Content-Encoding", "identity") != "identity":
                raise ProbeError("encoded_http_body_rejected")
            body = response.read(MAX_BYTES + 1)
        if len(body) > MAX_BYTES:
            raise ProbeError("http_body_too_large")
        connection.send((True, decode_response(body, 1)))
    except urllib.error.HTTPError as error:
        connection.send((False, f"http_status_{error.code}"))
    except ProbeError as error:
        connection.send((False, str(error)))
    except (OSError, ValueError, urllib.error.URLError):
        connection.send((False, "network_or_tls_error"))
    finally:
        connection.close()


def rpc(url, method, params, timeout=10.0):
    validate_url(url)
    if method not in READ_METHODS:
        raise ProbeError("method_not_in_read_only_allowlist")
    if not math.isfinite(timeout) or not 0 < timeout <= 30:
        raise ProbeError("timeout_out_of_range")
    # A separate process bounds DNS, connection, slow-body and JSON parsing time.
    context = multiprocessing.get_context("spawn")
    receive, send = context.Pipe(duplex=False)
    process = context.Process(target=_worker, args=(send, url, method, params, timeout))
    process.start()
    send.close()
    try:
        if not receive.poll(timeout):
            raise ProbeError("request_deadline_exceeded")
        try:
            ok, value = receive.recv()
        except EOFError:
            raise ProbeError("request_worker_failed") from None
        if not ok:
            raise ProbeError(value)
        return value
    finally:
        receive.close()
        process.join(timeout=0.1)
        if process.is_alive():
            process.terminate()
            process.join(timeout=1)
        if process.is_alive():
            process.kill()
            process.join(timeout=1)


def header_summary(value):
    if not isinstance(value, dict) or not isinstance(value.get("woHeader"), dict):
        raise ProbeError("invalid_pinned_header_shape")
    header = value["woHeader"]
    if not isinstance(header.get("hash"), str) or not HASH.fullmatch(header["hash"]):
        raise ProbeError("invalid_header_hash")
    if header.get("location") != "0x0000":
        raise ProbeError("wrong_zone_expected_cyprus1")
    return {"hash": header["hash"], "zone_height": quantity(header.get("number")),
            "prime_terminus_height": quantity(header.get("primeTerminusNumber")),
            "gas_limit": quantity(value.get("gasLimit")),
            "state_limit": quantity(value.get("stateLimit")),
            "expansion_number": quantity(value.get("expansionNumber"))}


def validate_outpoints(value):
    if not isinstance(value, list):
        raise ProbeError("invalid_outpoints")
    outpoints = set()
    for item in value:
        if not isinstance(item, dict) or not isinstance(item.get("txHash"), str) or not HASH.fullmatch(item["txHash"]):
            raise ProbeError("invalid_outpoint_hash")
        index = quantity(item.get("index"))
        if index > 65535 or quantity(item.get("denomination")) > 14:
            raise ProbeError("invalid_outpoint_bounds")
        quantity(item.get("lock"))
        key = (item["txHash"].lower(), index)
        if key in outpoints:
            raise ProbeError("duplicate_outpoint")
        outpoints.add(key)
    return outpoints


def probe(url, expected_chain_id, timeout=10.0, quai_address=None, qi_address=None,
          expected_outpoint=None):
    report = {"observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "endpoint": endpoint_label(url), "routing": "direct_preserve_supplied_url",
              "expected_chain_id": expected_chain_id, "readiness": "failed",
              "transaction_acceptance": "unproven", "index_readiness": "unproven"}
    try:
        chain = quantity(rpc(url, "quai_chainId", [], timeout))
        report["chain_id"] = chain
        if chain != expected_chain_id:
            raise ProbeError("chain_id_mismatch")
        height = quantity(rpc(url, "quai_blockNumber", [], timeout))
        # Pin the header request to the height just observed, avoiding a latest race.
        header = header_summary(rpc(url, "quai_getHeaderByNumber", [hex(height)], timeout))
        if header["zone_height"] != height:
            raise ProbeError("header_height_mismatch")
        report["header"] = header
        report["execution_limits_nonzero"] = header["gas_limit"] > 0 and header["state_limit"] > 0
        report["readiness"] = "reads_validated"
        if quai_address:
            balance = quantity(rpc(url, "quai_getBalance", [quai_address, hex(height)], timeout))
            report["account_balance_positive"] = balance > 0
        if qi_address:
            outpoints = validate_outpoints(rpc(url, "quai_getOutpointsByAddress", [qi_address], timeout))
            report["outpoint_count"] = len(outpoints)
            if expected_outpoint:
                if expected_outpoint not in outpoints:
                    raise ProbeError("expected_outpoint_missing")
                report["index_readiness"] = "known_outpoint_observed_current_head"
        # A validated response does not establish deployed node version or active rules.
        report["fork_qualification"] = "requires_verified_node_profile"
    except ProbeError as error:
        report["readiness"] = "failed"
        report["error"] = str(error)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True, help="Exact HTTP(S) endpoint; no paths are appended")
    parser.add_argument("--expected-chain-id", required=True, type=int)
    parser.add_argument("--timeout", type=float, default=10.0, help="Per-request wall deadline, maximum 30 seconds")
    parser.add_argument("--quai-address", help="Optional public test account for historical balance check")
    parser.add_argument("--qi-address", help="Optional public test Qi address for current-head index check")
    parser.add_argument("--expected-outpoint", help="Known Qi txHash:index; index is decimal")
    args = parser.parse_args()
    try:
        validate_url(args.url)
        if not 0 < args.expected_chain_id < 2**256:
            raise ProbeError("invalid_expected_chain_id")
        if not math.isfinite(args.timeout) or not 0 < args.timeout <= 30:
            raise ProbeError("timeout_out_of_range")
        for address, qi in ((args.quai_address, False), (args.qi_address, True)):
            if address and (not ADDRESS.fullmatch(address) or int(address[2:4], 16) != 0 or
                            bool(int(address[4:6], 16) & 128) != qi):
                raise ProbeError("invalid_address_for_cyprus1_ledger")
        expected = None
        if args.expected_outpoint:
            tx_hash, separator, index = args.expected_outpoint.partition(":")
            if not args.qi_address or not separator or not HASH.fullmatch(tx_hash) or not index.isascii() or not index.isdecimal() or len(index) > 5 or int(index) > 65535:
                raise ProbeError("invalid_expected_outpoint")
            expected = (tx_hash.lower(), int(index))
        report = probe(args.url, args.expected_chain_id, args.timeout,
                       args.quai_address, args.qi_address, expected)
    except ProbeError as error:
        report = {"readiness": "failed", "error": str(error)}
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["readiness"] == "reads_validated" else 1


if __name__ == "__main__":
    raise SystemExit(main())
