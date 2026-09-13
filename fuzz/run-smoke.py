#!/usr/bin/env python3
"""Run already-built sanitizer fuzz targets for bounded time on copied public corpora."""
import argparse,datetime,hashlib,json,os,pathlib,re,shutil,subprocess,tempfile
root=pathlib.Path(__file__).resolve().parent
parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--seconds',type=int,default=30);parser.add_argument('--target',action='append',choices=['transactions','abi','wallet_import','encoding','fixed','head_state']);parser.add_argument('--report',type=pathlib.Path,default=root/'smoke-results.json');args=parser.parse_args()
if not 1<=args.seconds<=600:parser.error('seconds must be 1..600')
report={'recordedAt':datetime.datetime.now(datetime.timezone.utc).isoformat(),'kind':'bounded sanitizer fuzz smoke; not sustained security qualification','fuzzLockSha256':hashlib.sha256((root/'Cargo.lock').read_bytes()).hexdigest(),'sdkLockSha256':hashlib.sha256((root.parent/'Cargo.lock').read_bytes()).hexdigest(),'targets':[]}
failed=False
for target in args.target or ['transactions','abi','wallet_import','encoding','fixed','head_state']:
 binary=root/'target'/'x86_64-unknown-linux-gnu'/'release'/target
 if not binary.is_file():raise SystemExit('build missing target: '+str(binary))
 symbols=subprocess.run(['nm',str(binary)],capture_output=True,check=True,text=True).stdout
 if '__asan_init' not in symbols:raise SystemExit('AddressSanitizer symbol missing: '+target)
 artifacts=root/'artifacts'/target;artifacts.mkdir(parents=True,exist_ok=True)
 with tempfile.TemporaryDirectory(prefix='quai-public-fuzz-') as work:
  corpus=pathlib.Path(work)/'corpus';shutil.copytree(root/'corpus'/target,corpus)
  command=[str(binary),str(corpus),'-max_total_time='+str(args.seconds),'-max_len='+str(163887 if target=='head_state' else 65536),'-rss_limit_mb=1024','-timeout=5','-seed=1337','-artifact_prefix='+str(artifacts)+'/']
  print('Fuzzing',target,'for',args.seconds,'seconds',flush=True)
  try:result=subprocess.run(command,capture_output=True,text=True,timeout=args.seconds+30,env={**os.environ,'RUST_BACKTRACE':'1'})
  except subprocess.TimeoutExpired as error:
   report['targets'].append({'name':target,'exitCode':'timeout'});failed=True;continue
  log=result.stdout+result.stderr;(artifacts/'smoke.log').write_text(log)
  done=re.findall(r'#(\d+)\s+DONE\s+cov:\s*(\d+)\s+ft:\s*(\d+).*?rss:\s*(\d+)Mb',log)
  row={'name':target,'binarySha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'seconds':args.seconds,'exitCode':result.returncode,'addressSanitizer':True,'initialCorpusFiles':len(list((root/'corpus'/target).iterdir())),'finalCorpusFiles':len(list(corpus.iterdir()))}
  if done:row.update(executions=int(done[-1][0]),coverageCounters=int(done[-1][1]),features=int(done[-1][2]),rssMiB=int(done[-1][3]))
  report['targets'].append(row);print(json.dumps(row),flush=True)
  if result.returncode:print(log[-1800:],flush=True);failed=True
args.report.write_text(json.dumps(report,indent=2)+'\n')
raise SystemExit(1 if failed else 0)
