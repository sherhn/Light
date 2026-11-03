from common import call, match, module, outline, region, words
from json_lang import GRAMMAR as JSON_GRAMMAR

KEYWORDS = "async await break case catch class const continue debugger default delete do else export extends finally for from function get if import in instanceof let new of return set static super switch throw try typeof var void while with yield as".split()
BUILTINS = "Array ArrayBuffer BigInt Boolean Date Error EvalError Function Intl JSON Map Math Number Object Promise Proxy RangeError Reflect RegExp Set String Symbol SyntaxError TypeError URIError Uint8Array URL URLSearchParams WeakMap WeakSet WebSocket console document window globalThis process Buffer require module exports fetch setTimeout clearTimeout setInterval clearInterval queueMicrotask structuredClone parseInt parseFloat isNaN isFinite encodeURIComponent decodeURIComponent AbortController TextEncoder TextDecoder".split()

GRAMMAR = [
    match(r"//[^\n]*", "comment"), region(r"/\*", r"\*/", "comment"),
    match(r'"(?:[^"\\\n]|\\.)*"?', "string"), match(r"'(?:[^'\\\n]|\\.)*'?", "string"),
    region("`", r"(?:\\.|[^`\\])*`", "string"),
    match(r"(?:^|[=(,:;!&|?{}])[ \t]*(/(?:\\.|\[(?:\\.|[^\]\\\n])*\]|[^/\\\n])+/[dgimsuvy]*)", captures={1: "string"}),
    match(r"\b(class)[ \t]+([A-Za-z_$][\w$]*)", captures={1: "keyword", 2: "type"}),
    match(r"\b(function)[ \t]*\*?[ \t]+([A-Za-z_$][\w$]*)", captures={1: "keyword", 2: "function"}),
    words(KEYWORDS, "keyword"), words(BUILTINS, "builtin"), words(["true", "false", "null", "undefined", "NaN", "Infinity"], "constant"),
    match(r"\b(?:0[xX][\da-fA-F_]+|0[bB][01_]+|0[oO][0-7_]+|\d[\d_]*(?:\.[\d_]*)?(?:[eE][+-]?[\d_]+)?n?)\b", "number"),
    match(r"\b(this)\b", "variable"), call(),
]

# the name of a test or a suite in quotes of any kind (a template with `${...}` in it is not a fixed name)
def _declared(callee):
    return r"^\s*" + callee + r"""(?:\.(?:only|skip|todo|concurrent|fails|failing))*\(\s*(?:'(?P<name>[^'\n]*)'|"(?P<name2>[^"\n]*)"|`(?P<name3>[^`$\n]*)`)"""


TEST_FILES = r"(?:^|/)__tests__/|\.(?:test|spec)\.[cm]?jsx?$"
TESTS = [
    # Vitest and Jest when the package.json of the file names them; node:test when the file imports it
    dict(name="Vitest", languages=["javascript", "jsx"], files=TEST_FILES, project_file="package.json", project_needs='"vitest"',
         test=_declared("(?:it|test)"), suite=_declared("(?:describe|suite)"), separator=" ",
         command='npx vitest run "{file}" -t "{test}"', file_command='npx vitest run "{file}"', env={"NO_COLOR": "1", "FORCE_COLOR": "0"},
         passed=r"(?m)^\s*✓ (?P<name>.+?)(?: \d+ms)?$", failed=r"(?m)^\s*× (?P<name>.+?)(?: \d+ms)?$", result_separator=" > "),
    dict(name="Jest", languages=["javascript", "jsx"], files=TEST_FILES, project_file="package.json", project_needs='"jest"',
         test=_declared("(?:it|test)"), suite=_declared("describe"), separator=" ",
         command='npx jest --runTestsByPath "{file}" -t "{test}"', file_command='npx jest --runTestsByPath "{file}"', env={"NO_COLOR": "1", "FORCE_COLOR": "0"},
         failed=r"(?m)^\s*● (?P<name>.+?)$", result_separator=" › ",
         # Jest measures coverage itself (istanbul)
         coverage="lcov", coverage_command='npx jest --runTestsByPath "{file}" -t "{test}" --coverage --coverageReporters=lcov --coverageReporters=text-summary --coverageDirectory="{report_dir}"',
         coverage_file_command='npx jest --runTestsByPath "{file}" --coverage --coverageReporters=lcov --coverageReporters=text-summary --coverageDirectory="{report_dir}"'),
    dict(name="node:test", languages=["javascript"], content=r"""from\s+['"]node:test['"]|require\(\s*['"]node:test['"]\s*\)""", project_file="package.json",
         test=_declared("(?:it|test)"), suite=_declared("(?:describe|suite)"), separator=" ",
         command='node --test --test-reporter=tap --test-name-pattern="{test}" "{file}"', file_command='node --test --test-reporter=tap "{file}"',
         passed=r"(?m)^\s*ok \d+ - (?P<name>.+?)(?: # .*)?$", failed=r"(?m)^\s*not ok \d+ - (?P<name>.+?)(?: # .*)?$",
         # Node.js measures coverage itself: the TAP goes to the Run window, the LCOV to the report
         coverage="lcov", coverage_command='node --test --experimental-test-coverage --test-reporter=tap --test-reporter-destination=stdout --test-reporter=lcov --test-reporter-destination="{report}" --test-name-pattern="{test}" "{file}"',
         coverage_file_command='node --test --experimental-test-coverage --test-reporter=tap --test-reporter-destination=stdout --test-reporter=lcov --test-reporter-destination="{report}" "{file}"'),
]

