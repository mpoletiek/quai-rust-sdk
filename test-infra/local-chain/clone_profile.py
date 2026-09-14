#!/usr/bin/env python3
"""Copy a stopped, marked SDK /tmp fixture; never reset the source or overwrite work."""
import argparse,json,pathlib,shutil,socket
import harness

def clone(source,work,snapshot=False):
    source=source.resolve();work=work.resolve()
    harness.owned(source)
    if not work.is_relative_to(pathlib.Path('/tmp')) or work.exists():raise ValueError('requires a new /tmp directory')
    if (source/harness.MARKER).read_text().strip()!=harness.COMMIT:raise ValueError('source marker mismatch')
    # Permissions errors propagate: a restricted namespace is not proof of absence.
    with socket.socket() as probe:probe.bind(('127.0.0.1',19200))
    harness.no_running(source)
    manifest=json.loads((source/'snapshot.json').read_text())
    if harness.inventory_tree(source/'snapshot')!=manifest['files']:raise ValueError('snapshot digest mismatch')
    work.mkdir()
    for name in [harness.MARKER,'go-quai-sdk-development','sdk-local-tools','snapshot.json']:
        shutil.copy2(source/name,work/name)
    if (source/'conversion-profile.json').exists():shutil.copy2(source/'conversion-profile.json',work/'conversion-profile.json')
    shutil.copytree(source/('snapshot' if snapshot else 'development-data'),work/'development-data')
    for name in ['development-config','snapshot']:shutil.copytree(source/name,work/name)
    (work/'source').mkdir()
    shutil.copy2(source/'source/VERSION',work/'source/VERSION')
    shutil.copytree(source/'source/params',work/'source/params')
    return {'source':str(source),'copy':str(work),'fromVerifiedSnapshot':snapshot,'sourceDataPreserved':True}

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source',type=pathlib.Path,required=True);p.add_argument('--work',type=pathlib.Path,required=True)
    p.add_argument('--snapshot',action='store_true')
    a=p.parse_args();print(json.dumps(clone(a.source,a.work,a.snapshot)))
