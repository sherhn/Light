from common import match, module, outline, region, words

KEY = r'(?:[A-Za-z0-9_-]+|"(?:[^"\\\n]|\\.)*"|\'[^\'\n]*\')(?:[ \t]*\.[ \t]*(?:[A-Za-z0-9_-]+|"(?:[^"\\\n]|\\.)*"|\'[^\'\n]*\'))*'
MODULE = module(
    id="toml", name="TOML", category="Languages",
    summary="TOML configuration: tables, dotted keys, multiline strings, dates and numeric literals.",
    description="Highlight TOML tables and arrays of tables, bare and quoted dotted keys, basic and literal strings (including multiline strings), comments, booleans, dates and numbers. Table outline, brace indentation and array folding. No external tools required.",
    languages=[dict(id="toml", name="TOML", extensions=["toml"], line_comment="#", brackets=["[]", "{}"], indent={"mode": "braces", "size": 2}, fold="braces", check=["brackets"], grammar=[
        match(r'#[^\n]*', "comment"),
        match(r'^[ \t]*\[\[?[^\]\n]+\]\]?', "heading"),
        match(r'^[ \t]*(' + KEY + r')[ \t]*=', captures={1: "variable"}),
        region('"""', r'(?:\\.|[^"\\]|"{1,2}[^"\\])*"""', "string"), region("'''", "'''", "string"),
        match(r'"(?:[^"\\\n]|\\.)*"?|\'[^\'\n]*\'?', "string"),
        match(r'\b\d{4}-\d{2}-\d{2}(?:[Tt ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:[Zz]|[+-]\d{2}:\d{2})?)?|\b\d{2}:\d{2}:\d{2}(?:\.\d+)?', "constant"),
        words(["true", "false", "inf", "nan"], "constant"),
        match(r'[+-]?\b(?:0x[\da-fA-F_]+|0o[0-7_]+|0b[01_]+|\d[\d_]*(?:\.[\d_]+)?(?:[eE][+-]?[\d_]+)?)\b', "number"),
    ], outline=[outline(r'^[ \t]*\[\[?[ \t]*([^\]\n]+?)\]\]?', "module")])],
    detect=dict(extensions=["toml"], files=[]),
)
