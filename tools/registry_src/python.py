from common import call, match, module, outline, region, words

KEYWORDS = """and as assert async await break class continue def del elif else except finally for from global if import in is lambda
nonlocal not or pass raise return try while with yield match case""".split()

BUILTINS = """print len range int str float list dict set tuple bool bytes bytearray type isinstance issubclass enumerate zip map filter sum min
max abs open sorted reversed any all input super object repr hash id iter next round pow divmod format vars dir getattr setattr hasattr
callable chr ord hex oct bin slice property staticmethod classmethod frozenset memoryview complex compile eval exec globals locals
Exception BaseException ValueError TypeError KeyError IndexError AttributeError RuntimeError OSError IOError ImportError NameError
StopIteration NotImplementedError ZeroDivisionError AssertionError KeyboardInterrupt FileNotFoundError PermissionError""".split()

PREFIX = r"(?:[rRbBuUfF]{1,2})?"
PY_CHECK = "import sys;compile(open(sys.argv[1],encoding='utf-8').read(),sys.argv[1],'exec')"

MODULE = module(
    version="1.1.0",
    id="python",
    name="Python",
    category="Languages",
    summary="Highlighting, indentation, folding by blocks and an outline for Python; run a file; python3 and pip in Linux.",
    description="Python for Light: strings of every kind (triple-quoted, f-strings, raw), decorators, keywords, builtins and numbers; Enter after a colon indents and return/pass dedent; folding by indentation; the outline of classes and functions; a command to run the current file. The Linux side brings python3, pip and venv - installed from inside the IDE.",
    languages=[dict(
        id="python", name="Python", extensions=["py", "pyw", "pyi", "pyx"], filenames=["SConstruct", "SConscript", "Pipfile"], first_line=r"^#!.*\bpython",
        line_comment="#", brackets=["()", "[]", "{}"], check=["brackets"], indent={"mode": "python", "size": 4}, fold="indent",
        grammar=[
            region(PREFIX + '"""', '"""', "string"),
            region(PREFIX + "'''", "'''", "string"),
            match(r"#.*$", "comment"),
            match(PREFIX + r'"(?:[^"\\\n]|\\.)*"?', "string"),
            match(PREFIX + r"'(?:[^'\\\n]|\\.)*'?", "string"),
            match(r"^[ \t]*(@[A-Za-z_][\w.]*)", captures={1: "decorator"}),
            match(r"\b(class)[ \t]+([A-Za-z_]\w*)", captures={1: "keyword", 2: "type"}),
            match(r"\b(def)[ \t]+([A-Za-z_]\w*)", captures={1: "keyword", 2: "function"}),
            words(KEYWORDS, "keyword"),
            words(["True", "False", "None", "NotImplemented", "Ellipsis", "__name__", "__main__", "__file__"], "constant"),
            words(["self", "cls"], "variable"),
            words(BUILTINS, "builtin"),
            match(r"\b(?:0[xX][0-9a-fA-F_]+|0[bB][01_]+|0[oO][0-7_]+|\d[\d_]*(?:\.[\d_]*)?(?:[eE][+-]?\d+)?[jJ]?|\.\d[\d_]*(?:[eE][+-]?\d+)?[jJ]?)\b", "number"),
            call(),
        ],
        outline=[
            outline(r"^[ \t]*(?:async[ \t]+)?def[ \t]+(\w+)", "function"),
            outline(r"^[ \t]*class[ \t]+(\w+)", "class"),
        ],
    )],
    # the syntax check compiles the file without running it; a SyntaxError names the line:
    #   File "x.py", line 2 ... SyntaxError: invalid syntax   (the first `File "<string>"` of the traceback is the check's own)
    check=[dict(
        language="python", name="Syntax", command=f'python3 -c "{PY_CHECK}" "{{file}}"',
        windows=f'where py >nul 2>nul & if errorlevel 1 (python -c "{PY_CHECK}" "{{file}}") else (py -c "{PY_CHECK}" "{{file}}")',
        pattern=r'(?s)File "(?P<file>[^<"][^"]*)", line (?P<line>\d+)\n.*?(?P<sev>SyntaxError|IndentationError|TabError)[^:\n]*: (?P<msg>[^\n]*)',
    )],
    # on Windows `python` is often only the Microsoft Store stub: the `py` launcher is preferred when there is one
    run=[dict(language="python", name="Run file", command='python3 "{file}"',
              windows='where py >nul 2>nul & if errorlevel 1 (python "{file}") else (py "{file}")')],
    tools=[dict(name="Python", programs=["python3", "pip3"], apt="python3 python3-pip python3-venv")],
    detect=dict(extensions=["py", "pyw"], files=["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"]),
    templates=[
        dict(name="Script", description="main.py", open="main.py",
             files={"main.py": 'def main():\n    print("Hello, Light")\n\n\nif __name__ == "__main__":\n    main()\n'}),
        dict(name="Package with tests", description="a package, a test and requirements", open="app/core.py",
             files={"pyproject.toml": '[project]\nname = "app"\nversion = "0.1.0"\n',
                    "requirements.txt": "pytest\n",
                    "app/__init__.py": '"""A small package."""\n',
                    "app/core.py": "def add(a, b):\n    return a + b\n",
                    "tests/test_core.py": "from app.core import add\n\n\ndef test_add():\n    assert add(2, 3) == 5\n"}),
    ],
)
