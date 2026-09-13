#!/usr/bin/env python3
"""Keep packaged public test fixtures and license notices identical to their origins."""
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def synchronize(check=False):
    manifest = json.loads((ROOT / 'test-infra/package-files.json').read_text())
    assert manifest['schemaVersion'] == 1
    destinations = set()
    changed = []
    for row in manifest['copies']:
        source, destination = ((ROOT / row[k]).resolve() for k in ['source', 'destination'])
        if (not source.is_relative_to(ROOT) or not destination.is_relative_to(ROOT / 'crates')
                or source == destination or destination in destinations):
            raise ValueError('invalid package mirror entry')
        destinations.add(destination)
        expected = source.read_bytes()
        if not destination.is_file() or destination.read_bytes() != expected:
            changed.append(row['destination'])
            if not check:
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(expected)
    if check and changed:
        raise ValueError('stale packaged public files; run python3 test-infra/sync_package_files.py: ' + ', '.join(changed))
    return len(destinations), len(changed)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    count, changed = synchronize(args.check)
    print(f'Checked {count} packaged public files; {changed} synchronized')
