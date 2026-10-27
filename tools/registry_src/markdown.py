from common import match, module, outline, region

MODULE = module(
    version="1.1.1",
    id="markdown",
    name="Markdown",
    category="Documents",
    summary="Headings, emphasis, code, links and lists; Enter continues a list; folding and an outline by headings.",
    description="Markdown for Light: headings (# and underlined), bold, italic, inline code and fenced code blocks, links and images, block quotes, lists and tables are coloured; Enter at the end of a list item or a quotation continues it and Enter on an empty item ends it; the outline and folding follow the headings; Comment Line wraps a line in an HTML comment. A rendered preview shows the document next to the code or instead of it (Editor, Editor and Preview, Preview).",
    languages=[dict(
        id="markdown", name="Markdown", extensions=["md", "markdown", "mdown", "mkd", "mdx"], filenames=["README", "CHANGELOG", "LICENSE"],
        block_comment=["<!--", "-->"], brackets=[], indent={"mode": "markup", "size": 2}, fold="headings", preview="markdown",
        grammar=[
            region(r"^[ \t]*(?:```|~~~)", r"^[ \t]*(?:```|~~~)[ \t]*$", "code"),
            region(r"<!--", r"-->", "comment"),
            match(r"^#{1,6}[ \t].*$", "heading"),
            match(r"^[^\s#>\-*+`~|\d<\[!][^\n]*\n(?:=+|-+)[ \t]*$", "heading"),
            match(r"^[ \t]*(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,})$", "prefix"),
            match(r"^[ \t]*\|?[ \t]*:?-{3,}:?[ \t]*(?:\|[ \t]*:?-{3,}:?[ \t]*)*\|?[ \t]*$", "prefix"),
            match(r"^[ \t]*>.*$", "quote"),
            match(r"^[ \t]*(?:[-*+]|\d+[.)])[ \t]+(?:\[[ xX]\][ \t])?", "prefix"),
            match(r"``[^\n]+?``|`[^`\n]+`", "code"),
            match(r"!?\[[^\]\n]*\]\([^)\n]*\)", "link"),
            match(r"!?\[[^\]\n]+\]\[[^\]\n]*\]", "link"),
            match(r"^[ \t]*\[[^\]\n]+\]:[ \t]*\S+", "link"),
            match(r"<https?://[^>\s]+>|https?://[^\s)>\]]+", "link"),
            match(r"\*\*[^*\n]+\*\*|__[^_\n]+__", "strong"),
            match(r"\*[^*\s][^*\n]*\*|\b_[^_\s][^_\n]*_\b", "emphasis"),
            match(r"~~[^~\n]+~~", "comment"),
            match(r"</?[A-Za-z][\w:-]*(?:[ \t][^>\n]*)?/?>", "preproc"),
        ],
        outline=[
            outline(r"^(#{1,6})[ \t]+(.+?)[ \t]*#*[ \t]*$", "heading", name=2, level=1),
            outline(r"^([^\s#>\-*+`~|\d<\[!][^\n]*)\n(=+|-+)[ \t]*$", "heading", name=1, level=2, level_mode="setext"),
        ],
    )],
    run=[],
    tools=[],
    detect=dict(extensions=["md", "markdown"], files=[]),
    templates=[
        dict(name="README", description="README.md", open="README.md",
             files={"README.md": "# Project\n\nWhat it is, in a sentence.\n\n## Getting started\n\n1. Clone it\n2. Build it\n3. Run it\n\n## Notes\n\n- one\n- two\n"}),
    ],
)
