#!/usr/bin/env python3
"""Build Cargo archives and a clean consumer using only the extracted SDK sources.

Never publishes or contacts a registry. Run cargo fetch --locked beforehand.
External dependency cache access is allowed; SDK workspace paths are used only
while constructing archives and are replaced with extracted archives for tests.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def run(args, cwd, log, env=None):
    completed = subprocess.run(args, cwd=cwd, env=env, stdout=log,
                               stderr=subprocess.STDOUT, check=False)
    if completed.returncode:
        raise RuntimeError(f'{args[0]} {args[1]} failed ({completed.returncode}); see rehearsal log')


def extract(archive, destination, expected_root):
    """Only bounded regular Cargo source files; no links or path escapes."""
    with tarfile.open(archive, 'r:gz') as source:
        members = source.getmembers()
        if len(members) > 10000 or sum(m.size for m in members) > 64 * 1024 * 1024:
            raise ValueError('archive exceeds source budget')
        for member in members:
            path = PurePosixPath(member.name)
            if (path.is_absolute() or '..' in path.parts or not path.parts
                    or path.parts[0] != expected_root or '\\' in member.name
                    or not (member.isfile() or member.isdir())):
                raise ValueError('invalid Cargo archive member')
        source.extractall(destination, members=members, filter='data')


def patch_file(path, packages, roots):
    path.write_text('[patch.crates-io]\n' + ''.join(
        f'{p["name"]} = {{ path = {json.dumps(str(roots[p["name"]]))} }}\n'
        for p in packages))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    report_path = args.report.resolve()
    report_path.parent.mkdir(parents=True, exist_ok=True)
    log_path = report_path.with_suffix('.log')
    subprocess.run(['python3', str(ROOT / 'test-infra/sync_package_files.py'), '--check'], check=True)
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--no-deps', '--format-version', '1', '--offline'], cwd=ROOT))
    packages = [p for p in metadata['packages'] if p['id'] in metadata['workspace_members']]
    report = {'schemaVersion': 1, 'published': False, 'networkAccess': False,
              'sourceCommit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'sourceDirty': bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT)),
              'packages': [], 'consumerTests': {}, 'packagedTargetChecks': {},
              'qualification': 'Extracted source archive consumer tests; not public-registry publication, packaged test execution, docs.rs or release certification.'}
    with tempfile.TemporaryDirectory(prefix='quai-package-rehearsal-') as work, log_path.open('w') as log:
        work = Path(work)
        source_patches = work / 'source-patches.toml'
        patch_file(source_patches, packages, {p['name']: Path(p['manifest_path']).parent for p in packages})
        print(f'Packaging {len(packages)} crates; log: {log_path}', flush=True)
        run(['cargo', 'package', '--workspace', '--allow-dirty', '--offline', '--no-verify',
             '--config', str(source_patches), '--target-dir', str(work / 'build')], ROOT, log)
        extracted = work / 'extracted'
        extracted.mkdir()
        staged_roots = {}
        for package in packages:
            name = f'{package["name"]}-{package["version"]}'
            archive = work / 'build' / 'package' / f'{name}.crate'
            extract(archive, extracted, name)
            staged_roots[package['name']] = extracted / name
            for required in ['README.md', 'LICENSE', 'LICENSE.quais-js', 'THIRD_PARTY_NOTICES.md']:
                if not (extracted / name / required).is_file():
                    raise ValueError(f'{name} lacks packaged {required}')
            report['packages'].append({'name': package['name'], 'version': package['version'],
                                       'bytes': archive.stat().st_size,
                                       'sha256': hashlib.sha256(archive.read_bytes()).hexdigest()})
        consumer = work / 'consumer'
        (consumer / 'src').mkdir(parents=True)
        version = next(p['version'] for p in packages if p['name'] == 'quai-sdk')
        (consumer / 'Cargo.toml').write_text('''[package]
name = "quai-package-consumer"
version = "0.0.0"
edition = "2024"
publish = false
[workspace]
[features]
default = ["quai-sdk/default"]
full = ["quai-sdk/sqlite", "quai-sdk/abi", "quai-sdk/payments", "quai-sdk/keystore", "quai-sdk/ws", "quai-sdk/browser"]
[dependencies]
quai-sdk = {version = "=''' + version + '''", default-features = false}
''')
        (consumer / 'src' / 'lib.rs').write_text('''#[test]
fn public_address_and_fixed_point() {
    use quai_sdk::primitives::{Address, FixedPoint, FixedFormat};
    let address: Address = "0x002b2596EcF05C93a31ff916E8b456DF6C77c750".parse().unwrap();
    assert_eq!(address.zone().unwrap(), quai_sdk::Zone::Cyprus1);
    let value = FixedPoint::parse("1.25", FixedFormat::default()).unwrap();
    assert_eq!(value.to_string(), "1.25");
}
#[cfg(feature = "default")]
#[test]
fn public_fixture_hd_derivation() {
    let root = quai_sdk::wallet::ExtendedPrivateKey::from_seed(&[1;16]).unwrap();
    let account = root.derive_relative_path("44'/969'/0'").unwrap();
    assert_eq!(account.metadata().depth,3);
    assert_eq!(account.derive_relative_path("0/7").unwrap().public_key().export(),
        account.public_key().derive_relative_path("0/7").unwrap().export());
}
#[cfg(feature = "full")]
#[test]
fn artifact_import() {
    let artifact = quai_sdk::abi::SolidityArtifact::from_json(br#"{"abi":[],"bytecode":"0x6000"}"#).unwrap();
    assert_eq!(artifact.init_code(), &[0x60,0]);
}
''')
        staged_patches = work / 'staged-patches.toml'
        patch_file(staged_patches, packages, staged_roots)
        # Override build output only. Resolution sees no workspace source patches.
        env = {**os.environ, 'CARGO_TARGET_DIR': str(work / 'consumer-build'), 'CARGO_NET_OFFLINE': 'true'}
        for label, flags in [('minimal', ['--no-default-features']), ('default', []), ('full', ['--all-features'])]:
            print(f'Testing extracted archive consumer: {label}', flush=True)
            run(['cargo', 'test', '--offline', '--config', str(staged_patches), *flags], consumer, log, env)
            report['consumerTests'][label] = 'passed'
        for package in packages:
            print(f'Checking packaged tests/examples: {package["name"]}', flush=True)
            run(['cargo', 'check', '--offline', '--all-features', '--all-targets',
                 '--config', str(staged_patches), '--manifest-path',
                 str(staged_roots[package['name']] / 'Cargo.toml')], consumer, log, env)
            report['packagedTargetChecks'][package['name']] = 'passed'
        resolved = json.loads(subprocess.check_output(['cargo', 'metadata', '--offline', '--format-version', '1', '--config', str(staged_patches)], cwd=consumer, env=env, stderr=subprocess.DEVNULL))
        for package in resolved['packages']:
            if package['name'] in staged_roots and Path(package['manifest_path']).parent != staged_roots[package['name']]:
                raise ValueError('consumer resolved an SDK package outside extracted archives')
    report_path.write_text(json.dumps(report, indent=2) + '\n')
    print(f'All extracted consumer checks passed: {report_path}', flush=True)


if __name__ == '__main__':
    main()
