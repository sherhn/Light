from common import match, module, outline, region, scoped, words

TAGS = """a abbr address area article aside audio b base bdi bdo blockquote body br button canvas caption cite code col
colgroup data datalist dd del details dfn dialog div dl dt em embed fieldset figcaption figure footer form h1 h2 h3 h4
h5 h6 head header hgroup hr html i iframe img input ins kbd label legend li link main map mark menu meta meter nav
noscript object ol optgroup option output p picture pre progress q rp rt ruby s samp script search section select slot
small source span strong style sub summary sup table tbody td template textarea tfoot th thead time title tr track u
ul var video wbr""".split()

ATTRIBUTES = """accept action alt async autocomplete autofocus autoplay charset checked class cols colspan content
contenteditable controls defer disabled download enctype for height hidden href id integrity lang loading max maxlength
method min multiple name open pattern placeholder readonly rel required role rows rowspan selected src srcset step style
tabindex target title type value width aria-label aria-labelledby aria-describedby aria-hidden aria-expanded aria-controls
aria-current aria-live data-testid""".split()

# Quotes can contain angle brackets and newlines. Keep incomplete tags coloured while they are being edited.
TAG = r'''</?[A-Za-z][\w:-]*(?:[^"'<>]|"[^"]*(?:"|$)|'[^']*(?:'|$))*(?:>|$)'''
TAG_GRAMMAR = [
    match(r'^(</?)([A-Za-z][\w:-]*)', captures={1: "preproc", 2: "keyword"}),
    match(r'"[^"]*"?', "string"),
    match(r"'[^']*'?", "string"),
    match(r'([A-Za-z_:][\w:.-]*)\s*=', captures={1: "argument"}, consume=1),
    match(r'=\s*([^\s"\'=<>`]+)', captures={1: "string"}),
    words(ATTRIBUTES, "argument", ignore_case=True),
    # These words also supply completion, but only colour inside a tag.
    words(TAGS, "keyword", ignore_case=True),
    match(r'[A-Za-z_:][\w:.-]*', "argument"),
    match(r'/?>|=', "preproc"),
]

MODULE = module(
    id="html",
    name="HTML",
    category="Languages",
    summary="Tags, attributes, entities and comments; tag indentation and folding, an element outline and a web page template.",
    description="HTML for Light: tags (including custom elements), quoted and unquoted attributes, character references, declarations and comments are coloured without treating page text as code. Tag and attribute names are available in completion; the outline lists opening elements. Comment Line wraps lines in HTML comments; opening and closing tags control indentation and folding, with void and self-closing elements handled separately. Script, style, textarea and title contents stay separate from HTML tags. A new project can start with an HTML page and a linked stylesheet.",
    languages=[dict(
        id="html", name="HTML", extensions=["html", "htm", "xhtml"], filenames=[],
        first_line=r"(?i)^\s*(?:<!doctype\s+html\b|<html(?:\s|>))",
        block_comment=["<!--", "-->"], brackets=[], identifier_chars="-:",
        void_tags="area base br col embed hr img input link meta param source track wbr".split(),
        indent={"mode": "tags", "size": 2}, fold="tags",
        grammar=[
            region(r"<!--", r"-->", "comment"),
            region(r"<!\[CDATA\[", r"\]\]>", "code"),
            match(r'''(?i:<!doctype\b)(?:[^"'>]|"[^"]*"|'[^']*')*(?:>|$)''', "preproc"),
            region(r"<\?", r"\?>", "preproc"),
            # Raw text / RCDATA elements can contain tag-shaped strings. Their bodies must not enter the outline.
            *[scoped(
                rf'''(?is:<{tag}(?:\s+(?:[^"'<>]|"[^"]*"|'[^']*')*)?>.*?(?:</{tag}\s*>|\z))''',
                [scoped(rf'''(?is:\A<{tag}\b(?:[^"'<>]|"[^"]*"|'[^']*')*>|</{tag}\s*>\z)''', TAG_GRAMMAR),
                 match(rf'(?is:(.+?))(?:</{tag}\s*>|\z)', captures={1: "code"}, consume=1)],
            ) for tag in ["script", "style", "textarea", "title"]],
            scoped(TAG, TAG_GRAMMAR),
            match(r"&(?:#[0-9]+|#[xX][0-9A-Fa-f]+|[A-Za-z][A-Za-z0-9]+);", "constant"),
        ],
        outline=[outline(r"<([A-Za-z][\w:-]*)(?:\s|/?>)", "type")],
    )],
    run=[],
    tools=[],
    detect=dict(extensions=["html", "htm", "xhtml"], files=[]),
    templates=[dict(
        name="Web page", description="An HTML page with a linked stylesheet", open="index.html",
        files={
            "index.html": '<!doctype html>\n<html lang="en">\n  <head>\n    <meta charset="utf-8">\n    <meta name="viewport" content="width=device-width, initial-scale=1">\n    <title>My website</title>\n    <link rel="stylesheet" href="styles.css">\n  </head>\n  <body>\n    <main class="page">\n      <h1>Hello, Light</h1>\n      <p>Your first web page.</p>\n    </main>\n  </body>\n</html>\n',
            "styles.css": ':root {\n  color-scheme: light dark;\n  font-family: system-ui, sans-serif;\n}\n\nbody {\n  margin: 0;\n}\n\n.page {\n  max-width: 60rem;\n  margin: 0 auto;\n  padding: 2rem;\n}\n',
        },
    )],
)
