"""Exercise published signed module updates using real IDE executables and a disposable profile.

Checks signature rejection, update opt-out, disabled state, pinned/modified modules,
rollback and retry. Downloads only immutable public releases; never uses the real profile.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import urllib.request

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tools'))
import prepare_modules

def download(url):
    assert url.startswith('https://github.com/sherhn/Light-Modules/releases/download/')
    with urllib.request.urlopen(urllib.request.Request(url,headers={'User-Agent':'Light-native-module-update-test'}),timeout=60) as response:
        value=response.read(100*1024*1024+1)
    assert len(value)<=100*1024*1024
    return value

def published(version):
    base=f'https://github.com/sherhn/Light-Modules/releases/download/v{version}/'
    envelope=download(base+'updates.json')
    manifest=json.loads(json.loads(envelope)['signed'])
    artifact=manifest['targets']['catalog']
    assert manifest['version']==version and artifact['name']=='index.json' and artifact['url']==base+'index.json'
    index=download(artifact['url'])
    assert len(index)==artifact['size'] and hashlib.sha256(index).hexdigest()==artifact['sha256']
    return envelope,index,json.loads(index)

def save(path,value):
    path.parent.mkdir(parents=True,exist_ok=True)
    path.write_text(json.dumps(value),encoding='utf-8')

parser=argparse.ArgumentParser()
parser.add_argument('--bootstrap',type=Path,required=True)
parser.add_argument('--exe',type=Path,required=True)
args=parser.parse_args()
old_envelope,old_bytes,old_index=published('1.0.1')
new_envelope,new_bytes,new_index=published('1.0.3')
old={entry['id']:entry for entry in old_index['modules']}
new={entry['id']:entry for entry in new_index['modules']}
archive=download('https://github.com/sherhn/Light-Modules/releases/download/v1.0.1/light-modules-1.0.1.zip')
files=prepare_modules.validate(archive,hashlib.sha256(archive).hexdigest())
assert json.loads(files['index.json'])==old_index
with tempfile.TemporaryDirectory(prefix='light-native-module-update-') as temporary:
    work=Path(temporary);registry=work/'registry';profile=work/'profile';store=profile/'modules';cache=profile/'updates'
    for name,data in files.items():
        path=registry/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(data)
    settings=profile/'settings.json'
    save(settings,{'modules_welcome_done':True,'auto_update_ide':False,'auto_update_modules':True,'theme_contrast':123,'font_size':17.0})
    script=work/'script.json'
    save(script,[{'op':'assert_modules','installed':['python','javascript','docker']},{'op':'quit'}])
    env=dict(os.environ,LIGHT_CONFIG_DIR=str(profile),LIGHT_MODULES_DIR=str(store),LIGHT_DISABLE_AUTO_UPDATES='1')
    env.pop('LIGHT_PRELOAD_MODULES',None);env.pop('LIGHT_REGISTRY',None)
    logs=[]
    def launch(executable,extra=None):
        current=dict(env);current.update(extra or {})
        result=subprocess.run([str(executable.resolve()),'--script',str(script),'--no-restore'],capture_output=True,env=current,timeout=90)
        log=result.stdout.decode('utf-8',errors='replace')+result.stderr.decode('utf-8',errors='replace');logs.append(log)
        assert result.returncode==0 and 'SCRIPT OK' in log,log
    def versions():
        return {name:json.loads((store/name/'module.json').read_text(encoding='utf-8'))['version'] for name in ['python','javascript','docker']}
    launch(args.bootstrap,{'LIGHT_REGISTRY':str(registry),'LIGHT_PRELOAD_MODULES':'python,javascript,docker'})
    before=versions();assert before=={name:old[name]['version'] for name in before}
    state=json.loads((store/'state.json').read_text());state['enabled']['python']=False;save(store/'state.json',state)
    docker=store/'docker/LICENSE';docker.write_bytes(docker.read_bytes()+b'\nPersonal local modification\n');original_docker=docker.read_bytes()
    (store/'docker/my-settings.txt').write_text('Keep my module data')
    preferences=json.loads(settings.read_text());preferences['module_update_pins']=['javascript'];save(settings,preferences)
    note=work/'project/.light-notes/project.md';note.parent.mkdir(parents=True);note.write_text('Keep my project notes')
    catalog=cache/'catalog/1.0.3/index.json';catalog.parent.mkdir(parents=True);catalog.write_bytes(new_bytes)
    for name in before:
        entry=new[name];info=entry['archive'];assert re.fullmatch('[0-9a-f]{64}',info['sha256'])
        data=download(info['url']);assert hashlib.sha256(data).hexdigest()==info['sha256'] and len(data)==info['size']
        path=cache/'modules'/info['sha256']/f"{name}-{entry['version']}.zip";path.parent.mkdir(parents=True);path.write_bytes(data)
    pending=cache/'pending-modules.json'
    request={'envelope':new_envelope.decode(),'old':[old[name] for name in before]}
    invalid=json.loads(new_envelope);invalid['signature']='00'*64
    save(pending,{**request,'envelope':json.dumps(invalid)})
    launch(args.bootstrap);assert versions()==before and pending.exists()
    save(pending,request)
    preferences=json.loads(settings.read_text());preferences['auto_update_modules']=False;save(settings,preferences)
    launch(args.bootstrap);assert versions()==before and pending.exists()
    preferences=json.loads(settings.read_text());preferences['auto_update_modules']=True;save(settings,preferences)
    def preserved():
        assert json.loads((store/'state.json').read_text())['enabled']['python'] is False
        assert docker.read_bytes()==original_docker and (store/'docker/my-settings.txt').read_text()=='Keep my module data'
        assert note.read_text()=='Keep my project notes'
        current=json.loads(settings.read_text());assert current['theme_contrast']==123 and current['font_size']==17.0
    def updated():
        assert versions()=={**before,'python':new['python']['version']},versions()
        assert not pending.exists();preserved()
    launch(args.bootstrap);updated()
    rollback=cache/'rollback-modules.json'
    save(rollback,[{'id':'python','version':old['python']['version']}]);launch(args.exe);assert versions()==before;preserved()
    # Retrying a persisted rollback must not toggle versions.
    save(rollback,[{'id':'python','version':old['python']['version']}]);launch(args.exe);assert versions()==before;preserved()
    save(pending,request);launch(args.exe);updated()
    (ROOT/'target/native-module-update.log').write_text('\n'.join(logs),encoding='utf-8')
    print('PASS native signed modules on published 1.3.0 and current IDE: Python 1.1.1 → 1.1.2 → 1.1.1 → 1.1.2')
    print('Invalid signature rejected; opt-out honored; disabled state, pinned JavaScript, modified Docker, preferences and notes preserved; rollback retry is idempotent')
