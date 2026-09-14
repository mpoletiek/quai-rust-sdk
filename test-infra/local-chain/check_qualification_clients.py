#!/usr/bin/env python3
"""Compile read-only recovery and synthetic refund clients without starting nodes."""
import json,os,pathlib,shutil,subprocess,tempfile
HERE=pathlib.Path(__file__).resolve().parent
ROOT=HERE.parents[1]
with tempfile.TemporaryDirectory(prefix='quai-qualification-clients-') as temporary:
    for name in ['recovery','refund']:
        client=pathlib.Path(temporary)/name;(client/'src').mkdir(parents=True)
        shutil.copy2(HERE/f'{name}_client.rs',client/'src/main.rs')
        (client/'Cargo.toml').write_text('[package]\nname="quai-'+name+'-qualification-check"\nversion="0.0.0"\nedition="2024"\npublish=false\n[workspace]\n[dependencies]\nquai-sdk={path='+json.dumps(str(ROOT/'crates/quai-sdk'))+',features=["sqlite","abi"]}\ntokio={version="1",features=["macros","rt-multi-thread"]}\nserde_json="1"\n')
        shutil.copy2(ROOT/'Cargo.lock',client/'Cargo.lock')
        env=os.environ.copy();env.setdefault('CARGO_TARGET_DIR',str(ROOT/'target'))
        subprocess.run(['cargo','clippy','--manifest-path',str(client/'Cargo.toml'),'--offline','--','-D','warnings'],env=env,check=True)
