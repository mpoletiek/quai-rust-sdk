#!/usr/bin/env python3
"""Build and operate an explicit disposable patched go-quai profile in /tmp only."""
import argparse
import hashlib
import io
import json
import os
import pathlib
import shutil
import signal
import subprocess
import tarfile
import time
import urllib.request

HERE=pathlib.Path(__file__).resolve().parent
COMMIT='f3f345c877300c044e3e0081a48bf3cf786fb9cc'
GENESIS='0x654e7a894d57de62ec19b9c161cb1c647466278e0565d3e1ba5d806ae6af0aee'
URL='http://127.0.0.1:19200'
MARKER='.quai-sdk-disposable-profile'
CLIENT_SOURCE=HERE/'client.rs'
CLIENT_NAME='quai-local-acceptance'
CLIENT_MODES=['inventory','transfer','replacement','deploy','qi','qi-split','qi-duplicate-fund','qi-multi','qi-inventory','receipt']

def rpc(method,params):
    body=json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params}).encode()
    req=urllib.request.Request(URL,data=body,headers={'Content-Type':'application/json'})
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self,*args,**kwargs):raise ValueError('redirect refused')
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),NoRedirect)
    with opener.open(req,timeout=3) as response:raw=response.read(2*1024*1024+1)
    if len(raw)>2*1024*1024:raise ValueError('response size')
    result=json.loads(raw)
    if result.get('jsonrpc')!='2.0' or result.get('id')!=1 or 'error' in result:raise ValueError(result.get('error','wrong ID'))
    return result['result']

def check():
    chain=rpc('quai_chainId',[])
    header=rpc('quai_getHeaderByNumber',['0x0'])
    if chain!='0x539' or header['woHeader']['hash']!=GENESIS:raise ValueError('refusing unknown chain/genesis')
    head=rpc('quai_getHeaderByNumber',['latest'])
    return {'chainId':1337,'genesisHash':GENESIS,'height':head['woHeader']['number'],'headHash':head['woHeader']['hash'],'gasLimit':head['gasLimit'],'stateLimit':head['stateLimit'],'balance':rpc('quai_getBalance',['0x0049CdA3305ccB9cB23E7Ce2528ceF555E9a5B32','latest']),'nonce':rpc('quai_getTransactionCount',['0x0049CdA3305ccB9cB23E7Ce2528ceF555E9a5B32','latest']),'qiOutpoints':rpc('quai_getOutpointsByAddress',['0x00eDf2d16AfbC028FB1e879559b07997Af79539f'])}

def owned(work):
    if not work.is_relative_to(pathlib.Path('/tmp')) or work==pathlib.Path('/tmp') or not (work/MARKER).is_file():raise ValueError('requires marked disposable /tmp work directory')

def no_running(work):
    pidfile=work/'node.pid'
    if pidfile.exists():
        try:os.kill(int(pidfile.read_text()),0)
        except ProcessLookupError:pidfile.unlink()
        else:raise ValueError('stop the owned node before snapshot/reset')

def args_for(work):
    return [str(work/'go-quai-sdk-development'),'start','--global.config-dir',str(work/'development-config'),'--global.data-dir',str(work/'development-data'),
        '--node.environment','local','--node.consensus-engine','blake3','--node.solo','--node.ipaddr','127.0.0.1','--node.port','14002','--node.portmap=false','--node.min-peers','0','--node.max-peers','0','--node.telemetry=false','--node.cache','128','--node.bloomfilter-size','16','--node.index-address-utxos','--node.miner-preference','0','--node.quai-coinbases','0x0049cda3305ccb9cb23e7ce2528cef555e9a5b32','--node.qi-coinbases','0x00edf2d16afbc028fb1e879559b07997af79539f',
        '--rpc.http-addr','127.0.0.1','--rpc.ws-addr','127.0.0.1','--rpc.http-port','19001','--rpc.ws-port','18001','--rpc.http-vhosts','localhost,127.0.0.1','--rpc.http-api','quai','--rpc.ws-api','quai','--txpool.sync-tx-with-return=false']

