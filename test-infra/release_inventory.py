#!/usr/bin/env python3
"""Record Cargo's resolved alpha dependency inventory without private local paths.

Run with the locked offline cache. This is a custom-schema inventory, not an
SPDX document, license clearance or a claim that every target dependency ships.
"""
import argparse
import datetime
import hashlib
import json
import pathlib
import subprocess
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--format-version', '1', '--all-features',
        '--locked', '--offline'], cwd=ROOT))
    lock = tomllib.loads((ROOT / 'Cargo.lock').read_text())
    checksums = {(p['name'], p['version'], p.get('source')): p.get('checksum')
                 for p in lock['package']}
    workspace = set(metadata['workspace_members'])
    ids = {p['id']: f"{p['name']}@{p['version']}" for p in metadata['packages']}
    assert len(set(ids.values())) == len(ids), 'Inventory identity collision'
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}
    packages = []
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        node = nodes[package['id']]
        manifest = pathlib.Path(package['manifest_path'])
        # Retain hashes of packaged upstream license/notice files, never local paths.
        notices = []
        for path in sorted(manifest.parent.iterdir()):
            if path.is_file() and path.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE', 'COPYRIGHT')):
                notices.append({'file': path.name,
                                'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
        packages.append({
            'id': ids[package['id']], 'name': package['name'], 'version': package['version'],
            'workspace': package['id'] in workspace, 'source': package['source'],
            'checksum': checksums.get((package['name'], package['version'], package['source'])),
            'licenseExpression': package['license'],
            'licenseFile': pathlib.Path(package['license_file']).name if package['license_file'] else None,
            'licenseNotices': notices, 'repository': package['repository'],
            'resolvedFeatures': sorted(node['features']),
            'dependencies': sorted([{
                'id': ids[d['pkg']],
                'kinds': [{'kind': k['kind'] or 'normal', 'target': k['target']}
                          for k in d['dep_kinds']]
            } for d in node['deps']], key=lambda d: d['id']),
        })
    report = {
        'schema': 'quai-sdk-cargo-dependency-inventory-v1',
        'recordedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'scope': 'Cargo metadata --all-features resolution, including build/dev and target-specific dependencies; not a deployed-binary component list or license clearance.',
        'cargoLockSha256': hashlib.sha256((ROOT / 'Cargo.lock').read_bytes()).hexdigest(),
        'packages': packages,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(f"Recorded {len(packages)} resolved packages; {len(workspace)} workspace crates")


if __name__ == '__main__':
    main()
