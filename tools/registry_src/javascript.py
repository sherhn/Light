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


MODULE = module(
    id="javascript", name="JavaScript", category="Languages", version="1.0.2",
    summary="JavaScript/JSX, Node.js and npm: highlighting, completion, outline, folding, syntax checks, run, watch and tests.",
    description="JavaScript ES modules and CommonJS, JSX and package.json. Syntax coloring includes comments, strings, template literals, regular expressions, keywords and browser/Node builtins. Brace indentation and folding, function/class outline and completion. Node.js syntax checks, Run, Watch and node:test; package.json exposes npm install, start, dev, build, test and lint. Uses your Node.js/npm on Windows, macOS or Linux (including WSL). No files or dependencies are created until you explicitly run a command.",
    languages=[
        dict(id="javascript", name="JavaScript", extensions=["js", "mjs", "cjs"], first_line=r"^#!.*\bnode", line_comment="//", block_comment=["/*", "*/"], brackets=["()", "[]", "{}"], indent={"mode": "braces", "size": 2}, fold="braces", check=["brackets"], grammar=GRAMMAR, outline=[outline(r"^[ \t]*(?:export[ \t]+(?:default[ \t]+)?)?(?:async[ \t]+)?function[ \t]*\*?[ \t]+([\w$]+)", "function"), outline(r"^[ \t]*(?:export[ \t]+(?:default[ \t]+)?)?class[ \t]+([\w$]+)", "class"), outline(r"^[ \t]*(?:export[ \t]+)?(?:const|let|var)[ \t]+([\w$]+)[ \t]*=[ \t]*(?:async[ \t]+)?(?:\([^\n]*\)|[\w$]+)[ \t]*=>", "function")]),
        dict(id="jsx", name="JSX", extensions=["jsx"], line_comment="//", block_comment=["/*", "*/"], brackets=["()", "[]", "{}"], indent={"mode": "braces", "size": 2}, fold="braces", grammar=GRAMMAR + [match(r"</?([A-Za-z][\w.-]*)", captures={1: "type"}), match(r"\b([\w-]+)=", captures={1: "argument"})], outline=[outline(r"(?:function|class)[ \t]+([\w$]+)", "function")]),
        dict(id="javascript-package", name="npm package", filenames=["package.json"], brackets=["[]", "{}"], indent={"mode": "braces", "size": 2}, fold="braces", check=["brackets"], grammar=JSON_GRAMMAR, outline=[outline(r'^[ \t]*"([^"\n]+)"[ \t]*:', "variable")]),
    ],
    run=[dict(language="javascript", name="Node: Run file", command='node "{file}"'), dict(language="javascript", name="Node: Watch file", command='node --watch "{file}"'), dict(language="javascript", name="Node: Test file", command='node --test "{file}"')]
        + [dict(language="javascript-package", name="npm: " + label, command='npm --prefix "{dir}" ' + command) for label, command in [("Install dependencies", "install"), ("Start", "start"), ("Development server", "run dev"), ("Build", "run build"), ("Test", "test"), ("Lint", "run lint")]],
    check=[dict(language="javascript", name="Node syntax", command='node --check "{file}"', pattern=r"(?m)^(?P<file>[^\n]+):(?P<line>\d+)\r?\n(?:[^\n]*\n)*?SyntaxError: (?P<msg>[^\n]+)")],
    tools=[dict(name="Node.js and npm", programs=["node", "npm"], required=["node", "npm"], apt="nodejs npm")],
    detect=dict(extensions=["js", "mjs", "cjs", "jsx"], files=["package.json", "package-lock.json", "yarn.lock", "pnpm-lock.yaml"]),
)
