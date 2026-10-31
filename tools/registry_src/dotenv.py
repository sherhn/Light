from common import match, module, outline

FILES = [".env", ".env.*", "*.env", "*.env.*"]
MODULE = module(
    id="dotenv", name="Environment", category="Languages",
    summary="Environment files: .env variants, assignments, export, quoted values and variable references.",
    description="Dedicated dotenv editing for .env, .env.local, .env.example and named environment files. Highlights assignment names, export, quoted values, comments and unquoted variable references, with a variable outline. Files are edited as text; no environment variables are loaded or commands executed.",
    languages=[dict(id="dotenv", name="Environment", extensions=["env"], filenames=FILES, line_comment="#", indent={"mode": "keep", "size": 2}, grammar=[
        match(r'^[ \t]*#.*$', "comment"),
        match(r'^[ \t]*(?:(export)[ \t]+)?([A-Za-z_][A-Za-z0-9_]*)[ \t]*=', captures={1: "keyword", 2: "variable"}),
        match(r'"(?:[^"\\]|\\.)*"?|\'[^\']*\'?', "string"),
        match(r'#[^\n]*', "comment"),
        match(r'\$\{[^}\n]+\}|\$[A-Za-z_][A-Za-z0-9_]*', "variable"),
    ], outline=[outline(r'^[ \t]*(?:export[ \t]+)?([A-Za-z_][A-Za-z0-9_]*)[ \t]*=', "variable")])],
    detect=dict(extensions=["env"], files=FILES),
)
