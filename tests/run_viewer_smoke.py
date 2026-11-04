"""Exercise real file-only startup without changing source files or the full IDE session."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parent.parent
profile = "release" if "--release" in sys.argv else "debug"
exe = root / "target" / profile / ("light.exe" if os.name == "nt" else "light")
with tempfile.TemporaryDirectory(prefix="light-viewer-smoke-") as directory:
    work = Path(directory)
    config = work / "config"
    config.mkdir()
    settings = config / "settings.json"
    saved = json.dumps({"last_project": "keep-this-project", "open_files": ["keep-this-tab"], "logo": "Light", "external_logo": "Dark"}).encode()
    settings.write_bytes(saved)
    source = work / "code space ~ # % пример.js"
    original = b"function double(x) {\n  return x * 2;\n}\nconsole.log(double(21));\n"
    source.write_bytes(original)
    env = dict(os.environ, LIGHT_CONFIG_DIR=str(config))
    # One screenshot, then repeated starts without screenshot/readback overhead.
    screenshot = root / "target/e2e-shots/quick_view.png"
    screenshot.parent.mkdir(parents=True, exist_ok=True)
    timings = []
    for run in range(3):
        current = dict(env)
        if run == 0:
            current["LIGHT_VIEWER_SCREENSHOT"] = str(screenshot)
        result = subprocess.run([str(exe), "--viewer-smoke", str(source)], capture_output=True, text=True, encoding="utf-8", errors="replace", env=current, timeout=30)
        assert result.returncode == 0, result.stderr
        match = re.search(r"QUICK VIEW READY (\d+) ms", result.stderr)
        assert match, result.stderr
        timings.append(int(match[1]))
        assert source.read_bytes() == original
        assert settings.read_bytes() == saved
        assert sorted(p.name for p in config.iterdir()) == ["settings.json"]
    assert screenshot.is_file()
    print(f"PASS quick view ({profile}): content ready {timings} ms; source and IDE session unchanged")
    (root / f"target/quick-view-{profile}-timings.json").write_text(json.dumps(timings))
