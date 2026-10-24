#!/usr/bin/env python3
"""A stand-in for wsl.exe used by the UI tests (LIGHT_FAKE_WSL points at this file).

It never touches the real WSL. `make TARGET` "inside" a distribution prints what a kernel build with one compiler
error looks like (and exits 2); the build tools of a distribution can be "installed" like SBCL. Its state (is WSL "installed", which distributions exist, which have SBCL) lives in
the JSON file named by LIGHT_FAKE_WSL_STATE; LIGHT_FAKE_WSL_START=absent starts without WSL. Programs "inside"
a distribution run on Windows: /mnt/<drive>/... paths are mapped back, `sbcl` is the Windows SBCL
(LIGHT_FAKE_WSL_SBCL or PATH) and `sh -lc LINE` becomes `cmd /c LINE`.
"""
import json
import os
import re
import shutil
import subprocess
import sys
import time

STATE = os.environ.get("LIGHT_FAKE_WSL_STATE") or os.path.join(os.environ.get("TEMP", "."), "light-fake-wsl.json")
ONLINE = [("Debian", "Debian GNU/Linux"), ("Ubuntu", "Ubuntu"), ("Ubuntu-24.04", "Ubuntu 24.04 LTS"), ("Ubuntu-22.04", "Ubuntu 22.04 LTS"), ("archlinux", "Arch Linux")]