# Microsoft's JavaScript debugger, the one VS Code uses, as a debug adapter of its own: downloaded when it is first
# needed, after asking
JS_DEBUG = dict(name="vscode-js-debug", version="1.140.0", url="https://github.com/microsoft/vscode-js-debug/releases/download/v1.140.0/js-debug-dap-v1.140.0.tar.gz",
                sha256="27dab92937ec1ab35821ae955aac867544fe06a1b6307229049f2d789af10968", size=1249709, license="MIT", homepage="https://github.com/microsoft/vscode-js-debug")
DEBUG = [dict(name="Node.js", languages=["javascript", "jsx"], download=JS_DEBUG, adapter_id="pwa-node",
              command=["node", "{adapter}/js-debug/src/dapDebugServer.js", "{port}", "127.0.0.1"],
              launch={"type": "pwa-node", "request": "launch", "name": "Light", "program": "{file}", "cwd": "{dir}", "console": "internalConsole", "skipFiles": ["<node_internals>/**"]})]


MODULE = module(
    id="javascript", name="JavaScript", category="Languages", version="1.5.0",
    summary="JavaScript/JSX, Node.js and npm: highlighting, completion, outline, folding, syntax checks, run, watch and tests.",
    description="JavaScript ES modules and CommonJS, JSX and package.json. Syntax coloring includes comments, strings, template literals, regular expressions, keywords and browser/Node builtins. Brace indentation and folding, function/class outline and completion. Node.js syntax checks, Run, Watch and node:test; package.json exposes npm install, start, dev, build, test and lint. Uses your Node.js/npm on Windows, macOS or Linux (including WSL). No files or dependencies are created until you explicitly run a command.",
    languages=[
        dict(id="javascript", name="JavaScript", extensions=["js", "mjs", "cjs"], first_line=r"^#!.*\bnode", line_comment="//", block_comment=["/*", "*/"], brackets=["()", "[]", "{}"], indent={"mode": "braces", "size": 2}, fold="braces", check=["brackets"], grammar=GRAMMAR, outline=[outline(r"^[ \t]*(?:export[ \t]+(?:default[ \t]+)?)?(?:async[ \t]+)?function[ \t]*\*?[ \t]+([\w$]+)", "function"), outline(r"^[ \t]*(?:export[ \t]+(?:default[ \t]+)?)?class[ \t]+([\w$]+)", "class"), outline(r"^[ \t]*(?:export[ \t]+)?(?:const|let|var)[ \t]+([\w$]+)[ \t]*=[ \t]*(?:async[ \t]+)?(?:\([^\n]*\)|[\w$]+)[ \t]*=>", "function")]),
        dict(id="jsx", name="JSX", extensions=["jsx"], line_comment="//", block_comment=["/*", "*/"], brackets=["()", "[]", "{}"], indent={"mode": "braces", "size": 2}, fold="braces", grammar=GRAMMAR + [match(r"</?([A-Za-z][\w.-]*)", captures={1: "type"}), match(r"\b([\w-]+)=", captures={1: "argument"})], outline=[outline(r"(?:function|class)[ \t]+([\w$]+)", "function")]),
        dict(id="javascript-package", name="npm package", filenames=["package.json"], brackets=["[]", "{}"], indent={"mode": "braces", "size": 2}, fold="braces", check=["brackets"], grammar=JSON_GRAMMAR, outline=[outline(r'^[ \t]*"([^"\n]+)"[ \t]*:', "variable")]),
    ],
    run=[dict(language="javascript", name="Node: Run file", command='node "{file}"'), dict(language="javascript", name="Node: Watch file", command='node --watch "{file}"'), dict(language="javascript", name="Node: Test file", command='node --test "{file}"'),
         # where the time goes: Node.js samples the program and the IDE draws the calls as a flame graph
         dict(language="javascript", name="Node: Profile file", profile="cpuprofile", command='node --cpu-prof --cpu-prof-dir="{report_dir}" "{file}"')]
        + [dict(language="javascript-package", name="npm: " + label, command='npm --prefix "{dir}" ' + command) for label, command in [("Install dependencies", "install"), ("Start", "start"), ("Development server", "run dev"), ("Build", "run build"), ("Test", "test"), ("Lint", "run lint")]],
    check=[dict(language="javascript", name="Node syntax", command='node --check "{file}"', pattern=r"(?m)^(?P<file>[^\n]+):(?P<line>\d+)\r?\n(?:[^\n]*\n)*?SyntaxError: (?P<msg>[^\n]+)")],
    tests=TESTS,
    debug=DEBUG,
    # every script of every package.json of the project, run in its folder (the Tasks window)
    tasks=[dict(name="npm", files=["package.json"], json="scripts", command='npm run "{task}"', skip=["node_modules"])],
    tools=[dict(name="Node.js and npm", programs=["node", "npm"], required=["node", "npm"], apt="nodejs npm")],
    detect=dict(extensions=["js", "mjs", "cjs", "jsx"], files=["package.json", "package-lock.json", "yarn.lock", "pnpm-lock.yaml"]),
)
