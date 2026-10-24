from common import match, module, outline, region, scoped, words

PROPERTIES = """accent-color align-content align-items align-self animation animation-delay animation-direction
animation-duration animation-fill-mode animation-iteration-count animation-name animation-play-state animation-timing-function
appearance aspect-ratio backdrop-filter backface-visibility background background-attachment background-blend-mode
background-clip background-color background-image background-origin background-position background-repeat background-size
border border-bottom border-bottom-color border-bottom-left-radius border-bottom-right-radius border-bottom-style
border-bottom-width border-collapse border-color border-image border-left border-radius border-right border-spacing
border-style border-top border-top-left-radius border-top-right-radius border-width bottom box-shadow box-sizing break-after
break-before break-inside caption-side caret-color clear clip clip-path color color-scheme column-count column-gap column-rule
column-width columns contain container container-name container-type content counter-increment counter-reset cursor direction
display empty-cells filter flex flex-basis flex-direction flex-flow flex-grow flex-shrink flex-wrap float font font-family
font-feature-settings font-size font-stretch font-style font-variant font-weight gap grid grid-area grid-auto-columns
grid-auto-flow grid-auto-rows grid-column grid-column-end grid-column-start grid-row grid-row-end grid-row-start
grid-template grid-template-areas grid-template-columns grid-template-rows height hyphens image-rendering inline-size inset
inset-block inset-inline isolation justify-content justify-items justify-self left letter-spacing line-height list-style
list-style-image list-style-position list-style-type margin margin-block margin-bottom margin-inline margin-left margin-right
margin-top max-height max-inline-size max-width min-height min-inline-size min-width mix-blend-mode object-fit object-position
opacity order outline outline-color outline-offset outline-style outline-width overflow overflow-wrap overflow-x overflow-y
overscroll-behavior padding padding-block padding-bottom padding-inline padding-left padding-right padding-top perspective
place-content place-items place-self pointer-events position resize right rotate row-gap scale scroll-behavior scroll-margin
scroll-padding scroll-snap-align scroll-snap-type tab-size table-layout text-align text-decoration text-decoration-color
text-decoration-line text-decoration-style text-indent text-overflow text-shadow text-transform text-underline-offset
text-wrap top touch-action transform transform-origin transform-style transition transition-delay transition-duration
transition-property transition-timing-function translate unicode-bidi user-select vertical-align visibility white-space
width will-change word-break word-spacing word-wrap writing-mode z-index""".split()

VALUES = """absolute auto baseline black block blue bold border-box both bottom center circle collapse column contain contents
cover currentColor dashed default dense dotted ease end fixed flex grid hidden inherit initial inline italic justify left
linear medium none normal nowrap pointer red relative repeat revert right round row scroll solid space start static sticky
stretch table thin top transparent underline unset visible white wrap yellow""".split()

STRING = r'''"(?:[^"\\]|\\[\s\S])*"?|'(?:[^'\\]|\\[\s\S])*'?'''
URL = r'''(?i:\burl)\(\s*(?:[^)"'\\]|\\.|"[^"]*"|'[^']*')*(?:\)|$)'''
# A declaration is scoped so #abc remains an ID selector outside a property value.
# Comments and strings may contain semicolons/braces without ending the value.
PROPERTY = r"(?:--[\w-]+|(?:-[a-z]+-)?(?:" + "|".join(sorted(PROPERTIES, key=len, reverse=True)) + r"))"
DECLARATION = rf'''(?i:{PROPERTY})\s*:(?:{URL}|[^;{{}}"'/]|/(?:[^*]|$)|/\*[\s\S]*?(?:\*/|$)|{STRING})*'''
DECLARATION_GRAMMAR = [
    region(r"/\*", r"\*/", "comment"),
    match(r"\A(--[\w-]+)\s*:", captures={1: "variable"}, consume=1),
    match(r"\A([\w-]+)\s*:", captures={1: "argument"}, consume=1),
    match(STRING, "string"),
    match(URL, "string"),
    match(r"#[0-9A-Fa-f]{3,8}\b", "number"),
    match(r"--[\w-]+", "variable"),
    match(r"(?i:!\s*important)\b", "keyword"),
    match(r"[-+]?(?:\d*\.\d+|\d+\.?\d*)(?:[eE][+-]?\d+)?(?:%|[A-Za-z]+)?", "number"),
    match(r"([A-Za-z_][\w-]*)\(", captures={1: "builtin"}, consume=1),
    words(VALUES, "constant", ignore_case=True),
    words(PROPERTIES, "argument", ignore_case=True),
]

MODULE = module(
    id="css",
    name="CSS",
    category="Languages",
    summary="Selectors, properties, values, colours, variables and at-rules; brace indentation, folding and a stylesheet template.",
    description="CSS for Light: class and ID selectors, pseudo-classes and pseudo-elements, attribute selectors, common properties (including vendor prefixes), custom properties, strings, URLs, colours, dimensions, functions and at-rules. Comment Line uses CSS block comments; braces indent, match and fold. The outline lists rule selectors and block at-rules. Start with a stylesheet template or use the stylesheet linked by the HTML module's web page template.",
    languages=[dict(
        id="css", name="CSS", extensions=["css"], filenames=[],
        block_comment=["/*", "*/"], brackets=["()", "[]", "{}"], identifier_chars="-", indent={"mode": "braces", "size": 2}, fold="braces", check=["brackets"],
        grammar=[
            region(r"/\*", r"\*/", "comment"),
            match(STRING, "string"),
            match(URL, "string"),
            scoped(DECLARATION, DECLARATION_GRAMMAR, fallback="plain"),
            match(r"@[A-Za-z_-][\w-]*", "preproc"),
            match(r"[.#][-\w]+", "type"),
            match(r"::?[A-Za-z_-][\w-]*", "builtin"),
            match(r"[A-Za-z_-][\w-]*", "plain"),
            match(r"[-+]?(?:\d*\.\d+|\d+\.?\d*)(?:%|[A-Za-z]+)?", "number"),
        ],
        outline=[
            outline(r"^[ \t]*(@[\w-]+[^;{}]*?)\s*\{", "module"),
            outline(r"^[ \t]*([^\s@{};/][^{};]*?)\s*\{", "class"),
        ],
    )],
    run=[],
    tools=[],
    detect=dict(extensions=["css"], files=[]),
    templates=[dict(
        name="Stylesheet", description="Base styles, a custom property and a responsive layout", open="styles.css",
        files={"styles.css": ':root {\n  --accent: #2563eb;\n  font-family: system-ui, sans-serif;\n  color-scheme: light dark;\n}\n\n* {\n  box-sizing: border-box;\n}\n\nbody {\n  margin: 0;\n  line-height: 1.5;\n}\n\na {\n  color: var(--accent);\n}\n\n.page {\n  max-width: 60rem;\n  margin: 0 auto;\n  padding: 2rem;\n}\n\n@media (max-width: 40rem) {\n  .page {\n    padding: 1rem;\n  }\n}\n'},
    )],
)
