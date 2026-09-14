#!/usr/bin/env python3
"""Clone the stopped public local-chain fixtures and build a read-only custody verifier.

Use harness.py --work /tmp/quai-sdk-reorg-qualification for start/reset. This
helper never rewinds the original node or wallets and never accesses Orchard.
"""
import argparse,json,pathlib,shutil,sqlite3,subprocess,os,socket
from clone_profile import clone
HERE=pathlib.Path(__file__).resolve().parent
WORK=pathlib.Path('/tmp/quai-sdk-reorg-qualification')
SOURCE=pathlib.Path('/tmp/quai-sdk-local-chain')
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('stage',choices=['prepare','build','restore','included','removed'])
a=p.parse_args()
def stopped(root):
    # A restricted process namespace can hide a host PID. Refuse a live RPC too;
    # permission errors must propagate instead of being mistaken for a stopped node.
    try:
        with socket.create_connection(('127.0.0.1',19200),timeout=2):
            raise ValueError('stop the local fixture RPC before copying databases')
    except ConnectionRefusedError:
        pass
    path=root/'node.pid'
    if path.exists():
        try:os.kill(int(path.read_text()),0)
        except ProcessLookupError:pass
        else:raise ValueError('node must be stopped before copying its databases')
if a.stage=='prepare':
    clone(SOURCE,WORK)
    shutil.copytree(WORK/'development-data',WORK/'included-data')
    with sqlite3.connect('file:/tmp/quai-sdk-highlevel-wallets/qi.sqlite?mode=ro',uri=True) as src:
        with sqlite3.connect(WORK/'qi.sqlite') as dst:src.backup(dst)
    print(json.dumps({'cloned':str(WORK),'originalNodeAndWalletsPreserved':True}))
    raise SystemExit
if not (WORK/'.quai-sdk-disposable-profile').is_file():raise ValueError('prepare qualification first')
if a.stage=='restore':
    stopped(WORK)
    old=WORK/'removed-data'
    if old.exists():raise ValueError('refusing existing removed-data')
    (WORK/'development-data').rename(old)
    shutil.copytree(WORK/'included-data',WORK/'development-data')
    print(json.dumps({'restoredInclusionDatabase':True,'rollbackDataRetained':str(old)}))
    raise SystemExit
client=WORK/'client';(client/'src').mkdir(parents=True,exist_ok=True)
shutil.copy2(HERE/'recovery_client.rs',client/'src/main.rs')
(client/'Cargo.toml').write_text('[package]\nname="quai-recovery-acceptance"\nversion="0.0.0"\nedition="2024"\npublish=false\n[workspace]\n[dependencies]\nquai-sdk={path='+json.dumps(str(HERE.parents[1]/'crates/quai-sdk'))+',features=["sqlite"]}\ntokio={version="1",features=["macros","rt-multi-thread"]}\nserde_json="1"\n')
if not (client/'Cargo.lock').exists():shutil.copy2(HERE.parents[1]/'Cargo.lock',client/'Cargo.lock')
command=['cargo','build' if a.stage=='build' else 'run','--manifest-path',str(client/'Cargo.toml'),'--offline']
if a.stage!='build':command+=['--',a.stage]
subprocess.run(command,check=True)
