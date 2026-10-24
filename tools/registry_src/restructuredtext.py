from common import match, module, outline

UNDERLINE = r"[=\-~^\"'#*+:._`]"

MODULE = module(
    version="1.1.0",
    id="restructuredtext",
    name="reStructuredText",
    category="Documents",
    summary="Headings by underline, directives, roles, links and lists for .rst; folding and an outline by sections.",
    description="reStructuredText for Light (the markup of Sphinx and Python documentation): headings found by their underlines - the first style of underline is the top level - directives, hyperlink targets, comments, roles, literals, emphasis and links are coloured; Enter continues a list; the outline and folding follow the sections. A rendered preview shows the document next to the code or instead of it (Editor, Editor and Preview, Preview).",
    languages=[dict(
        id="rst", name="reStructuredText", extensions=["rst", "rest"], filenames=[],
        line_comment="..", brackets=[], indent={"mode": "markup", "size": 3}, fold="headings", preview="rst",
        grammar=[
            # a title and its underline (the title is the first line)
            match(r"^[^\s.\n][^\n]*\n" + UNDERLINE + r"{3,}[ \t]*$", "heading"),
            match(r"^(\.\.)[ \t]+([A-Za-z][\w+.:-]*)(::)", captures={1: "preproc", 2: "keyword", 3: "preproc"}),
            match(r"^\.\.[ \t]+_[^\n:]+:.*$", "link"),
            match(r"^\.\.[ \t]+\[[^\]\n]+\].*$", "link"),
            match(r"^\.\.[ \t]+\|[^|\n]+\|.*$", "link"),
            match(r"^\.\.(?:[ \t].*)?$", "comment"),
            match(r"::[ \t]*$", "prefix"),
            match(r"^[ \t]*:[A-Za-z][\w -]*:(?:[ \t]|$)", "builtin"),
            match(r"(:[A-Za-z][\w:+.-]*:)(`[^`\n]+`)", captures={1: "keyword", 2: "link"}),
            match(r"``[^`\n]+``", "code"),
            match(r"`[^`\n]+`__?", "link"),
            match(r"`[^`\n]+`", "emphasis"),
            match(r"\*\*[^*\n]+\*\*", "strong"),
            match(r"\*[^*\s][^*\n]*\*", "emphasis"),
            match(r"\[(?:\d+|#\w*|\*)\]_", "link"),
            match(r"\|[^|\s][^|\n]*\|_{0,2}", "link"),
            match(r"https?://[^\s)>\]]+", "link"),
            match(r"^[ \t]*(?:[-*+•]|\d+\.|#\.|[a-zA-Z]\.|\(\d+\))[ \t]+", "prefix"),
            match(r"^[ \t]*\+[-=+]+\+[ \t]*$|^[ \t]*=+(?:[ \t]+=+)+[ \t]*$", "prefix"),
            match(r"^\|[ \t]", "prefix"),
        ],
        outline=[outline(r"^([^\s.\n][^\n]*)\n(" + UNDERLINE + r"{3,})[ \t]*$", "heading", name=1, level=2, level_mode="first_seen")],
    )],
    run=[],
    tools=[],
    detect=dict(extensions=["rst"], files=["conf.py"]),
    templates=[
        dict(name="Document", description="index.rst", open="index.rst",
             files={"index.rst": "Title\n=====\n\nIntroduction\n------------\n\nSome *emphasis*, some **strong** text and ``literal`` code.\n\n.. note::\n\n   A directive.\n\n- first\n- second\n"}),
    ],
)
