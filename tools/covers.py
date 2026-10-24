"""The covers of the modules, drawn in the look of Light: a black ground with green and blue light, liquid glass on top -
a frosted code card, a glass tile with the mark of the language, drops of glass - and the name of the module.

Used by tools/build_registry.py. Each cover is 640 x 360 (drawn at twice that and scaled down).
"""
import math
import pathlib
import random
import sys

import numpy as np
from PIL import Image

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import glass as g  # noqa: E402

W, H = 1280, 720
OUT_W, OUT_H = 640, 360

# colours of the code, as in the editor (apps/light/src/theme.rs, dark)
C = {
    "p": (207, 216, 212),    # plain
    "k": (107, 166, 255),    # keyword
    "s": (126, 212, 146),    # string
    "n": (90, 209, 230),     # number
    "c": (98, 115, 108),     # comment
    "f": (245, 214, 122),    # function / definition
    "t": (79, 216, 200),     # type
    "v": (183, 140, 255),    # variable / register
    "h": (53, 208, 127),     # heading
    "l": (92, 160, 255),     # link
    "m": (255, 158, 122),    # constant / marker
    "e": (111, 224, 200),    # code
    "x": (199, 178, 90),     # preprocessor
}

# per module: the glyph on the tile, its font, the code of the card and where the light comes from
SCENES = {
    "lisp": dict(
        glyph="λ", mono=True, size=250, label="LANGUAGE",
        code=[[("(", "p"), ("defun ", "k"), ("greet ", "f"), ("(name)", "p")],
              [("  ", "p"), ('"Say hello."', "s")],
              [("  (", "p"), ("format ", "k"), ("t ", "m"), ('"Hello, ~a~%"', "s"), (" name))", "p")],
              [("", "p")],
              [("(", "p"), ("greet ", "f"), ('"Light"', "s"), (")", "p")]],
        orbs=[((200, 120), 700, g.GREEN, 0.80), ((1120, 640), 640, g.BLUE, 0.78)],
    ),
    "c-cpp": dict(
        glyph="C++", mono=False, size=150, label="LANGUAGE",
        code=[[("#include ", "x"), ("<stdio.h>", "s")],
              [("", "p")],
              [("int ", "t"), ("main", "f"), ("(", "p"), ("void", "t"), (") {", "p")],
              [("    ", "p"), ("printf", "f"), ("(", "p"), ('"Hello, Light\\n"', "s"), (");", "p")],
              [("    ", "p"), ("return ", "k"), ("0", "n"), (";", "p")],
              [("}", "p")]],
        orbs=[((1090, 110), 720, g.BLUE, 0.85), ((150, 650), 620, g.GREEN, 0.72), ((640, 360), 380, g.TEAL, 0.14)],
    ),
    "python": dict(
        glyph="Py", mono=False, size=170, label="LANGUAGE",
        code=[[("def ", "k"), ("greet", "f"), ("(name):", "p")],
              [("    ", "p"), ('"""Say hello."""', "s")],
              [("    ", "p"), ("return ", "k"), ('f"Hello, {name}"', "s")],
              [("", "p")],
              [("print", "v"), ("(greet(", "p"), ('"Light"', "s"), ("))", "p")]],
        orbs=[((300, 80), 640, g.GREEN, 0.74), ((1040, 560), 700, g.BLUE, 0.80), ((700, 300), 340, g.TEAL, 0.20)],
    ),
    "assembler": dict(
        glyph="0x", mono=True, size=190, label="LANGUAGE",
        code=[[("section ", "x"), (".text", "p")],
              [("global ", "x"), ("_start", "f")],
              [("_start:", "f")],
              [("    mov ", "k"), ("eax", "v"), (", ", "p"), ("4", "n"), ("   ", "p"), ("; write", "c")],
              [("    int ", "k"), ("0x80", "n")]],
        orbs=[((640, 760), 800, g.GREEN, 0.82), ((120, 90), 600, g.BLUE, 0.74)],
    ),
    "html": dict(
        glyph="</>", mono=True, size=170, label="LANGUAGE",
        code=[[("<!doctype html>", "x")],
              [("<main", "k"), (" class=", "v"), ('"page"', "s"), (">", "k")],
              [("  <h1>", "k"), ("Hello, Light", "p"), ("</h1>", "k")],
              [("  <p>", "k"), ("Make it yours.", "p"), ("</p>", "k")],
              [("</main>", "k")]],
        orbs=[((180, 120), 700, g.GREEN, 0.80), ((1100, 600), 680, g.BLUE, 0.78)],
    ),
    "css": dict(
        glyph="{ }", mono=True, size=170, label="LANGUAGE",
        code=[[(".page", "t"), (" {", "p")],
              [("  display", "v"), (": ", "p"), ("grid", "m"), (";", "p")],
              [("  gap", "v"), (": ", "p"), ("1rem", "n"), (";", "p")],
              [("  color", "v"), (": ", "p"), ("#35d07f", "n"), (";", "p")],
              [("}", "p")]],
        orbs=[((1040, 100), 740, g.BLUE, 0.84), ((200, 640), 660, g.GREEN, 0.76)],
    ),
    "markdown": dict(
        glyph="M↓", mono=False, size=170, label="DOCUMENTS",
        code=[[("# ", "h"), ("Light", "h")],
              [("", "p")],
              [("Write ", "p"), ("**docs**", "p"), (" in ", "p"), ("*Markdown*", "p"), (".", "p")],
              [("- ", "m"), ("lists", "p")],
              [("- ", "m"), ("`code`", "e"), (" and ", "p"), ("[links](#)", "l")]],
        orbs=[((140, 560), 700, g.BLUE, 0.80), ((1130, 150), 680, g.GREEN, 0.76)],
    ),
    "restructuredtext": dict(
        glyph=".rst", mono=True, size=140, label="DOCUMENTS",
        code=[[("Light", "h")],
              [("===", "h")],
              [("", "p")],
              [(".. ", "x"), ("note::", "k")],
              [("   Docs with ", "p"), ("*Sphinx*", "p"), (".", "p")]],
        orbs=[((640, 40), 760, g.TEAL, 0.50), ((1150, 660), 640, g.BLUE, 0.78), ((110, 560), 560, g.GREEN, 0.66)],
    ),
    "shell-make": dict(
        glyph="$_", mono=True, size=210, label="LANGUAGE",
        code=[[("all", "f"), (": kernel", "p")],
              [("kernel", "f"), (":", "p")],
              [("\t$(CC) -o ", "p"), ("$@", "v"), (" main.c", "p")],
              [("run", "f"), (": all", "p")],
              [("\t./kernel", "p")]],
        orbs=[((700, 30), 700, g.GREEN, 0.78), ((260, 700), 700, g.BLUE, 0.76)],
    ),
}


