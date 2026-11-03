from common import match, module, outline, words

METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT"]
# the IDE sends the requests (its HTTP engine is switched on by `builtin`); the module brings the language
MODULE = module(
    id="http", name="HTTP Client", category="Tools", version="1.0.0", builtin="http",
    summary="HTTP requests in .http files: send them from the editor, with variables, environments and readable responses.",
    description="Write requests the way the HTTP clients of JetBrains IDEs and VS Code's REST Client do: a method, a URL, headers and a body, requests separated by ###. A button next to each request sends it with curl and shows the status, headers and the body (JSON indented) in the Run window. {{variables}} come from @name = value lines, from http-client.env.json and http-client.private.env.json environments, or are made up on the spot ({{$uuid}}, {{$timestamp}}, {{$isoTimestamp}}, {{$randomInt}}). < file sends a file as the body; # @no-redirect keeps redirects from being followed.",
    languages=[dict(id="http", name="HTTP requests", extensions=["http"], line_comment="#", indent={"mode": "keep", "size": 2}, grammar=[
        match(r"^###.*$", "heading"),
        match(r"^[ \t]*(?:#|//)[ \t]*(@(?:name|no-redirect))\b.*$", captures={1: "meta"}),
        match(r"^[ \t]*(?:#|//).*$", "comment"),
        match(r"^[ \t]*(@[A-Za-z_][\w.-]*)[ \t]*=", captures={1: "variable"}),
        match(r"^(" + "|".join(METHODS) + r")[ \t]+(\S+)(?:[ \t]+(HTTP/[\d.]+))?", captures={1: "keyword", 2: "string", 3: "constant"}),
        match(r"^(https?://\S+)", captures={1: "string"}),
        match(r"^([A-Za-z][\w-]*)(:)[ \t]", captures={1: "argument"}),
        match(r"\{\{[^}\n]*\}\}", "variable"),
        match(r"^<[ \t]+\S.*$", "preproc"),
        match(r'"(?:[^"\\n]|\.)*"?', "string"),
        words(["true", "false", "null"], "constant"),
        match(r"-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b", "number"),
    ], outline=[outline(r"^###[ \t]*(\S[^\n]*)$", "function"), outline(r"^((?:" + "|".join(METHODS) + r")[ \t]+\S+)", "function")])],
    detect=dict(extensions=["http"], files=["http-client.env.json"]),
    templates=[
        dict(name="HTTP requests", description="requests.http with a GET and a POST", open="requests.http", files={
            "requests.http": "@host = https://httpbin.org\n\n### Who am I\nGET {{host}}/get\nAccept: application/json\n\n### Send JSON\nPOST {{host}}/post\nContent-Type: application/json\n\n{\n  \"id\": \"{{$uuid}}\",\n  \"at\": \"{{$isoTimestamp}}\"\n}\n",
        }),
    ],
)
