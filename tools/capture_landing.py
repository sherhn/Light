"""Capture real Light Studio screenshots for the landing page. Build first: cargo build -p light.
Uses an isolated sample project and settings under target/light-capture.
Run from any directory with Python 3: python tools/capture_landing.py.
"""
import json, pathlib, subprocess, os, argparse
root=pathlib.Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--site',type=pathlib.Path,default=root.parent/'Light-site',help='Standalone Light website directory')
args=parser.parse_args()
work=root/'target/light-capture'
project=work/'observatory'
(project/'src').mkdir(parents=True,exist_ok=True)
(project/'tests').mkdir(exist_ok=True)
(project/'docs').mkdir(exist_ok=True)
(project/'src/pipeline.py').write_text('''from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


@dataclass
class Reading:
    station: str
    temperature: float
    active: bool = True


def summarize(readings: Iterable[Reading]) -> dict[str, float]:
    """Collect active stations and calculate their mean reading."""
    values = [item.temperature for item in readings if item.active]
    if not values:
        return {"stations": 0, "average": 0.0}

    return {
        "stations": len(values),
        "average": round(sum(values) / len(values), 2),
    }


def main() -> None:
    readings = [
        Reading("North", 18.4),
        Reading("Central", 21.6),
        Reading("South", 23.0),
    ]
    result = summarize(readings)
    print(f"Stations online: {result['stations']}")
    print(f"Average temperature: {result['average']} C")


if __name__ == "__main__":
    main()
''',encoding='utf8')
(project/'README.md').write_text('# Observatory\n\nA small Python workspace for sensor readings.\n\n## Run\n\npython src/pipeline.py\n',encoding='utf8')
(project/'tests/test_pipeline.py').write_text('from src.pipeline import Reading, summarize\n\ndef test_empty():\n    assert summarize([])["stations"] == 0\n',encoding='utf8')
(project/'docs/notes.md').write_text('# Development notes\n\n- [x] Read station data\n- [x] Calculate an average\n- [ ] Add CSV export\n',encoding='utf8')
(project/'pyproject.toml').write_text('[project]\nname = "observatory"\nversion = "0.1.0"\nrequires-python = ">=3.10"\n',encoding='utf8')
if not (project/'.git').exists():
    for args in [['init','-q','-b','main'],['add','.'],['-c','user.name=Studio demo','-c','user.email=studio@example.com','-c','commit.gpgsign=false','commit','-qm','Create sample Python workspace']]:
        subprocess.run(['git',*args],cwd=project,check=True,capture_output=True)
cfg=work/'config';cfg.mkdir(exist_ok=True)
(cfg/'settings.json').write_text(json.dumps({'theme':'Dark','language':'English','font_size':14,'modules_welcome_done':True}),encoding='utf8')
asset=args.site.resolve()/'dist/assets'
asset.mkdir(parents=True,exist_ok=True)
script=[{'op':'open_project','path':project.as_posix()},{'op':'project_env','value':'windows'},{'op':'open','path':(project/'README.md').as_posix()},{'op':'open','path':(project/'src/pipeline.py').as_posix()},{'op':'expand','path':(project/'src').as_posix()},{'op':'expand','path':(project/'tests').as_posix()},{'op':'wait','what':'index'},{'op':'assert_language','value':'python'},{'op':'theme','value':'dark'},{'op':'assert_theme','value':'dark','colors':{'accent':'#B3D6AD','editor_bg':'#121416'}},{'op':'key','value':'Alt+F12'},{'op':'frames','value':6},{'op':'text','value':'python src/pipeline.py'},{'op':'key','value':'Enter'},{'op':'wait','what':'term_idle','timeout':30},{'op':'frames','value':60},{'op':'pointer_gone'},{'op':'sleep','value':5},{'op':'screenshot','path':(asset/'light-studio-dark.png').as_posix()},{'op':'theme','value':'light'},{'op':'assert_theme','value':'light','colors':{'accent':'#46674B','editor_bg':'#F8FAF6'}},{'op':'frames','value':15},{'op':'screenshot','path':(asset/'light-studio-light.png').as_posix()},{'op':'theme','value':'dark'},{'op':'key','value':'Alt+F12'},{'op':'command','value':'app.modules'},{'op':'wait','what':'modules_index','timeout':30},{'op':'wait','what':'modules_idle','timeout':30},{'op':'frames','value':15},{'op':'screenshot','path':(asset/'light-studio-modules.png').as_posix()},{'op':'quit'}]
script_path=work/'capture.json';script_path.write_text(json.dumps(script),encoding='utf8')
env=dict(os.environ,LIGHT_CONFIG_DIR=str(cfg),LIGHT_REGISTRY=str(root/'registry'),LIGHT_PRELOAD_MODULES='python,markdown',LIGHT_GITHUB_AUTODETECT='0')
env['PATH']=str(pathlib.Path(os.sys.executable).parent)+os.pathsep+env['PATH']
result=subprocess.run([str(root/'target/debug/light.exe'),'--script',str(script_path),'--size','1440x900','--no-restore'],env=env,capture_output=True,text=True,timeout=150)
print(result.stdout);print(result.stderr);raise SystemExit(result.returncode)
