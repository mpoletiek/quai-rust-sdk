#!/usr/bin/env python3
"""Independent (no go-quai) recomputation of a Quai zone block hash from RPC JSON.

A byte-level reference for an SDK `verify_header_hash`: hand-rolled protobuf
encoding + BLAKE3, mirroring go-quai core/types Header.SealEncode,
WorkObjectHeader.SealEncode / SealHash / Hash, and AuxPow.ProtoEncode.

usage:
  quai_hash.py header <quai_getHeaderByNumber result JSON file>   (v1: headerHash; full hash only pre-KawPoW)
  quai_hash.py wo     <newHeadsV2 WorkObject JSON file>           (v2: headerHash, sealHash, hash)
"""
import json
import sys

from blake3 import blake3

KAWPOW_FORK_BLOCK = 1171500  # params.KawPowForkBlock (prime terminus number)


def b3(b: bytes) -> bytes:
    return blake3(b).digest()


# ---- minimal protobuf writer (wire types 0 and 2 only) ----
def varint(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def f_varint(num: int, v: int) -> bytes:  # optional uint64/uint32 set via pointer: always emitted, even 0
    return varint(num << 3 | 0) + varint(v)


def f_bytes(num: int, b: bytes) -> bytes:  # length-delimited, emitted even when empty (presence)
    return varint(num << 3 | 2) + varint(len(b)) + b


def proto_hash(h: bytes) -> bytes:  # common.ProtoHash{ bytes value = 1 } -- 32 bytes, always non-empty
    return f_bytes(1, h)


def hx(s: str) -> bytes:
    s = s[2:] if s.startswith("0x") else s
    if len(s) % 2:
        s = "0" + s
    return bytes.fromhex(s)


def big(s: str) -> bytes:
    """big.Int.Bytes(): minimal big-endian, zero -> b'' (still emitted as present-empty)."""
    n = int(s, 16)
    return n.to_bytes((n.bit_length() + 7) // 8, "big") if n else b""


def u(s: str) -> int:
    return int(s, 16)


def header_seal_bytes(h: dict) -> bytes:
    """proto.Marshal(Header.SealEncode()) -- block.go; fields in field-number order."""
    H = lambda k: proto_hash(hx(h[k]).rjust(32, b"\0"))
    out = b""
    for p in h["parentHash"]:  # 1 repeated ProtoHash (prime, region)
        out += f_bytes(1, proto_hash(hx(p)))
    out += f_bytes(2, H("uncleHash"))
    out += f_bytes(3, H("evmRoot"))
    out += f_bytes(4, H("transactionsRoot"))
    out += f_bytes(5, H("outboundEtxsRoot"))
    out += f_bytes(6, H("etxRollupRoot"))
    for m in h["manifestHash"]:  # 7 repeated ProtoHash (prime, region, zone)
        out += f_bytes(7, proto_hash(hx(m)))
    out += f_bytes(8, H("receiptsRoot"))
    # 9 difficulty: not in SealEncode
    for e in h["parentEntropy"]:
        out += f_bytes(10, big(e))
    for e in h["parentDeltaEntropy"]:
        out += f_bytes(11, big(e))
    for e in h["parentUncledDeltaEntropy"]:
        out += f_bytes(12, big(e))
    out += f_bytes(13, big(h["uncledEntropy"]))
    for n in h["number"]:  # 14 repeated bytes (prime, region)
        out += f_bytes(14, big(n))
    out += f_varint(15, u(h["gasLimit"]))
    out += f_varint(16, u(h["gasUsed"]))
    out += f_bytes(17, big(h["baseFeePerGas"]))
    # 18 location, 20 mix_hash, 21 nonce: not in SealEncode
    out += f_bytes(19, hx(h["extraData"]))  # present when non-nil (always, from JSON)
    out += f_bytes(22, H("utxoRoot"))
    out += f_bytes(23, H("etxSetRoot"))
    out += f_varint(24, u(h["efficiencyScore"]))
    out += f_varint(25, u(h["thresholdCount"]))
    out += f_varint(26, u(h["expansionNumber"]))
    out += f_bytes(27, H("etxEligibleSlices"))
    out += f_bytes(28, H("primeTerminusHash"))
    out += f_bytes(29, H("interlinkRootHash"))
    out += f_varint(30, u(h["stateLimit"]))
    out += f_varint(31, u(h["stateUsed"]))
    out += f_bytes(32, big(h["quaiStateSize"]))
    out += f_bytes(33, big(h["exchangeRate"]))
    out += f_bytes(35, big(h["avgTxFees"]))
    out += f_bytes(36, big(h["totalFees"]))
    out += f_bytes(37, big(h["kQuaiDiscount"]))
    out += f_bytes(38, big(h["conversionFlowAmount"]))
    out += f_bytes(39, big(h["minerDifficulty"]))
    out += f_bytes(40, H("primeStateRoot"))
    out += f_bytes(41, H("regionStateRoot"))
    return out


def share(d: dict) -> bytes:
    """PowShareDiffAndCount.ProtoEncode: zero is encoded as b'\\x00', not b''."""
    z = lambda s: big(s) or b"\x00"
    return f_bytes(1, z(d["difficulty"])) + f_bytes(2, z(d["count"])) + f_bytes(3, z(d["uncled"]))


def wo_seal_hash(w: dict) -> bytes:
    """WorkObjectHeader.SealHash() -- wo.go."""
    kawpow = u(w["primeTerminusNumber"]) >= KAWPOW_FORK_BLOCK
    out = b""
    out += f_bytes(1, proto_hash(hx(w["headerHash"])))
    out += f_bytes(2, proto_hash(hx(w["parentHash"])))
    out += f_bytes(3, big(w["number"]))
    out += f_bytes(4, big(w["difficulty"]))
    out += f_bytes(5, proto_hash(hx(w["txHash"])))
    out += f_bytes(7, f_bytes(1, hx(w["location"])) if hx(w["location"]) else b"")
    out += f_varint(9, u(w["timestamp"]))
    out += f_bytes(10, big(w["primeTerminusNumber"]))
    out += f_varint(11, u(w["lock"]))
    coinbase = hx(w["primaryCoinbase"])
    if not kawpow:
        out += f_bytes(12, f_bytes(1, coinbase))
    if hx(w["data"]):  # proto3 non-optional bytes: omitted when empty
        out += f_bytes(13, hx(w["data"]))
    if kawpow:
        out += f_bytes(15, share(w["scryptDiffAndCount"]))
        out += f_bytes(16, share(w["shaDiffAndCount"]))
        out += f_bytes(17, big(w["shaShareTarget"]))
        out += f_bytes(18, big(w["scryptShareTarget"]))
        out += f_bytes(19, big(w["kawpowDifficulty"]))
        return b3(b3(out) + coinbase)
    return b3(out)


def auxpow_bytes(a: dict) -> bytes:
    """proto.Marshal(AuxPow.ProtoEncode()) -- auxpow.go."""
    out = f_varint(1, u(a["powId"]))
    out += f_bytes(2, hx(a["header"]))
    out += f_bytes(3, hx(a["signature"]))
    for m in a["merkleBranch"]:
        out += f_bytes(4, hx(m))
    out += f_bytes(5, hx(a["transaction"]))
    # 6 signature_time: never set by ProtoEncode
    out += f_bytes(7, hx(a["auxpow2"]))  # present even when "0x"
    return out


def wo_hash(w: dict) -> bytes:
    kawpow = u(w["primeTerminusNumber"]) >= KAWPOW_FORK_BLOCK
    if kawpow and "auxpow" in w:
        return b3(auxpow_bytes(w["auxpow"]))
    if kawpow:
        # IsTransitionProgPowBlock (no auxpow, within the 4-week grace) would use the
        # progpow form below; after the transition an auxpow is mandatory.
        raise ValueError("post-KawPoW header without auxpow: need a v2 WorkObjectHeader")
    return b3(hx(w["mixHash"]) + wo_seal_hash(w) + hx(w["nonce"]))


def read_varint_btc(b: bytes, i: int):
    v = b[i]
    if v < 0xFD:
        return v, i + 1
    n = {0xFD: 2, 0xFE: 4, 0xFF: 8}[v]
    return int.from_bytes(b[i + 1:i + 1 + n], "little"), i + 1 + n


def coinbase_seal_hash(tx: bytes) -> bytes:
    """ExtractScriptSigFromCoinbaseTx + ExtractSealHashFromCoinbase (auxpow_coinbase_utils.go)."""
    i = 4                                   # version
    _, i = read_varint_btc(tx, i)           # input count
    i += 36                                 # prev txid + vout
    slen, i = read_varint_btc(tx, i)
    script = tx[i:i + slen]
    j = 0
    def push(j):
        op = script[j]
        if op > 75:
            raise ValueError(f"unsupported opcode {op:#x}")
        return script[j + 1:j + 1 + op], j + 1 + op
    height, j = push(j)
    if len(height) > 5:
        raise ValueError("bad BIP34 height push")
    payload, j = push(j)
    if len(payload) != 44 or payload[:4] != bytes.fromhex("fabe6d6d"):
        raise ValueError("no fabe6d6d auxpow commitment")
    return payload[4:36]


def report(label, got: bytes, want: str):
    print(f"{label:12s} {('0x' + got.hex())}  node={want}  match={('0x' + got.hex()) == want.lower()}")


def main():
    mode, path = sys.argv[1], sys.argv[2]
    d = json.load(open(path))
    d = d.get("result", d)
    if mode == "header":
        hdr, w = d, d["woHeader"]
    else:
        hdr, w = d["woBody"]["header"], d["woHeader"]
    report("headerHash", b3(header_seal_bytes(hdr)), w["headerHash"])
    try:
        seal = wo_seal_hash(w)
        print(f"sealHash     0x{seal.hex()}  (not reported by the node)")
        if "auxpow" in w:
            cb = coinbase_seal_hash(hx(w["auxpow"]["transaction"]))
            print(f"coinbase     0x{cb.hex()}  commits sealHash={cb == seal}  (powId={u(w['auxpow']['powId'])}, Kawpow=1; Scrypt=4 would need the Doge aux merkle root instead)")
        report("hash", wo_hash(w), w["hash"])
    except (KeyError, ValueError) as e:
        print("hash         cannot recompute:", e)


if __name__ == "__main__":
    main()
