#!/usr/bin/env python3
"""Run only a separately prepared conversion profile; ordinary client remains fail-closed."""
import argparse,json,pathlib
import harness
p=argparse.ArgumentParser(add_help=False);p.add_argument('--work',type=pathlib.Path,default=pathlib.Path('/tmp/quai-sdk-conversion-chain'))
a,_=p.parse_known_args();work=a.work.resolve();harness.owned(work)
profile=json.loads((work/'conversion-profile.json').read_text())
if profile.get('kind')!='conversion-development' or profile.get('sourceCommit')!=harness.COMMIT:raise ValueError('wrong conversion profile')
if profile.get('genesisHash')==harness.GENESIS:raise ValueError('conversion profile must have distinct genesis')
harness.GENESIS=profile['genesisHash']
harness.CLIENT_SOURCE=harness.HERE/'conversion_client.rs'
harness.CLIENT_NAME='quai-conversion-acceptance'
harness.CLIENT_MODES=['quai-to-qi','qi-to-quai','quai-to-qi-large','qi-spend-converted']
# Inject the profile work argument when not explicitly present.
import sys
if '--work' not in sys.argv:sys.argv.extend(['--work',str(work)])
harness.main()
