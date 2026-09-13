#!/usr/bin/env python3
"""Build/run the explicit public-fixture wallet-session client; no automatic funding."""
import argparse,json,pathlib,shutil,subprocess
HERE=pathlib.Path(__file__).resolve().parent
CLIENT=pathlib.Path('/tmp/quai-sdk-highlevel-client')
p=argparse.ArgumentParser(description=__doc__);p.add_argument('command',choices=['build','run']);p.add_argument('--mode',choices=['init-fund','account-prepare','account-broadcast','qi-prepare','qi-broadcast','verify','verify-qi','replacement-prepare','replacement-broadcast','verify-replacement','verify-deployment'] + ['wquai-'+operation+'-'+stage for operation in ['deploy','deposit','approve','transfer','withdraw'] for stage in ['prepare','broadcast','verify']])
a=p.parse_args()
if a.command=='run' and not a.mode:p.error('run requires explicit --mode')
if CLIENT.exists() and (not (CLIENT/'Cargo.toml').is_file() or 'name="quai-highlevel-acceptance"' not in (CLIENT/'Cargo.toml').read_text()):raise ValueError('refusing unrelated existing client directory')
(CLIENT/'src').mkdir(parents=True,exist_ok=True);shutil.copy2(HERE/'highlevel_client.rs',CLIENT/'src/main.rs');shutil.copy2(HERE/'wrapper_workflow.rs',CLIENT/'src/wrapper_workflow.rs')
(CLIENT/'Cargo.toml').write_text('[package]\nname="quai-highlevel-acceptance"\nversion="0.0.0"\nedition="2024"\npublish=false\n[workspace]\n[dependencies]\nquai-sdk={path='+json.dumps(str(HERE.parents[1]/'crates/quai-sdk'))+',features=["sqlite","abi"]}\ntokio={version="1",features=["macros","rt-multi-thread"]}\nserde_json="1"\n')
if not (CLIENT/'Cargo.lock').exists():shutil.copy2(HERE.parents[1]/'Cargo.lock',CLIENT/'Cargo.lock')
command=['cargo','build' if a.command=='build' else 'run','--manifest-path',str(CLIENT/'Cargo.toml'),'--offline']
if a.command=='run':command+=['--',a.mode]
subprocess.run(command,check=True)