def load():
    try:
        with open(STATE, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return {"installed": os.environ.get("LIGHT_FAKE_WSL_START") != "absent", "distros": {}, "default": None}


def save(s):
    with open(STATE, "w", encoding="utf-8") as f:
        json.dump(s, f)


def out(text):
    sys.stdout.write(text)
    sys.stdout.flush()


def progress(label):
    out(f"{label}\n")
    for p in (0, 25, 50, 75, 100):
        bar = "=" * (p // 5)
        out(f"\r[{bar:<20}] {p}.0%")
        time.sleep(0.12)
    out("\n")


def to_windows(arg):
    if arg.startswith("/mnt/") and len(arg) >= 6 and arg[5].isalpha() and (len(arg) == 6 or arg[6] == "/"):
        return arg[5].upper() + ":/" + arg[7:]
    return arg


def no_distro(name):
    out(f"There is no distribution with the supplied name: {name}\n")
    return 255


def fake_apt(packages, version_lines):
    """What the setup scripts print on Ubuntu: the markers of the script, then apt's own lines (slowly enough for the
    progress bar of the UI to be seen)."""
    out("@@light stage prepare\n")
    for n in range(1, 7):
        out(f"Get:{n} http://archive.ubuntu.com/ubuntu noble InRelease [126 kB]\n")
        time.sleep(0.06)
    out("Reading package lists... Done\n@@light stage install\n")
    out("Reading package lists...\nThe following NEW packages will be installed:\n  " + " ".join(packages) + "\n")
    out(f"0 upgraded, {len(packages)} newly installed, 0 to remove and 4 not upgraded.\nNeed to get 31.4 MB of archives.\n")
    for i, p in enumerate(packages, 1):
        out(f"Get:{i} http://archive.ubuntu.com/ubuntu noble/main amd64 {p} amd64 1.0 [1,204 kB]\n")
        time.sleep(0.08)
    for p in packages:
        out(f"Selecting previously unselected package {p}.\nUnpacking {p} (1.0) ...\n")
        time.sleep(0.08)
    for p in packages:
        out(f"Setting up {p} (1.0) ...\n")
        time.sleep(0.08)
    out("@@light stage verify\n")
    for line in version_lines:
        out(line + "\n")


TOOLS = ["make", "gcc", "ld", "nasm", "gdb", "git", "qemu-system-i386", "qemu-system-x86_64"]
# every program the developer tools check looks for (see essential_groups and the modules in the registry)
ALL_PROGRAMS = TOOLS + ["g++", "cmake", "ninja", "pkg-config", "clang", "clangd", "lldb", "clang-format", "valgrind", "strace", "ltrace", "gdb-multiarch", "xxd", "curl", "wget", "ssh", "rsync", "zip", "unzip", "xz", "jq", "rg", "fdfind", "tree", "vim", "nano", "htop", "man", "doxygen", "python3", "pip3", "qemu-img", "xorriso", "grub-mkrescue"]


# the programs a package brings when they are not named like it
PACKAGE_PROGRAMS = {
    "build-essential": ["make", "gcc", "g++", "ld"], "ripgrep": ["rg"], "fd-find": ["fdfind"], "openssh-client": ["ssh"],
    "xz-utils": ["xz"], "man-db": ["man"], "python3-pip": ["pip3"], "qemu-system-x86": ["qemu-system-i386", "qemu-system-x86_64"],
}
# the first loop of the install script of the developer tools: the packages that are asked of apt
TOOLS_LOOP = re.compile(r"for p in ([^;]*); do\n\s+if apt-cache show")


def package_found(d, name):
    """Whether the check of the developer tools finds `name` (a program, or `dpkg:PACKAGE`) in the distribution."""
    packages = d.get("packages", [])
    if name.startswith("dpkg:"):
        return name[5:] in packages
    if d.get("tools") and name in ALL_PROGRAMS or name == "ld":
        return True
    return name in packages or any(name in PACKAGE_PROGRAMS.get(p, []) for p in packages)


def fake_make(lcwd, target, merged):
    """A kernel build: the portable/ subdirectory fails on an undeclared variable. `merged`: 2>&1 was asked for."""
    err = sys.stdout if merged else sys.stderr
    if target == "clean":
        out("rm -f bin/kernel portable/*.o\n")
        return 0
    out("make -C portable\n")
    out(f"make[1]: Entering directory '{lcwd}/portable'\n")
    out("gcc -g -m32 -c eval.c\n")
    err.write("eval.c:3:12: error: 'x' undeclared (first use in this function)\n")
    err.flush()
    out(f"make[1]: Leaving directory '{lcwd}/portable'\n")
    err.write("make: *** [makefile:4: portable] Error 2\n")
    err.flush()
    return 2


def run_inside(s, name, cwd, prog, args):
    d = s["distros"].get(name)
    if d is None:
        return no_distro(name)
    lcwd = cwd
    cwd = to_windows(cwd) if cwd else None
    if prog == "wslpath":
        out("/mnt/c/\n")
        return 0
    if prog == "sh" and args[:1] == ["-c"] and "pgrep -" in (args[1] if len(args) > 1 else ""):  # not "ripgrep"
        # the cleanup of a tagged process: end the Windows programs carrying the marker
        marker = (args[3] if len(args) > 3 else "").replace("[", "").replace("]", "")
        if marker:
            ps = ("Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'sbcl.exe' -or $_.Name -eq 'cmd.exe') -and $_.CommandLine -like '*" + marker + "*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }")
            subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True)
        return 0
    if prog == "sh" and args[:1] == ["-c"]:
        script = args[1] if len(args) > 1 else ""
        loop = TOOLS_LOOP.search(script)
        if loop:
            packages = loop.group(1).split()
            if "gcc-multilib" in packages:
                # everything the modules ask for: the full install, after which every tool is there
                fake_apt(["build-essential", "gcc-multilib", "nasm", "gdb", "git", "qemu-system-x86", "qemu-system-common", "libvirglrenderer1", "seabios", "ipxe-qemu"], ["GNU Make 4.3", "gcc (Ubuntu 13.2.0) 13.2.0"])
                d["tools"] = True
            else:
                # a choice: only these packages come, and `make` and `gcc` are told only if they are there
                fake_apt(packages, ["GNU Make 4.3"] if "build-essential" in packages else [])
            d["packages"] = sorted(set(d.get("packages", [])) | set(packages))
            d["last_install"] = packages
            save(s)
            return 0
        if "cd ~ && pwd" in script:
            out("/home/user\n")
            return 0
        if "apt-get install -y sbcl" in script:
            fake_apt(["libzstd1", "libffi8", "sbcl"], ["SBCL 2.2.9.debian"])
            d["sbcl"] = True
            save(s)
            return 0
        out("fake wsl: unsupported script\n")
        return 1
    checked = re.findall(r'echo "([^"=]+)=\$\((?:command -v|dpkg-query)', args[1]) if prog == "sh" and args[:1] == ["-lc"] and len(args) > 1 else []
    if checked:
        # the check of the developer tools: one `NAME=...` line per program (or `dpkg:PACKAGE=...`) of its script
        for name in checked:
            found = package_found(d, name)
            out(f"{name}=" + (("install ok installed" if name.startswith("dpkg:") else f"/usr/bin/{name}") if found else "") + "\n")
        return 0
    if prog == "sh" and args[:1] == ["-lc"] and "git config --global" in (args[1] if len(args) > 1 else ""):
        out("name: E2E\nemail: e2e@example.com\ncredentials: !'/mnt/c/Program Files/Git/mingw64/bin/git-credential-manager.exe'\n")
        return 0
    if prog == "sh" and args[:1] == ["-lc"] and "LIGHT_SBCL=" in (args[1] if len(args) > 1 else ""):
        out("LIGHT_SBCL=" + ("/usr/bin/sbcl" if d.get("sbcl") else "") + "\n")
        if d.get("sbcl"):
            out("LIGHT_VERSION=SBCL 2.2.9.debian\n")
        out("LIGHT_MOUNT=/mnt/c/\n")
        return 0
    env = dict(os.environ)
    if prog == "env":
        while args and "=" in args[0] and not args[0].startswith("-"):
            k, v = args.pop(0).split("=", 1)
            env[k] = v
        prog = args.pop(0) if args else ""
    if prog == "sh" and args[:1] == ["-lc"]:
        line = args[1] if len(args) > 1 else ""
        marker = args[2] if len(args) > 2 else ""
        words = line.split()
        first = words[0] if words else ""
        if first in TOOLS and first != "ld":
            if not d.get("tools"):
                # what dash says when the program is not installed and a Windows folder of PATH cannot be read
                sys.stderr.write(f"{marker}: 1: {first}: Permission denied\n")
                sys.stderr.flush()
                return 127
            if first == "make":
                target = words[1].strip("'") if len(words) > 1 and not words[1].startswith("2>") else "all"
                return fake_make(lcwd or "/", target, "2>&1" in line)
        return subprocess.call(f"cmd /d /c {line} & rem {marker}", cwd=cwd, env=env)
    if prog.endswith("sbcl"):
        if not d.get("sbcl"):
            out(f"execvpe({prog}) failed: No such file or directory\n")
            return 1
        sbcl = os.environ.get("LIGHT_FAKE_WSL_SBCL") or shutil.which("sbcl")
        if not sbcl:
            out("fake wsl: no Windows SBCL to stand in for Linux SBCL\n")
            return 1
        return subprocess.call([sbcl] + [to_windows(a) for a in args], cwd=cwd, env=env)
    out(f"fake wsl: cannot run {prog}\n")
    return 1


def main(argv):
    s = load()
    if not argv:
        return 0
    if argv[0] == "--version":
        if not s["installed"]:
            out("Usage: wsl.exe [Argument] [Options...] [CommandLine]\n")
            return 1
        out("WSL version: 2.6.1.0\nKernel version: 6.6.87.2-1\n")
        return 0
    if argv[0] == "--status":
        if not s["installed"]:
            out("The Windows Subsystem for Linux is not installed.\n")
            return 1
        out("Default Version: 2\n")
        return 0
    if argv[0] in ("--list", "-l"):
        rest = argv[1:]
        if "--online" in rest or "-o" in rest:
            out("The following is a list of valid distributions that can be installed.\n\nNAME            FRIENDLY NAME\n")
            for n, f in ONLINE:
                out(f"{n:<16}{f}\n")
            return 0
        if not s["distros"]:
            out("Windows Subsystem for Linux has no installed distributions.\n")
            return 255
        out("  NAME            STATE           VERSION\n")
        for n in s["distros"]:
            star = "*" if n == s.get("default") else " "
            out(f"{star} {n:<15} Stopped         2\n")
        return 0
    if argv[0] == "--install":
        rest = argv[1:]
        if "--no-distribution" in rest:
            progress("Installing: Windows Subsystem for Linux")
            s["installed"] = True
            save(s)
            out("The requested operation is successful.\n")
            return 0
        if not s["installed"]:
            out("WSL is not installed.\n")
            return 1
        name = None
        if "-d" in rest:
            name = rest[rest.index("-d") + 1]
        if "--name" in rest:
            name = rest[rest.index("--name") + 1]
        if not name:
            out("fake wsl: no distribution given\n")
            return 1
        if name in s["distros"]:
            out(f"A distribution with the supplied name already exists: {name}\n")
            return 1
        progress(f"Downloading: {name}")
        out(f"Installing: {name}\nDistribution successfully installed.\n")
        s["distros"][name] = {"sbcl": False}
        s["default"] = s.get("default") or name
        save(s)
        return 0
    if argv[0] == "--import":
        name = argv[1]
        s["distros"][name] = {"sbcl": False}
        s["default"] = s.get("default") or name
        save(s)
        out("The operation completed successfully.\n")
        return 0
    # -d NAME [-u USER] [--cd DIR] -e PROGRAM ARGS...
    name, cwd, i = s.get("default"), None, 0
    while i < len(argv):
        a = argv[i]
        if a in ("-d", "--distribution"):
            name = argv[i + 1]
            i += 2
        elif a in ("-u", "--user"):
            i += 2
        elif a == "--cd":
            cwd = argv[i + 1]
            i += 2
        elif a in ("-e", "--exec"):
            return run_inside(s, name, cwd, argv[i + 1], list(argv[i + 2:]))
        else:
            out(f"fake wsl: unexpected argument {a}\n")
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