def gradient_text(canvas, xy, text, fnt, c0=(60, 235, 130), c1=(70, 130, 255), anchor="mm", alpha=0.95):
    h, w = canvas.shape[:2]
    m = g.text_mask(w, h, xy, text, fnt, anchor)
    ink = g.diagonal(w, h, c0, c1, 55)
    ink[..., 3] = m * alpha
    return g.over(canvas, ink)


def code_card(canvas, rect, lines):
    """The frosted code card with its lines of code."""
    x0, y0, x1, y1 = rect
    h, w = canvas.shape[:2]
    mask = g.rounded_mask(w, h, rect, 46)
    canvas = g.glass(canvas, mask, tint_alpha=0.06, frost=22, rim=0.42, highlight=0.18, dark_edge=0.22, rim_width=3)
    # three dots, the way a window has
    for i, col in enumerate([(255, 100, 100), (245, 200, 80), (60, 215, 110)]):
        cx, cy = x0 + 44 + i * 30, y0 + 40
        canvas = g.over(canvas, g.layer(w, h, col, g.circle_mask(w, h, cx, cy, 8) * 0.75))
    fnt = g.mono(33)
    adv = fnt.getlength("M")
    y = y0 + 92
    for line in lines:
        x = x0 + 44
        for text, key in line:
            if text:
                canvas = g.draw_text(canvas, (x, y), text.replace("\t", "    "), fnt, C[key], 0.96)
                x += adv * len(text.replace("\t", "    "))
        y += 52
    return canvas


def draw(module, path):
    scene = SCENES[module["id"]]
    rnd = random.Random(module["id"])
    canvas = g.vgradient(W, H, (9, 14, 13), (6, 8, 14))
    for (cx, cy), r, col, a in scene["orbs"]:
        canvas = g.over(canvas, g.glow(W, H, cx, cy, r, col, a))
    # the glass: a card with the code, a tile with the mark, a pane under the title
    card = (470, 96, 1210, 96 + 92 + 52 * max(len(scene["code"]), 4) + 20)
    canvas = code_card(canvas, card, scene["code"])
    tile = g.rounded_mask(W, H, (64, 64, 304, 304), 70)
    canvas = g.glass(canvas, tile, tint_alpha=0.07, frost=18, rim=0.55, highlight=0.28, dark_edge=0.25, rim_width=3)
    # the mark is as big as fits in the tile
    size = scene["size"]
    while True:
        fnt = g.mono(size) if scene["mono"] else g.inter(size)
        if fnt.getlength(scene["glyph"]) <= 176 or size <= 60:
            break
        size -= 6
    glow_layer = g.blur(g.layer(W, H, g.GREEN, g.text_mask(W, H, (184, 184), scene["glyph"], fnt, "mm") * 0.5), 22)
    canvas = g.over(canvas, glow_layer)
    canvas = gradient_text(canvas, (184, 184), scene["glyph"], fnt)
    # drops of glass on the edges of the panes
    for cx, cy, r in [(card[0] - 6, card[3] - 40, 38), (card[2] - 90, card[3] + 34, 24), (304 + 24, 304 - 10, 30), (card[0] + 90, card[1] - 22, 16)]:
        canvas = g.bubble(canvas, cx + rnd.randint(-6, 6), cy + rnd.randint(-6, 6), r, strength=0.95)
    # the name
    canvas = g.draw_text(canvas, (72, 520), scene["label"], g.inter(24), g.WHITE, 0.50)
    canvas = g.draw_text(canvas, (68, 560), module["name"], g.inter(84), g.WHITE, 0.98)
    # a thin light along the bottom edge and a vignette, to hold the picture together
    ys = np.linspace(0, 1, H, dtype=np.float32)[:, None] * np.ones((1, W), np.float32)
    xs = np.linspace(0, 1, W, dtype=np.float32)[None, :] * np.ones((H, 1), np.float32)
    vignette = np.clip(((xs - 0.5) ** 2 + (ys - 0.5) ** 2) * 1.6 - 0.12, 0, 1)
    canvas = g.over(canvas, g.layer(W, H, (0, 0, 0), vignette * 0.55))
    img = g.to_image(canvas).convert("RGB").resize((OUT_W, OUT_H), Image.LANCZOS)
    img.save(path, optimize=True)
