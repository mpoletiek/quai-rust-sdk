#!/usr/bin/env python3
"""Build/run explicit synthetic refund stages against the isolated conversion genesis."""
import argparse,json,pathlib,shutil,subprocess
HERE=pathlib.Path(__file__).resolve().parent
WORK=pathlib.Path('/tmp/quai-sdk-refund-qualification')
p=argparse.ArgumentParser(description=__doc__);p.add_argument('stage',choices=['build','credit','prepare','broadcast','observe']);p.add_argument('--origin');a=p.parse_args()
if a.stage=='credit' and not a.origin:p.error('credit requires --origin')
if not (WORK/'conversion-profile.json').is_file():raise ValueError('requires the copied conversion fixture')
client=WORK/'refund-client';(client/'src').mkdir(parents=True,exist_ok=True)
shutil.copy2(HERE/'refund_client.rs',client/'src/main.rs')
(client/'Cargo.toml').write_text('[package]\nname="quai-refund-acceptance"\nversion="0.0.0"\nedition="2024"\npublish=false\n[workspace]\n[dependencies]\nquai-sdk={path='+json.dumps(str(HERE.parents[1]/'crates/quai-sdk'))+',features=["sqlite","abi"]}\ntokio={version="1",features=["macros","rt-multi-thread"]}\nserde_json="1"\n')
if not (client/'Cargo.lock').exists():shutil.copy2(HERE.parents[1]/'Cargo.lock',client/'Cargo.lock')
command=['cargo','build' if a.stage=='build' else 'run','--manifest-path',str(client/'Cargo.toml'),'--offline']
if a.stage!='build':command+=['--',a.stage]
if a.origin:command+=[a.origin]
subprocess.run(command,check=True)
