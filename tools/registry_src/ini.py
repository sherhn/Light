from common import match, module, outline

MODULE = module(
    id="ini", name="INI", category="Languages",
    summary="INI and EditorConfig: sections, properties, quoted values, comments and section navigation.",
    description="INI and CFG configuration files plus .editorconfig, .gitconfig and .gitmodules. Highlights sections, property names, quoted values and full-line semicolon/hash comments; section and property outline. Values remain literal unless quoted. No external tools required.",
    languages=[dict(id="ini", name="INI", extensions=["ini", "cfg"], filenames=[".editorconfig", ".gitconfig", ".gitmodules"], line_comment=";", indent={"mode": "keep", "size": 2}, grammar=[
        match(r'^[ \t]*[;#].*$', "comment"),
        match(r'^[ \t]*\[[^\n]+\][ \t]*(?:[;#].*)?$', "heading"),
        match(r'^[ \t]*([^\s=:#][^=:\n]*?)[ \t]*[=:]', captures={1: "variable"}),
        match(r'"(?:[^"\\\n]|\\.)*"?|\'[^\'\n]*\'?', "string"),
    ], outline=[outline(r'^[ \t]*\[([^\n]+)\][ \t]*(?:[;#].*)?$', "module"), outline(r'^[ \t]*([^\s=:#;][^=:\n]*?)[ \t]*[=:]', "variable")])],
    detect=dict(extensions=["ini", "cfg"], files=[".editorconfig", ".gitconfig", ".gitmodules"]),
)
