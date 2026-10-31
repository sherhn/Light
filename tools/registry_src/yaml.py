from common import match, module, outline, words

KEY = r'(?:[\w.-]+|"(?:[^"\\\n]|\\.)*"|\'[^\'\n]*\')'
MODULE = module(
    id="yaml", name="YAML", category="Languages",
    summary="YAML mappings and sequences: keys, anchors, aliases, block scalars, indentation and folding.",
    description="YAML and YML configuration with mapping-key outline, comments, quoted strings, booleans, nulls, numbers, anchors, aliases, tags, block-scalar indicators and document markers. Two-space indentation and indentation folding. Docker Compose retains its dedicated language when installed. No external tools required.",
    languages=[dict(id="yaml", name="YAML", extensions=["yaml", "yml"], line_comment="#", brackets=["[]", "{}"], indent={"mode": "python", "size": 2}, fold="indent", grammar=[
        match(r'(?:^|[ \t])(#.*)$', captures={1: "comment"}),
        match(r'^[ \t]*(?:-[ \t]+)?(' + KEY + r')[ \t]*:(?:[ \t]|$)', captures={1: "variable"}),
        match(r'[|>][1-9+-]{0,2}[ \t]*(?:#[^\n]*)?$', "constant"),
        match(r'"(?:[^"\\\n]|\\.)*"?|\'(?:[^\'\n]|\'\')*\'?', "string"),
        match(r'[&*][A-Za-z_][\w.-]*', "variable"), match(r'![^\s,\[\]{}]+', "type"),
        match(r'^(?:---|\.\.\.)[ \t]*$', "preproc"),
        words(["true", "false", "null", "yes", "no", "on", "off"], "constant", ignore_case=True),
        match(r'~', "constant"), match(r'[+-]?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b', "number"),
    ], outline=[outline(r'^[ \t]*(?:-[ \t]+)?(' + KEY + r')[ \t]*:(?:[ \t]|$)', "variable")])],
    detect=dict(extensions=["yaml", "yml"], files=[]),
)
