"""Load current module manifests with a previously published IDE in an isolated profile."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

parser=argparse.ArgumentParser()
parser.add_argument('--bootstrap',type=Path,required=True)
args=parser.parse_args()
root=Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='light-module-compatibility-') as directory:
    work=Path(directory)
    registry=work/'registry'
    shutil.copytree(root/'registry',registry)
    index=json.loads((registry/'index.json').read_text(encoding='utf-8'))
    # Use the exact checked payloads locally; no network or release availability dependency.
    for module in index['modules']:
        module.pop('archive',None)
    (registry/'index.json').write_text(json.dumps(index),encoding='utf-8')
    config=work/'config'
    config.mkdir()
    (config/'settings.json').write_text(json.dumps({'modules_welcome_done':True,'auto_update_ide':False,'auto_update_modules':False}),encoding='utf-8')
    script=[]
    for filename,language,command in [('main.py','python','Run file'),('main.mjs','javascript','Node: Run file'),('Dockerfile','dockerfile','Docker: Build image')]:
        source=work/filename
        source.write_text('# compatibility fixture\n',encoding='utf-8')
        script.extend([{'op':'open','path':str(source)},{'op':'assert_language','value':language},{'op':'assert_run_options','names':[command]}])
    script.extend([{'op':'assert_modules','installed':['python','javascript','docker'],'enabled':['python','javascript','docker']},{'op':'quit'}])
    path=work/'script.json'
    path.write_text(json.dumps(script),encoding='utf-8')
    env=dict(os.environ,LIGHT_CONFIG_DIR=str(config),LIGHT_REGISTRY=str(registry),LIGHT_PRELOAD_MODULES='python,javascript,docker')
    result=subprocess.run([str(args.bootstrap.resolve()),'--script',str(path),'--no-restore','--size','1280x800'],capture_output=True,env=env,timeout=90)
    log=result.stdout.decode('utf-8',errors='replace')+result.stderr.decode('utf-8',errors='replace')
    (root/'target/module-compatibility.log').write_text(log,encoding='utf-8')
    assert result.returncode==0 and 'SCRIPT OK' in log,log
    for module in ['python','javascript','docker']:
        installed=json.loads((config/'modules'/module/'module.json').read_text(encoding='utf-8'))
        expected=json.loads((registry/'modules'/module/'module.json').read_text(encoding='utf-8'))
        assert installed==expected
    print('PASS published previous IDE: updated Python, JavaScript and Docker install and retain legacy commands; new metadata stays intact')
