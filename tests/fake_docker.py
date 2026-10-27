"""A Docker CLI fixture. Never connects to a daemon or touches real images/containers."""
import pathlib
import sys

args = sys.argv[1:]
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
