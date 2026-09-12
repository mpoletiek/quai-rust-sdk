#!/usr/bin/env python3
"""Run the pinned Go oracle with isolated build/module caches and no node process."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

PIN = "f3f345c877300c044e3e0081a48bf3cf786fb9cc"
MODULE = "github.com/dominant-strategies/go-quai"
HERE = Path(__file__).resolve().parent


def command(args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=Path("/tmp/quai-sdk-research-go"))
    parser.add_argument("--fixtures", type=Path, default=HERE.parent.parent / "compatibility/fixtures/transactions.json")
    parser.add_argument("--cache", type=Path, default=Path("/tmp/quai-go-oracle-cache"))
    parser.add_argument("--update-lock", action="store_true", help="explicitly resolve and copy this harness's Go lock files")
    parser.add_argument("--test", action="store_true", help="run oracle regression tests before producing report")
    parser.add_argument("--aggregates", type=Path, help="optional public ordered aggregate fixtures with Rust signatures")
    parser.add_argument("--contracts", type=Path, help="optional contract-address fixtures, requires --test")
    args = parser.parse_args()
    source = args.source.resolve(strict=True)
    actual = command(["git", "-C", str(source), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    if actual != PIN:
        parser.error("Go source does not match the required pinned commit")
    status = command(["git", "-C", str(source), "status", "--porcelain", "--untracked-files=all"], capture_output=True, text=True).stdout
    if status:
        parser.error("Go source must be a clean checkout, including untracked files")
    fixture = args.fixtures.resolve(strict=True)
    aggregates = args.aggregates.resolve(strict=True) if args.aggregates else None
    contracts = args.contracts.resolve(strict=True) if args.contracts else None
    if contracts and not args.test:
        parser.error("--contracts requires --test")
    cache = args.cache.resolve()
    # Isolate all Go writes. Avoid ambient workspaces/toolchain downloads/build flags.
    env = os.environ.copy()
    env.update({
        "GOMODCACHE": str(cache / "modules"), "GOCACHE": str(cache / "build"),
        "GOPATH": str(cache / "gopath"), "GOWORK": "off", "GOTOOLCHAIN": "local",
        "GOFLAGS": "", "GOENV": "off", "CGO_ENABLED": "1",
        "GOPROXY": "https://proxy.golang.org,direct", "GOSUMDB": "sum.golang.org",
        "GOPRIVATE": "", "GONOSUMDB": "", "GONOPROXY": "",
        "QUAI_ORACLE_FIXTURES": str(fixture),
    })
    env.pop("QUAI_AGGREGATE_FIXTURES", None)
    env.pop("QUAI_CONTRACT_FIXTURES", None)
    if contracts:
        env["QUAI_CONTRACT_FIXTURES"] = str(contracts)
    if aggregates:
        env["QUAI_AGGREGATE_FIXTURES"] = str(aggregates)
    with tempfile.TemporaryDirectory(prefix="quai-go-oracle-run-", dir="/tmp") as work:
        work = Path(work)
        for name in ["go.mod", "go.sum", *(p.name for p in HERE.glob("*.go"))]:
            path = HERE / name
            if path.exists():
                shutil.copyfile(path, work / name)
        command(["go", "mod", "edit", f"-replace={MODULE}={source}"], cwd=work, env=env)
        if args.update_lock:
            command(["go", "mod", "tidy"], cwd=work, env=env)
            command(["go", "mod", "edit", f"-dropreplace={MODULE}"], cwd=work, env=env)
            shutil.copyfile(work / "go.mod", HERE / "go.mod")
            shutil.copyfile(work / "go.sum", HERE / "go.sum")
            command(["go", "mod", "edit", f"-replace={MODULE}={source}"], cwd=work, env=env)
        command(["go", "mod", "verify"], cwd=work, env=env, stdout=subprocess.PIPE)
        if args.test:
            command(["go", "test", "-mod=readonly", "-count=1", "./..."], cwd=work, env=env, stdout=os.sys.stderr)
        executable = work / "oracle"
        command(["go", "build", "-mod=readonly", "-buildvcs=false", "-trimpath", "-o", str(executable), "."], cwd=work, env=env)
        arguments = [str(executable), "-fixtures", str(fixture)]
        if aggregates:
            arguments.extend(["-aggregates", str(aggregates)])
        completed = subprocess.run(arguments, cwd=work, env=env, capture_output=True, text=True)
        # Bind process output to verified source identity and explicit runtime mode.
        if completed.stdout:
            report = json.loads(completed.stdout)
            report["sourceClean"] = True
            report["sourceCommitVerified"] = actual
            report["cgoEnabled"] = True
            if args.test:
                report["specialFeeRegression"] = {
                    "testSourceSha256": hashlib.sha256((HERE / "special_fee_test.go").read_bytes()).hexdigest(),
                    "passed": True,
                    "tests": ["TestSpecialFeeGasBound", "TestShaAnchoredFeeRatesIgnoreDifficultyArgument"],
                    "scope": "Pinned function tests; not funded chain acceptance",
                }
            if contracts:
                report["contractRegression"] = {
                    "fixtureSha256": hashlib.sha256(contracts.read_bytes()).hexdigest(),
                    "passed": True,
                    "test": "TestExactContractAddresses",
                    "cases": 30,
                    "legacyLeadingZeroDivergences": 18,
                }
            print(json.dumps(report, indent=2))
        if completed.stderr:
            print(completed.stderr, file=os.sys.stderr, end="")
        return completed.returncode


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"Go oracle setup/build failed: {error}", file=os.sys.stderr)
        raise SystemExit(2)
