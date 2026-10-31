from common import match, module, outline, region, words

KEY = r'"(?:[^"\\\n]|\\.)*"'
GRAMMAR = [
    match("(" + KEY + r')[ \t]*:', captures={1: "argument"}),
    match(r'"(?:[^"\\\n]|\\.)*"?', "string"),
    words(["true", "false", "null"], "constant"),
    match(r'-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b', "number"),
]

def language(id, name, extensions, comments=False):
    return dict(id=id, name=name, extensions=extensions,
                brackets=["[]", "{}"], indent={"mode": "braces", "size": 2},
                fold="braces", check=["brackets"],
                **({"line_comment": "//", "block_comment": ["/*", "*/"]} if comments else {}),
                grammar=([match(r'//[^\n]*', "comment"), region(r'/\*', r'\*/', "comment")] if comments else []) + GRAMMAR,
                outline=[outline(r'^[ \t]*"((?:[^"\\\n]|\\.)*)"[ \t]*:', "variable")])

MODULE = module(
    id="json", name="JSON", category="Languages",
    summary="JSON, JSON Lines and JSON with comments: highlighting, key outline, indentation and folding.",
    description="Standalone JSON support without Node.js. Keys, strings, numbers and constants, bracket checks, key navigation and brace folding. JSONC additionally supports line and block comments; strict JSON keeps comments disabled. Includes JSON Lines (.jsonl and .ndjson).",
    languages=[language("json", "JSON", ["json", "jsonl", "ndjson"]), language("jsonc", "JSON with comments", ["jsonc"], comments=True)],
    detect=dict(extensions=["json", "jsonl", "ndjson", "jsonc"], files=[]),
)
