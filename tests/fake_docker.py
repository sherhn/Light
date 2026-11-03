"""A Docker CLI fixture. Never connects to a daemon or touches real images/containers."""
import json
import os
import pathlib
import sys
import time

args = sys.argv[1:]

# containers with a state of their own (LIGHT_FAKE_DOCKER_STATE): the Services window lists, starts, stops and removes
# them and follows their logs
STATE = os.environ.get("LIGHT_FAKE_DOCKER_STATE")
if STATE and args and args[0] in ("ps", "start", "stop", "restart", "rm", "logs") and ("--format" in args or args[0] != "ps"):
    path = pathlib.Path(STATE)
    if not path.exists():
        path.write_text(json.dumps([
            {"ID": "a1b2c3d4e5f6", "Names": "demo-web-1", "Image": "nginx:1.27", "State": "running", "Status": "Up 5 minutes", "Ports": "0.0.0.0:8080->80/tcp", "Labels": "com.docker.compose.project=demo,com.docker.compose.service=web"},
            {"ID": "0f9e8d7c6b5a", "Names": "demo-db-1", "Image": "postgres:16", "State": "exited", "Status": "Exited (0) 1 hour ago", "Ports": "", "Labels": "com.docker.compose.project=demo,com.docker.compose.service=db"},
            {"ID": "1234567890ab", "Names": "scratch", "Image": "busybox", "State": "exited", "Status": "Exited (1) 2 days ago", "Ports": "", "Labels": ""},
        ]))
    containers = json.loads(path.read_text())
    find = lambda key: next((c for c in containers if key in (c["ID"], c["Names"])), None)
    verb = args[0]
    if verb == "ps":
        for c in containers:
            print(json.dumps(c))
    elif verb == "logs":
        c = find(args[-1])
        if c is None:
            print(f"Error: No such container: {args[-1]}", file=sys.stderr)
            sys.exit(1)
        for i in range(1, 4):
            print(f"{c['Names']} log line {i}", flush=True)
        if ("-f" in args or "--follow" in args) and c["State"] == "running":
            n = 0
            while True:
                n += 1
                time.sleep(0.5)
                print(f"{c['Names']} tick {n}", flush=True)
    else:
        for key in [a for a in args[1:] if not a.startswith("-")]:
            c = find(key)
            if c is None:
                print(f"Error response from daemon: No such container: {key}", file=sys.stderr)
                sys.exit(1)
            if verb == "rm":
                containers.remove(c)
            elif verb == "stop":
                c["State"], c["Status"] = "exited", "Exited (0) Less than a second ago"
            else:
                c["State"], c["Status"] = "running", "Up Less than a second"
            print(key)
        path.write_text(json.dumps(containers))
    sys.exit(0)
if "-f" in args:
    file = pathlib.Path(args[args.index("-f") + 1])
    if not file.is_file():
        print("line 1: configuration file is missing")
        sys.exit(1)
    if "config" in args:
        if "INVALID" in file.read_text(encoding="utf-8"):
            print("yaml: line 4: invalid compose configuration")
            sys.exit(1)
        sys.exit(0)
print("fake-docker " + " ".join(args))
if args[:1] == ["ps"]:
    print("CONTAINER ID   IMAGE   STATUS   NAMES\nfixture123     demo    running  test-web")
elif args[:2] == ["image", "ls"]:
    print("REPOSITORY   TAG\ndemo         latest")
