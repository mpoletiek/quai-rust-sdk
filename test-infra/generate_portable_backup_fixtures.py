#!/usr/bin/env python3
"""Regenerate PUBLIC TOY native-to-portable QUAIWALT v3/v4/v5 fixtures.

Uses explicit deterministic test-only salt/nonce through private unit-test helpers.
Never reads a user's wallet. v1/v2 independent Python vectors remain separate.
"""
import os
import pathlib
import subprocess
root = pathlib.Path(__file__).resolve().parent.parent
subprocess.run(['cargo', 'test', '-p', 'quai-wallet', '--all-features', '--lib', '--locked', '--offline', 'full_backup::tests::'], cwd=root, env={**os.environ, 'QUAI_PORTABLE_BACKUP_FIXTURE_DIR': str(root / 'crates/quai-wallet/tests')}, check=True)
