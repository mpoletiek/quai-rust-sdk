#!/usr/bin/env python3
"""Check local guide links and reproduce the declaration-family review appendix."""
import collections
import json
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[1]


def main():
    paths = [ROOT / name for name in (
        'SDK_DOCUMENTATION.md', 'SDK_PARITY_ANALYSIS.md', 'docs/BROWSER_ATOMIC_RESTORE.md'
    )]
    for path in paths:
        for target in re.findall(r'\]\(([^)]+)\)', path.read_text()):
            if '://' in target or target.startswith('#'):
                continue
            assert (path.parent / target.split('#')[0]).exists(), f'{path.name}: missing {target}'
    rows = json.loads((ROOT / 'compatibility/parity.json').read_text())['entries']
    groups = collections.defaultdict(collections.Counter)
    counts = collections.Counter()
    for row in rows:
        groups[row['symbol'].split('.')[0]][row['status']] += 1
        counts[row['status']] += 1
    table = '| Export family | Pending | Partial | Implemented | Deviation |\n| --- | ---: | ---: | ---: | ---: |\n'
    for name, count in sorted(groups.items(), key=lambda item: (-item[1]['pending'] - item[1]['partial'], item[0].lower())):
        if count['pending'] or count['partial']:
            table += f"| `{name}` | {count['pending']} | {count['partial']} | {count['implemented']} | {count['deviation']} |\n"
    analysis = paths[1].read_text()
    actual = analysis.split('<!-- parity-family-table:start -->\n', 1)[1].split('<!-- parity-family-table:end -->', 1)[0]
    assert actual == table, 'Parity appendix drift: review and regenerate from the ledger'
    for status, count in counts.items():
        assert f'| `{status}` | {count} |' in analysis, f'Parity status count drift: {status}'
    print(f'SDK guide links and parity appendix checked against {len(rows)} declaration rows')


if __name__ == '__main__':
    main()
