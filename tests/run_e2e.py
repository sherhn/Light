#!/usr/bin/env python3
"""End-to-end tests: drive the real Light window with JSON scripts (see apps/light/src/script.rs).

Usage: python tests/run_e2e.py [--release] [name-substring ...]
Each tests/e2e/*.json script runs in a fresh fixture project with an isolated config directory.
Screenshots (taken by the app itself, never of the desktop) are written to target/e2e-shots/.
"""
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import stat
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tests"))
import fake_github
PROFILE = "release" if "--release" in sys.argv else "debug"
EXE = pathlib.Path(os.environ["LIGHT_E2E_EXE"]) if os.environ.get("LIGHT_E2E_EXE") else ROOT / "target" / PROFILE / ("light.exe" if os.name == "nt" else "light")
CLI = ROOT / "target" / PROFILE / ("light-cli.exe" if os.name == "nt" else "light-cli")
SHOTS = ROOT / "target" / "e2e-shots"

FIXTURE_FILES = {
    "src/scratch.lisp": "",
    "src/hello.lisp": '(format t "Hi from run~%")\n',
    "src/warn.lisp": '(defun f (x)\n  (+ x "a"))\n\n(defun g ()\n  (missing-function 1))\n',
    "src/nav.lisp": "(defun alpha (x)\n  (* x 2))\n\n(defun beta (y)\n  (alpha y))\n",
    "src/messy.lisp": "(defun f (x)\n(let ((a 1)\n(b 2))\n(+ a b x)))\n",
    "src/bad.lisp": "(defun broken (x\n  (+ x 1)\n",
    "src/skip.fasl": "compiled",
    "build/out.txt": "generated",
    "scripts/demo.als": "# demo script\necho step-one\ncd src\necho in-src\n-git definitely-not-a-command\n@echo quiet-step\necho after-ignored\n",
    "scripts/fail.als": "echo before-failure\ngit definitely-not-a-command\necho must-not-run\n",
}


def fwd(p):
    return str(p).replace("\\", "/")


def make_fixture(tmp):
    r = subprocess.run([str(CLI), "new-project", str(tmp), "demo"], capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"cannot create fixture: {r.stderr}")
    proj = tmp / "demo"
    for rel, content in FIXTURE_FILES.items():
        (proj / rel).parent.mkdir(parents=True, exist_ok=True)
        (proj / rel).write_text(content, encoding="utf-8")
    # every fixture is a git repository with one commit, so Git features have something to show
    def git(*args):
        subprocess.run(["git", "-c", "commit.gpgsign=false", *args], cwd=proj, capture_output=True, check=True)
    (proj / ".gitignore").write_text("*.fasl\n/build/\n", encoding="utf-8")
    git("init", "-q", "-b", "main")
    git("config", "user.email", "e2e@example.com")
    git("config", "user.name", "E2E")
    git("config", "core.autocrlf", "false")
    git("add", "-A")
    git("commit", "-q", "-m", "initial commit")
    return proj


