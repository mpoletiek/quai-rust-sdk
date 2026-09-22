#!/usr/bin/env python3
"""Fail when the public API breaks against crates.io without a declared Breaking section.

cargo-semver-checks treats an unchanged pre-release version as a major bump and
skips every check, so it is run as a minor release: any breaking change against
the latest published version is then reported. A break is allowed only when the
changelog's top section, `## Unreleased` or the version being released, lists it
under `Breaking:`. A tool failure that reports no lint is an error, never a pass.
"""
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def top_section(changelog):
    """The first `## ` section of the changelog, heading included."""
    match = re.search(r'^## .*?(?=^## |\Z)', changelog, re.M | re.S)
    if not match:
        raise ValueError('CHANGELOG.md has no section')
    return match.group(0)


def declares_breaking(section):
    return re.search(r'^Breaking:\s*$', section, re.M) is not None


def decide(returncode, output, section):
    """Return (passed, message) for one semver-checks run."""
    if returncode == 0:
        return True, 'no breaking change against the published API'
    if '--- failure ' not in output:
        return False, 'cargo-semver-checks failed without reporting a lint; see its output'
    heading = section.splitlines()[0]
    if declares_breaking(section):
        return True, f'breaking changes are declared under Breaking: in "{heading}"'
    return False, f'breaking changes found, but "{heading}" has no Breaking: section'


def main():
    section = top_section((ROOT / 'CHANGELOG.md').read_text())
    run = subprocess.run(
        ['cargo', 'semver-checks', 'check-release', '--workspace', '--release-type', 'minor'],
        cwd=ROOT, capture_output=True, text=True,
    )
    output = run.stdout + run.stderr
    print(output)
    passed, message = decide(run.returncode, output, section)
    print(('semver gate passed: ' if passed else 'semver gate failed: ') + message)
    return 0 if passed else 1


if __name__ == '__main__':
    sys.exit(main())
