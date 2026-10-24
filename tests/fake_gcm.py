#!/usr/bin/env python3
"""A stand-in for `git credential-manager` used by the UI tests (LIGHT_FAKE_GCM points at this file).

It never opens a browser or touches a real credential store. Accounts live in the JSON file named by
LIGHT_FAKE_GCM_STATE. `github login --browser` follows LIGHT_FAKE_GCM_MODES (comma separated, one entry per call,
the last one repeats): `hang` waits for a minute (the test cancels it), `fail` exits with an error, `ok` adds the
account `octocat`.
"""
import json
import os
import sys
import time

STATE = os.environ.get("LIGHT_FAKE_GCM_STATE") or os.path.join(os.environ.get("TEMP", "."), "light-fake-gcm.json")


def load():
    try:
        with open(STATE, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return {"accounts": [], "calls": 0}


def save(s):
    with open(STATE, "w", encoding="utf-8") as f:
        json.dump(s, f)


def main(argv):
    s = load()
    if argv[:2] == ["github", "list"]:
        for a in s["accounts"]:
            print(a)
        return 0
    if argv[:2] == ["github", "login"]:
        modes = [m for m in os.environ.get("LIGHT_FAKE_GCM_MODES", "ok").split(",") if m]
        mode = modes[min(s.get("calls", 0), len(modes) - 1)]
        s["calls"] = s.get("calls", 0) + 1
        save(s)
        if mode == "hang":
            time.sleep(60)
            return 0
        if mode == "fail":
            sys.stderr.write("error: the browser could not be opened\n")
            return 1
        time.sleep(0.6)
        if "octocat" not in s["accounts"]:
            s["accounts"].append("octocat")
        save(s)
        return 0
    sys.stderr.write("fake gcm: unsupported command\n")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