def inventory_tree(root):
    result=[]
    for file in sorted(root.rglob('*')):
        if file.is_symlink():raise ValueError('snapshot symlink refused')
        if file.is_file():result.append({'path':str(file.relative_to(root)),'bytes':file.stat().st_size,'sha256':hashlib.sha256(file.read_bytes()).hexdigest()})
    return result

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('command',choices=['prepare','start','probe','mine','snapshot','reset','client'])
    p.add_argument('--work',type=pathlib.Path,default=pathlib.Path('/tmp/quai-sdk-local-chain'))
    p.add_argument('--source',type=pathlib.Path,default=pathlib.Path('/tmp/quai-sdk-research-go'))
    p.add_argument('--count',type=int,default=3)
    p.add_argument('--mode',choices=CLIENT_MODES,default=CLIENT_MODES[0])
    p.add_argument('--hash')
    p.add_argument('--simulate-crash-after-ready',action='store_true',help='start only: SIGKILL the child this supervisor created after recording verified readiness')
    a=p.parse_args();work=a.work.resolve()
    if a.simulate_crash_after_ready and a.command!='start':raise ValueError('crash option requires start')
    if a.command=='prepare':
        if work.exists():raise ValueError('prepare refuses an existing work directory')
        if not work.is_relative_to(pathlib.Path('/tmp')):raise ValueError('work must be in /tmp')
        if subprocess.check_output(['git','-C',str(a.source),'rev-parse','HEAD'],text=True).strip()!=COMMIT:raise ValueError('source pin mismatch')
        work.mkdir();(work/MARKER).write_text(COMMIT+'\n');source=work/'source';source.mkdir()
        raw=subprocess.check_output(['git','-C',str(a.source),'archive','--format=tar',COMMIT])
        with tarfile.open(fileobj=io.BytesIO(raw)) as archive:archive.extractall(source,filter='data')
        env=os.environ.copy();env.update({'GOMAXPROCS':'4','GOCACHE':str(work/'build-cache'),'GOPATH':str(work/'gopath')})
        def build(name,target):subprocess.run(['go','build','-buildvcs=false','-p','4','-o',str(work/name),target],cwd=source,env=env,check=True)
        started=time.monotonic();build('go-quai-unmodified','./cmd/go-quai')
        helper=source/'cmd/sdk-local-tools';helper.mkdir();shutil.copy2(HERE/'tools.go',helper/'main.go');build('sdk-local-tools','./cmd/sdk-local-tools')
        subprocess.run(['python3',str(HERE/'patch_profile.py'),str(source)],check=True)
        build('go-quai-sdk-development','./cmd/go-quai')
        (work/'build.json').write_text(json.dumps({'sourceCommit':COMMIT,'seconds':time.monotonic()-started,'compiler':subprocess.check_output(['go','version'],text=True).strip(),'binarySha256':hashlib.sha256((work/'go-quai-sdk-development').read_bytes()).hexdigest()},indent=2)+'\n')
        return
    owned(work)
    if a.command=='probe':print(json.dumps(check(),indent=2));return
    if a.command=='client':
        check();client=work/'client';(client/'src').mkdir(parents=True,exist_ok=True)
        shutil.copy2(CLIENT_SOURCE,client/'src/main.rs')
        sdk=HERE.parents[1]/'crates/quai-sdk'
        (client/'Cargo.toml').write_text('[package]\nname="'+CLIENT_NAME+'"\nversion="0.0.0"\nedition="2024"\npublish=false\n[workspace]\n[dependencies]\nquai-sdk={path='+json.dumps(str(sdk))+'}\ntokio={version="1",features=["macros","rt-multi-thread"]}\n')
        if not (client/'Cargo.lock').exists():shutil.copy2(HERE.parents[1]/'Cargo.lock',client/'Cargo.lock')
        command=['cargo','run','--manifest-path',str(client/'Cargo.toml'),'--offline','--',a.mode]
        if a.mode=='receipt':
            if not a.hash:raise ValueError('--hash required')
            command.append(a.hash)
        subprocess.run(command,check=True);return
    if a.command=='mine':
        check();subprocess.run([str(work/'sdk-local-tools'),'mine',URL,GENESIS,str(a.count)],check=True);return
    if a.command=='start':
        no_running(work);(work/'development-config').mkdir(exist_ok=True)
        env={k:v for k,v in os.environ.items() if not k.startswith('GO_QUAI_')};env['GOMAXPROCS']='4'
        with open(work/'development.log','ab') as log:
            started=time.monotonic();node=subprocess.Popen(args_for(work),cwd=work/'source',env=env,stdout=log,stderr=subprocess.STDOUT)
            (work/'node.pid').write_text(str(node.pid))
            def stop(_signum=None,_frame=None):
                if node.poll() is None:node.send_signal(signal.SIGINT)
            signal.signal(signal.SIGINT,stop);signal.signal(signal.SIGTERM,stop)
            try:
                deadline=time.monotonic()+60
                while time.monotonic()<deadline:
                    if node.poll() is not None:raise ValueError('node exited; inspect development.log')
                    try:state=check();break
                    except (ValueError,OSError):time.sleep(0.2)
                else:raise TimeoutError('node readiness')
                state['startupSeconds']=time.monotonic()-started
                (work/'last-start.json').write_text(json.dumps(state,indent=2)+'\n')
                print(json.dumps(state),flush=True)
                if a.simulate_crash_after_ready:
                    node.kill();exit_code=node.wait()
                    (work/'simulated-crash.json').write_text(json.dumps({'preCrashState':state,'exitCode':exit_code,'scope':'owned child after readiness; no concurrent block import injected'},indent=2)+'\n')
                else:node.wait()
            finally:
                stop()
                try:node.wait(timeout=30)
                except subprocess.TimeoutExpired:node.kill();node.wait()
                (work/'node.pid').unlink(missing_ok=True)
        return
    no_running(work)
    data=work/'development-data';snapshot=work/'snapshot'
    if a.command=='snapshot':
        if snapshot.exists():raise ValueError('snapshot already exists')
        started=time.monotonic();shutil.copytree(data,snapshot)
        files=inventory_tree(snapshot);manifest={'files':files,'bytes':sum(f['bytes'] for f in files),'copySeconds':time.monotonic()-started}
        (work/'snapshot.json').write_text(json.dumps(manifest,indent=2)+'\n');print(json.dumps({k:v for k,v in manifest.items() if k!='files'}));return
    if a.command=='reset':
        expected=json.loads((work/'snapshot.json').read_text())
        if inventory_tree(snapshot)!=expected['files']:raise ValueError('snapshot digest mismatch')
        started=time.monotonic();backup=work/('previous-data-'+str(time.time_ns()))
        if data.exists():data.rename(backup)
        try:shutil.copytree(snapshot,data)
        except Exception:
            if backup.exists() and not data.exists():backup.rename(data)
            raise
        print(json.dumps({'resetSeconds':time.monotonic()-started,'previousDataRetained':str(backup)}))
if __name__=='__main__':main()