def run_one(script_path):
    name = script_path.stem
    tmp = pathlib.Path(tempfile.mkdtemp(prefix=f"light-e2e-{name}-"))
    try:
        proj = make_fixture(tmp)
        text = script_path.read_text(encoding="utf-8")
        text = text.replace("${GH_TOKEN}", fake_github.FAKE_TOKEN).replace("${PROJ}", fwd(proj)).replace("${SHOTS}", fwd(SHOTS)).replace("${NAME}", name).replace("${CFG}", fwd(tmp / "cfg")).replace("${SEP}", "\\\\" if os.name == "nt" else "/")
        if "@needs_python" in text and not (shutil.which("py") or shutil.which("python3")):
            # a real interpreter is what the script checks files with
            return name, True, "skipped: no Python on this machine", 0.0
        script = tmp / "script.json"
        script.write_text(text, encoding="utf-8")
        json.loads(text)  # validate
        if "@javascript" in text:
            web = proj / "web"
            web.mkdir(exist_ok=True)
            (web / "main.mjs").write_text('export const double = (x) => x * 2;\nconsole.log("javascript module ready", double(3));\n', encoding="utf-8")
            (web / "package.json").write_text(json.dumps({"name": "light-test", "private": True, "type": "module", "scripts": {"test": "node main.mjs", "build": "node main.mjs"}}), encoding="utf-8")
        if "@docker_module" in text:
            (proj / "Dockerfile").write_text("FROM busybox AS base\nWORKDIR /app\nCMD [\"echo\", \"fixture\"]\n", encoding="utf-8")
            (proj / "compose.yaml").write_text("services:\n  web:\n    image: busybox\n    command: [echo, fixture]\n", encoding="utf-8")
        if "@os_project" in text:
            # a small kernel-like project: a makefile, C and assembly, built in Linux
            files = {
                "makefile": "SUBDIRS = portable\nall: bin/kernel\nbin/kernel: $(SUBDIRS)\n\tld -m elf_i386 -o bin/kernel portable/*.o\n$(SUBDIRS):\n\t$(MAKE) -C $@\nrun: bin/kernel\n\tqemu-system-i386 -kernel bin/kernel\nclean:\n\trm -f bin/kernel portable/*.o\n",
                "portable/eval.c": "#include <stdio.h>\nint eval(int a) { // a comment; with a semicolon (and a paren\n    return x + a;\n}\n",
                "x86/boot.asm": "section .text ; entry\nglobal start\nstart:\n    mov eax, 0x10\n",
            }
            for rel, content in files.items():
                (proj / rel).parent.mkdir(parents=True, exist_ok=True)
                (proj / rel).write_bytes(content.encode("utf-8"))
            subprocess.run(["git", "-c", "commit.gpgsign=false", "add", "-A"], cwd=proj, capture_output=True, check=True)
            subprocess.run(["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "kernel"], cwd=proj, capture_output=True, check=True)
        if "@polyglot" in text:
            # files of several languages, for the modules that bring them
            files = {
                "tools/demo.py": "def greet(name):\n    return f\"hi {name}\"  # greet\n",
                "docs/notes.md": "# Notes\n\nSome **bold** text.\n",
                "docs/index.rst": "Title\n=====\n\nText.\n",
                "native/main.c": "#include <stdio.h>\nint main(void) { return 0; }\n",
                "native/boot.asm": "section .text\n_start:\n    mov eax, 1\n",
            }
            for rel, content in files.items():
                (proj / rel).parent.mkdir(parents=True, exist_ok=True)
                (proj / rel).write_bytes(content.encode("utf-8"))
            subprocess.run(["git", "-c", "commit.gpgsign=false", "add", "-A"], cwd=proj, capture_output=True, check=True)
            subprocess.run(["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "polyglot"], cwd=proj, capture_output=True, check=True)
        if "@markup" in text:
            # Markdown and reStructuredText documents with everything a preview shows, and a picture next to them
            site = proj / "site"
            site.mkdir(exist_ok=True)
            for f in (ROOT / "tests" / "fixtures" / "markup").iterdir():
                shutil.copy2(f, site / f.name)
            subprocess.run(["git", "-c", "commit.gpgsign=false", "add", "-A"], cwd=proj, capture_output=True, check=True)
            subprocess.run(["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "markup"], cwd=proj, capture_output=True, check=True)
        if "@untracked_files" in text:
            # new files after the initial commit: two that .gitignore names (`*.fasl`, `/build/`) and one it does not
            for rel, content in {"src/generated.fasl": "compiled", "build/output.bin": "generated", "src/fresh.lisp": "(print 1)\n"}.items():
                (proj / rel).parent.mkdir(parents=True, exist_ok=True)
                (proj / rel).write_text(content, encoding="utf-8")
        if "@hover" in text:
            # documented functions in Lisp, Python and C, and a call of one of them from another file
            for f in (ROOT / "tests" / "fixtures" / "hover").rglob("*"):
                if f.is_file():
                    dest = proj / f.relative_to(ROOT / "tests" / "fixtures" / "hover")
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(f, dest)
            subprocess.run(["git", "-c", "commit.gpgsign=false", "add", "-A"], cwd=proj, capture_output=True, check=True)
            subprocess.run(["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "hover"], cwd=proj, capture_output=True, check=True)
        if "@echo_module" in text:
            # a module of its own with a run command that needs no tools: `echo`, on Windows and in the fake Linux alike
            files = {"tools/hello.zz": "say hello\n"}
            for rel, content in files.items():
                (proj / rel).parent.mkdir(parents=True, exist_ok=True)
                (proj / rel).write_bytes(content.encode("utf-8"))
            subprocess.run(["git", "-c", "commit.gpgsign=false", "add", "-A"], cwd=proj, capture_output=True, check=True)
            subprocess.run(["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "echo"], cwd=proj, capture_output=True, check=True)
            echo = tmp / "cfg" / "modules" / "echo"
            echo.mkdir(parents=True)
            (echo / "module.json").write_text(json.dumps({
                "id": "echo", "name": "Echo", "version": "1.0.0", "category": "Languages", "summary": "a test language",
                "languages": [{"id": "echo", "name": "Echo", "extensions": ["zz"], "brackets": ["()"], "check": ["brackets"], "grammar": [{"match": r"\bhello\b", "class": "keyword"}]}],
                "run": [{"language": "echo", "name": "Say", "command": "echo ran {name} in {stem}"}],
                # a check that always finds the same problem: `echo` prints it the way a compiler would
                "check": [{"language": "echo", "name": "Demo", "command": "echo {name}:1:5: error: a demo problem", "pattern": r"^(?P<file>[^:\n]+):(?P<line>\d+):(?P<col>\d+): (?P<sev>\w+): (?P<msg>.*)$"}],
            }), encoding="utf-8")
        if "@linux_distro" in text:
            (tmp / "fake-wsl.json").write_text(json.dumps({"installed": True, "distros": {"Ubuntu-24.04": {"sbcl": False}}, "default": "Ubuntu-24.04"}), encoding="utf-8")
        if "@long_file" in text:
            # a long committed file, so the gutter, the overview and the scrollbar have something to show
            forms = [f"(defun fn-{i} (x)\n  \"Step {i}.\"\n  (let ((y (* x {i})))\n    (format t \"~a~%\" (+ y {i}))))\n" for i in range(1, 121)]
            (proj / "src" / "long.lisp").write_text("\n".join(forms), encoding="utf-8")
            subprocess.run(["git", "-c", "commit.gpgsign=false", "add", "-A"], cwd=proj, capture_output=True, check=True)
            subprocess.run(["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "long file"], cwd=proj, capture_output=True, check=True)
        if "@branches" in text:
            # a history with branches, a merge and a tag, for the commit graph
            def g(*args):
                subprocess.run(["git", "-c", "commit.gpgsign=false", *args], cwd=proj, capture_output=True, check=True)

            g("checkout", "-q", "-b", "feature")
            (proj / "feature.txt").write_text("f1\n", encoding="utf-8")
            g("add", "-A")
            g("commit", "-q", "-m", "feature: first step")
            (proj / "feature.txt").write_text("f1\nf2\n", encoding="utf-8")
            g("commit", "-q", "-am", "feature: second step")
            g("checkout", "-q", "main")
            (proj / "main.txt").write_text("m1\n", encoding="utf-8")
            g("add", "-A")
            g("commit", "-q", "-m", "main: independent work")
            g("merge", "-q", "--no-ff", "feature", "-m", "Merge branch feature")
            g("tag", "v0.1")
            g("checkout", "-q", "-b", "hotfix", "HEAD~1")
            (proj / "fix.txt").write_text("x\n", encoding="utf-8")
            g("add", "-A")
            g("commit", "-q", "-m", "hotfix: patch")
            g("checkout", "-q", "main")
        if "@no_git" in text:
            def _unlock(func, path, exc):
                os.chmod(path, stat.S_IWRITE)
                func(path)

            shutil.rmtree(proj / ".git", onexc=_unlock)
        env = dict(os.environ, LIGHT_CONFIG_DIR=str(tmp / "cfg"))
        if "@docker_module" in text:
            import shlex
            commands = tmp / "fake-cli"
            commands.mkdir()
            if os.name == "nt":
                (commands / "docker.cmd").write_text(f'@echo off\r\n"{sys.executable}" "{ROOT / "tests/fake_docker.py"}" %*\r\n', encoding="utf-8")
            else:
                wrapper = commands / "docker"
                wrapper.write_text(f'#!/bin/sh\nexec {shlex.quote(sys.executable)} {shlex.quote(str(ROOT / "tests/fake_docker.py"))} "$@"\n', encoding="utf-8")
                wrapper.chmod(0o755)
            env["PATH"] = str(commands) + os.pathsep + env.get("PATH", "")
        # modules come from the registry of this repository; a script gets Lisp, plus what `@modules a,b` names (a kernel
        # project also C, assembler and make), and `@no_modules` starts with none
        env["LIGHT_REGISTRY"] = str(ROOT / "registry")
        preload = ["lisp"]
        tag = re.search(r"@modules ([\w,-]+)", text)
        if tag:
            preload += tag.group(1).split(",")
        if "@os_project" in text:
            preload += ["c-cpp", "assembler", "shell-make"]
        if "@no_modules" in text:
            preload = []
        env["LIGHT_PRELOAD_MODULES"] = ",".join(dict.fromkeys(preload))
        # the tests never read the real GitHub login (gh / Git credential store) nor call api.github.com
        env["LIGHT_GITHUB_AUTODETECT"] = "0"
        env.pop("LIGHT_GITHUB_FAKE_TOKEN", None)
        github = None
        if "github" in name:
            remotes = tmp / "remotes"
            paths = {
                "lisp-demo": fake_github.make_remote(remotes, "lisp-demo", {"src/main.lisp": "(defun hello () :from-github)\n", "README.md": "demo\n"}),
                "private-notes": fake_github.make_remote(remotes, "private-notes", {"notes.txt": "secret notes\n"}),
            }
            github = fake_github.Server(paths).start()
            env["LIGHT_GITHUB_API"] = github.url
            # a fake Git Credential Manager: the browser sign-in hangs (cancelled), fails, then works
            env["LIGHT_FAKE_GCM"] = str(ROOT / "tests" / "fake_gcm.py")
            env["LIGHT_FAKE_GCM_PYTHON"] = sys.executable
            env["LIGHT_FAKE_GCM_STATE"] = str(tmp / "fake-gcm.json")
            env["LIGHT_FAKE_GCM_MODES"] = "hang,fail,ok"
            env["LIGHT_FAKE_GCM_TOKEN"] = fake_github.FAKE_TOKEN
            if "saved" in name:
                # "a login that is already on this computer" without touching the real one
                env["LIGHT_GITHUB_FAKE_TOKEN"] = fake_github.FAKE_TOKEN
        # the tests never touch the real WSL: a fake stands in for wsl.exe (scripts named *wsl* start without WSL)
        env["LIGHT_FAKE_WSL"] = str(ROOT / "tests" / "fake_wsl.py")
        env["LIGHT_FAKE_WSL_PYTHON"] = sys.executable
        env["LIGHT_FAKE_WSL_STATE"] = str(tmp / "fake-wsl.json")
        env["LIGHT_FAKE_WSL_START"] = "absent" if "wsl" in name else "installed"
        if "@root_wsl_user" in text:
            env["LIGHT_FAKE_WSL_PROJECT_USER"] = "root"
        if shutil.which("sbcl"):
            env.setdefault("LIGHT_FAKE_WSL_SBCL", shutil.which("sbcl"))
        if os.environ.get("LIGHT_E2E_STRIP_SBCL"):
            # prove that the bundled SBCL is used: no system SBCL on PATH, no explicit override
            env.pop("LIGHT_SBCL", None)
            sep = os.pathsep
            env["PATH"] = sep.join(p for p in env.get("PATH", "").split(sep) if "sbcl" not in p.lower() and "steel bank" not in p.lower())
        t0 = time.time()
        launch = [str(EXE), "--script", str(script), "--size", "1280x800"]
        if "@restore_session" in text or "@restore_without_project" in text:
            config = tmp / "cfg"
            config.mkdir(exist_ok=True)
            if "@restore_without_project" in text:
                scratch = config / "scratches/scratch-1.txt"
                scratch.parent.mkdir()
                scratch.write_text("retained scratch without a project\n", encoding="utf-8")
                settings = {"open_files": [fwd(scratch)], "active_file": fwd(scratch), "modules_welcome_done": True}
            else:
                settings = {"last_project": fwd(proj), "recent_projects": [fwd(proj)], "open_files": [fwd(proj / "src/main.lisp"), fwd(proj / "src/nav.lisp")], "active_file": fwd(proj / "src/nav.lisp"), "modules_welcome_done": True}
            (config / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
        else:
            launch.append("--no-restore")
        try:
            p = subprocess.run(launch, capture_output=True, text=True, timeout=180, env=env)
        except subprocess.TimeoutExpired as e:
            return name, False, f"TIMEOUT after 180s\n{(e.stdout or b'').decode(errors='replace') if isinstance(e.stdout, bytes) else e.stdout}", time.time() - t0
        out = (p.stdout or "") + (p.stderr or "")
        if os.environ.get("LIGHT_E2E_VERBOSE"):
            print(out)
        ok = p.returncode == 0 and "SCRIPT OK" in out
        return name, ok, out, time.time() - t0
    finally:
        if github is not None:
            github.stop()
        if os.environ.get("KEEP"):
            print(f"  kept fixture: {tmp}")
        else:
            shutil.rmtree(tmp, ignore_errors=True)


def main():
    if not EXE.exists() or not CLI.exists():
        raise SystemExit(f"build first: cargo build{' --release' if PROFILE == 'release' else ''} -p light -p light-cli")
    SHOTS.mkdir(parents=True, exist_ok=True)
    filters = [a for a in sys.argv[1:] if not a.startswith("--")]
    scripts = sorted((ROOT / "tests" / "e2e").glob("*.json"))
    if filters:
        scripts = [s for s in scripts if any(f in s.stem for f in filters)]
    failed = []
    for s in scripts:
        name, ok, out, dt = run_one(s)
        print(f"{'PASS' if ok else 'FAIL'}  {name:<22} {dt:5.1f}s")
        if not ok:
            failed.append(name)
            for line in out.strip().splitlines()[-25:]:
                print("      " + line)
    print(f"\n{len(scripts) - len(failed)}/{len(scripts)} e2e scripts passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
