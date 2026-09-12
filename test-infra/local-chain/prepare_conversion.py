#!/usr/bin/env python3
"""Prepare a distinct explicitly accelerated conversion profile; never reuse ordinary data."""
import argparse, hashlib, io, json, os, pathlib, shutil, subprocess, tarfile
import harness

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--work',type=pathlib.Path,default=pathlib.Path('/tmp/quai-sdk-conversion-chain'))
    p.add_argument('--source',type=pathlib.Path,default=pathlib.Path('/tmp/quai-sdk-research-go'))
    p.add_argument('--helper',type=pathlib.Path,default=pathlib.Path('/tmp/quai-sdk-local-chain/sdk-local-tools'))
    a=p.parse_args();work=a.work.resolve()
    if work.exists() or not work.is_relative_to(pathlib.Path('/tmp')):raise ValueError('requires new isolated /tmp directory')
    if subprocess.check_output(['git','-C',str(a.source),'rev-parse','HEAD'],text=True).strip()!=harness.COMMIT:raise ValueError('wrong source pin')
    work.mkdir();(work/harness.MARKER).write_text(harness.COMMIT+'\n');source=work/'source';source.mkdir()
    raw=subprocess.check_output(['git','-C',str(a.source),'archive','--format=tar',harness.COMMIT])
    with tarfile.open(fileobj=io.BytesIO(raw)) as archive:archive.extractall(source,filter='data')
    subprocess.run(['python3',str(harness.HERE/'patch_profile.py'),str(source)],check=True)
    alloc=json.loads((source/'params/genesis_alloc.json').read_text())
    for name in ['award','vested']:alloc[0][name]+=1
    alloc[0]['balanceSchedule']['0']+=1
    (source/'params/genesis_alloc.json').write_text(json.dumps(alloc,separators=(',',':'))+'\n')
    digest=subprocess.check_output([str(a.helper),'hash',str(source/'params/genesis_alloc.json')],text=True).strip()
    def replace(path,old,new):
        file=source/path;text=file.read_text()
        if text.count(old)!=1:raise ValueError('wrong conversion patch baseline '+path)
        file.write_text(text.replace(old,new))
    replace('params/config.go','0x11035b9cfa8bdea8f0274f9d39723f337941659d95972a303384258fe7a8988a',digest)
    replace('params/protocol_params.go','ControllerKickInBlock                uint64 = 262000','ControllerKickInBlock                uint64 = 1')
    replace('params/protocol_params.go','ConversionLockPeriod uint64 = 2 * BlocksPerWeek','ConversionLockPeriod uint64 = 4')
    helper=source/'cmd/sdk-local-tools';helper.mkdir();shutil.copy2(harness.HERE/'tools.go',helper/'main.go')
    genesis=source/'cmd/sdk-profile-genesis';genesis.mkdir()
    (genesis/'main.go').write_text('package main\nimport("fmt";"github.com/dominant-strategies/go-quai/core";"github.com/dominant-strategies/go-quai/params")\nfunc main(){g:=core.DefaultLocalGenesisBlock("blake3",0,nil);g.AllocHash=params.AllocHash;fmt.Println(g.ToBlock(0).Hash().Hex())}\n')
    env=os.environ.copy();env.update({'GOMAXPROCS':'4','GOCACHE':os.environ.get('QUAI_GO_BUILD_CACHE',str(work/'build-cache')),'GOPATH':str(work/'gopath')})
    def build(name,target):subprocess.run(['go','build','-buildvcs=false','-p','4','-o',str(work/name),target],cwd=source,env=env,check=True)
    build('go-quai-sdk-development','./cmd/go-quai');build('sdk-local-tools','./cmd/sdk-local-tools');build('sdk-profile-genesis','./cmd/sdk-profile-genesis')
    genesis_hash=subprocess.check_output([str(work/'sdk-profile-genesis')],text=True).strip()
    profile={'kind':'conversion-development','sourceCommit':harness.COMMIT,'chainId':1337,'genesisHash':genesis_hash,'allocationBlake3':digest,'controllerPrimeHeight':1,'conversionLockZoneBlocks':4,'otherForkThresholds':'pinned','quaiFixtureBalance':str(10**27+1),'binarySha256':hashlib.sha256((work/'go-quai-sdk-development').read_bytes()).hexdigest(),'unmodifiedConsensusQualified':False}
    (work/'conversion-profile.json').write_text(json.dumps(profile,indent=2)+'\n');print(json.dumps(profile,indent=2))
if __name__=='__main__':main()
