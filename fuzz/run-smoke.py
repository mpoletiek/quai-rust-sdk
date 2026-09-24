#!/usr/bin/env python3
"""Run already-built sanitizer fuzz targets for bounded time.

Inputs libFuzzer discovers are written to a persistent working corpus under
`fuzz/workdir/<target>` rather than a temporary directory, so a cached working
corpus accumulates coverage across runs instead of being discarded. The
committed `fuzz/corpus/<target>` tree stays read-only: it is reproducible from
`seed-corpus.py` and is checked against that generator, so libFuzzer must not
write into it.

The seed defaults to a fresh random one. A fixed seed plus a fixed corpus
explores a near-identical trajectory every run, which makes this a regression
check rather than fuzzing; pass --seed to reproduce a specific run.

Any crash, timeout, out-of-memory or leak artifact fails the run, whether it was
produced now or left by an earlier one. libFuzzer's exit code alone is not
enough: an artifact can be written in a run that still exits zero.
"""
import argparse, datetime, hashlib, json, os, pathlib, re, secrets, subprocess

TARGETS = ['transactions', 'abi', 'wallet_import', 'encoding', 'fixed', 'head_state',
           'rpc_envelope', 'ws_dispatch', 'provider_responses', 'state_proof',
           'state_proof_trie', 'header_hash']
ARTIFACT_PREFIXES = ('crash-', 'timeout-', 'oom-', 'leak-', 'slow-unit-')
root = pathlib.Path(__file__).resolve().parent


def artifacts_in(directory):
    """Report failure artifacts only; `smoke.log` and the report are not findings."""
    if not directory.is_dir():
        return []
    return sorted(p.name for p in directory.iterdir() if p.name.startswith(ARTIFACT_PREFIXES))


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--seconds', type=int, default=30)
parser.add_argument('--target', action='append', choices=TARGETS)
parser.add_argument('--seed', type=int, default=None,
                    help='libFuzzer PRNG seed; omit for a fresh random seed each run')
parser.add_argument('--report', type=pathlib.Path, default=root / 'smoke-results.json')
args = parser.parse_args()
if not 1 <= args.seconds <= 600:
    parser.error('seconds must be 1..600')

report = {
    'recordedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'kind': 'bounded sanitizer fuzz smoke; not sustained security qualification',
    'fuzzLockSha256': hashlib.sha256((root / 'Cargo.lock').read_bytes()).hexdigest(),
    'sdkLockSha256': hashlib.sha256((root.parent / 'Cargo.lock').read_bytes()).hexdigest(),
    'targets': [],
}
failed = False
for target in args.target or TARGETS:
    binary = root / 'target' / 'x86_64-unknown-linux-gnu' / 'release' / target
    if not binary.is_file():
        raise SystemExit('build missing target: ' + str(binary))
    symbols = subprocess.run(['nm', str(binary)], capture_output=True, check=True, text=True).stdout
    if '__asan_init' not in symbols:
        raise SystemExit('AddressSanitizer symbol missing: ' + target)

    artifacts = root / 'artifacts' / target
    artifacts.mkdir(parents=True, exist_ok=True)
    # An artifact left by an earlier run is an untriaged finding, not history.
    # Record it and fail rather than letting it sit unnoticed as one did for days.
    pre_existing = artifacts_in(artifacts)
    if pre_existing:
        print('pre-existing artifacts for', target + ':', ', '.join(pre_existing), flush=True)
        failed = True

    seeds = root / 'corpus' / target
    work = root / 'workdir' / target
    work.mkdir(parents=True, exist_ok=True)
    seed = args.seed if args.seed is not None else secrets.randbelow(2**31 - 1) + 1
    # The first directory is the writable corpus; later ones are read-only inputs.
    command = [
        str(binary), str(work), str(seeds),
        '-max_total_time=' + str(args.seconds),
        '-max_len=' + str(163887 if target == 'head_state' else 65536),
        '-rss_limit_mb=1024', '-timeout=5', '-seed=' + str(seed),
        '-artifact_prefix=' + str(artifacts) + '/',
    ]
    print('Fuzzing', target, 'for', args.seconds, 'seconds with seed', seed, flush=True)
    try:
        result = subprocess.run(command, capture_output=True, text=True,
                                timeout=args.seconds + 30,
                                env={**os.environ, 'RUST_BACKTRACE': '1'})
    except subprocess.TimeoutExpired:
        report['targets'].append({'name': target, 'exitCode': 'timeout', 'seed': seed})
        failed = True
        continue

    log = result.stdout + result.stderr
    (artifacts / 'smoke.log').write_text(log)
    done = re.findall(r'#(\d+)\s+DONE\s+cov:\s*(\d+)\s+ft:\s*(\d+).*?rss:\s*(\d+)Mb', log)
    discovered = [name for name in artifacts_in(artifacts) if name not in pre_existing]
    row = {
        'name': target,
        'binarySha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'seconds': args.seconds,
        'seed': seed,
        'exitCode': result.returncode,
        'addressSanitizer': True,
        'seedCorpusFiles': len(list(seeds.iterdir())),
        'workingCorpusFiles': len(list(work.iterdir())),
        'newArtifacts': discovered,
        'preExistingArtifacts': pre_existing,
    }
    if done:
        row.update(executions=int(done[-1][0]), coverageCounters=int(done[-1][1]),
                   features=int(done[-1][2]), rssMiB=int(done[-1][3]))
    report['targets'].append(row)
    print(json.dumps(row), flush=True)
    if discovered:
        print('new artifacts for', target + ':', ', '.join(discovered), flush=True)
        failed = True
    if result.returncode:
        print(log[-1800:], flush=True)
        failed = True

args.report.write_text(json.dumps(report, indent=2) + '\n')
raise SystemExit(1 if failed else 0)
