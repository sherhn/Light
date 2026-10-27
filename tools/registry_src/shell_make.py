from common import match, module, outline, region, words

SH_KEYWORDS = """if then else elif fi for while until do done case esac in function select time coproc""".split()
SH_BUILTINS = """echo printf cd pwd test read return break continue exit local export readonly declare typeset unset shift source alias unalias eval exec trap
set wait kill jobs fg bg let getopts type command builtin umask ulimit true false""".split()
SH_COMMANDS = """ls cat grep sed awk find mkdir rm cp mv chmod chown ln tar curl wget git make sudo apt dpkg ssh scp rsync tee head tail sort uniq wc xargs
cut tr diff patch gzip gunzip zip unzip which whoami uname date sleep touch basename dirname python3 pip3 cc gcc nasm""".split()

MAKE_DIRECTIVES = r"^[ \t]*(?:-?include|sinclude|ifeq|ifneq|ifdef|ifndef|else|endif|define|endef|export|unexport|override|private|vpath|undefine)\b"

MODULE = module(
    version="1.1.1",
    id="shell-make",
    name="Shell and Make",
    category="Languages",
    summary="Shell scripts, makefiles and the usual configuration files: highlighting, comments and an outline of functions and targets.",
    description="Shell scripts (sh, bash, zsh), makefiles and configuration files (YAML, TOML, INI, CMake, Dockerfile, .gitignore) for Light: comments, strings, variables, keywords and builtins, make targets and recipes, the outline of functions and targets, and Comment Line with #. Make targets also appear as run configurations.",
    languages=[
        dict(
            id="shell", name="Shell", extensions=["sh", "bash", "zsh", "ksh", "bashrc", "profile", "bash_profile", "zshrc"], filenames=[".bashrc", ".profile", ".bash_profile", ".zshrc"],
            first_line=r"^#!.*\b(?:ba|z|k|da)?sh\b",
            line_comment="#", brackets=["()", "[]", "{}"], indent={"mode": "keep", "size": 2}, fold="none",
            grammar=[
                match(r"^#!.*$", "preproc"),
                match(r"(?:^|[ \t;|&(])(#.*)$", captures={1: "comment"}),
                match(r'"(?:[^"\\]|\\.)*"?', "string"),
                match(r"'[^']*'?", "string"),
                match(r"\$\{[^}\n]*\}|\$[A-Za-z_]\w*|\$[@*#?$!0-9-]", "variable"),
                match(r"^[ \t]*(?:function[ \t]+)?([A-Za-z_][\w-]*)[ \t]*\(\)", captures={1: "function"}, consume=1),
                words(SH_KEYWORDS, "keyword"),
                words(SH_BUILTINS, "builtin"),
                words(SH_COMMANDS, "type"),
                match(r"(?:apt-get|qemu-system-[A-Za-z0-9_]+)", "type"),
                match(r"\b\d+\b", "number"),
            ],
            outline=[outline(r"^[ \t]*(?:function[ \t]+)?([A-Za-z_][\w-]*)[ \t]*\(\)", "function")],
        ),
        dict(
            id="make", name="Makefile", extensions=["mk", "mak"], filenames=["Makefile", "makefile", "GNUmakefile"],
            line_comment="#", brackets=["()", "{}"], indent={"mode": "keep", "size": 4}, fold="none",
            grammar=[
                match(r"#.*$", "comment"),
                match(r"^\t[@+-]+", "prefix"),
                match(MAKE_DIRECTIVES, "preproc"),
                match(r"^[ \t]*(\.[A-Z_]+)[ \t]*:", captures={1: "preproc"}, consume=1),
                match(r"^[ \t]*([A-Za-z_][\w.]*)[ \t]*(?:[:?+!]?=)", captures={1: "variable"}, consume=1),
                match(r"^([\w./%$()*+@-]+(?:[ \t]+[\w./%$()*+@-]+)*)[ \t]*::?(?:[ \t]|$)", captures={1: "function"}),
                match(r"\$\((?:[A-Za-z_][\w.]*)\)|\$\{[A-Za-z_][\w.]*\}|\$[@<^?*%+|]|\$\$", "variable"),
                match(r"\$\((?:subst|patsubst|strip|findstring|filter|filter-out|sort|word|words|wordlist|firstword|lastword|wildcard|shell|foreach|call|eval|if|or|and|origin|dir|notdir|basename|suffix|addprefix|addsuffix|join|abspath|realpath|error|warning|info)\b", "builtin"),
                match(r'"(?:[^"\\\n]|\\.)*"?|\'[^\'\n]*\'?', "string"),
            ],
            outline=[outline(r"^([A-Za-z0-9_/-][A-Za-z0-9_./-]*)[ \t]*:(?:[ \t]|$)", "function")],
        ),
        dict(
            id="config", name="Config", extensions=["yml", "yaml", "toml", "ini", "cfg", "conf", "cmake", "gitignore", "dockerfile", "env", "properties"],
            filenames=["Dockerfile", ".gitignore", ".gitattributes", ".editorconfig", "CMakeLists.txt"],
            line_comment="#", brackets=["()", "[]", "{}"], indent={"mode": "keep", "size": 2}, fold="indent",
            grammar=[
                match(r"(?:^|[ \t])(#.*)$", captures={1: "comment"}),
                match(r"^[ \t]*;.*$", "comment"),
                match(r'"(?:[^"\\\n]|\\.)*"?', "string"),
                match(r"'[^'\n]*'?", "string"),
                match(r"^[ \t]*\[[^\]\n]+\][ \t]*$", "heading"),
                match(r"^[ \t]*-?[ \t]*([A-Za-z_][\w.-]*)[ \t]*[:=]", captures={1: "variable"}, consume=1),
                words(["true", "false", "null", "yes", "no", "on", "off", "None"], "constant", ignore_case=True),
                match(r"\b-?\d+(?:\.\d+)?\b", "number"),
            ],
            outline=[outline(r"^[ \t]*\[([^\]\n]+)\][ \t]*$", "module")],
        ),
    ],
    run=[dict(language="shell", name="Run script", command='sh "{file}"')],
    # `sh -n` reads the script without running it: "x.sh: line 4: syntax error: unexpected end of file"
    check=[dict(language="shell", name="Syntax", command='sh -n "{file}"', pattern=r"^[^:\n]+: (?:line )?(?P<line>\d+): (?P<msg>[Ss]yntax error.*)$")],
    tools=[],
    detect=dict(extensions=["sh", "mk"], files=["Makefile", "makefile", "GNUmakefile", "Dockerfile"]),
    templates=[
        dict(name="Script", description="run.sh", open="run.sh", files={"run.sh": '#!/bin/sh\nset -e\n\necho "Hello, Light"\n'}),
    ],
)
