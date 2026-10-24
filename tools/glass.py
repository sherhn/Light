"""Drawing helpers for the look of Light: a black ground with green and blue glows and "liquid glass" on top - frosted,
translucent panels with a light rim and a specular highlight. Used for the app icon and the covers of the modules.

Everything works on RGBA float arrays (numpy) in 0..1 and is converted to Pillow images at the end.
"""
import math
import pathlib

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = pathlib.Path(__file__).resolve().parent.parent
FONTS = ROOT / "assets" / "fonts"

# the palette of Light (see apps/light/src/theme.rs)
BLACK = (8, 11, 12)
GREEN = (31, 214, 107)       # the glow; the UI accent is a touch deeper: 2DBE60
SBER = (33, 160, 56)         # the classic green
BLUE = (47, 107, 255)
BLUE_UI = (59, 130, 246)
TEAL = (79, 216, 200)
WHITE = (255, 255, 255)


def font(name, size):
    return ImageFont.truetype(str(FONTS / name), size)


def inter(size, bold=True):
    return font("Inter-SemiBold.ttf" if bold else "Inter-Regular.ttf", size)


def mono(size):
    return font("JetBrainsMono-Regular.ttf", size)


# ------------------------------------------------------------------ arrays

def new(w, h, color=BLACK, alpha=1.0):
    a = np.zeros((h, w, 4), np.float32)
    a[..., :3] = np.array(color, np.float32) / 255.0
    a[..., 3] = alpha
    return a


def to_image(a):
    return Image.fromarray((np.clip(a, 0, 1) * 255 + 0.5).astype(np.uint8), "RGBA")


def from_image(img):
    return np.asarray(img.convert("RGBA"), np.float32) / 255.0


def over(dst, src):
    """Composite `src` (premultiplication-free RGBA) over `dst`."""
    sa = src[..., 3:4]
    da = dst[..., 3:4]
    oa = sa + da * (1 - sa)
    rgb = np.where(oa > 1e-6, (src[..., :3] * sa + dst[..., :3] * da * (1 - sa)) / np.maximum(oa, 1e-6), 0)
    return np.concatenate([rgb, oa], axis=-1)


def layer(w, h, rgb, alpha):
    """A flat colour with a per-pixel alpha (h x w array)."""
    a = np.zeros((h, w, 4), np.float32)
    a[..., :3] = np.array(rgb, np.float32) / 255.0
    a[..., 3] = np.clip(alpha, 0, 1)
    return a


def vgradient(w, h, top, bottom):
    t = np.linspace(0, 1, h, dtype=np.float32)[:, None, None]
    top, bottom = np.array(top, np.float32) / 255.0, np.array(bottom, np.float32) / 255.0
    rgb = top * (1 - t) + bottom * t
    a = np.ones((h, w, 4), np.float32)
    a[..., :3] = np.broadcast_to(rgb, (h, w, 3))
    return a


def diagonal(w, h, c0, c1, angle=45.0):
    """A linear gradient c0 -> c1 along `angle` degrees (0 = left to right, 90 = top to bottom)."""
    ca, sa = math.cos(math.radians(angle)), math.sin(math.radians(angle))
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float32)
    proj = (xs - w / 2) * ca + (ys - h / 2) * sa
    half = (abs(w * ca) + abs(h * sa)) / 2
    t = np.clip((proj / half + 1) / 2, 0, 1)[..., None]
    c0, c1 = np.array(c0, np.float32) / 255.0, np.array(c1, np.float32) / 255.0
    a = np.ones((h, w, 4), np.float32)
    a[..., :3] = c0 * (1 - t) + c1 * t
    return a


def glow(w, h, cx, cy, radius, rgb, strength=0.6, squash=1.0):
    """A soft round glow (an orb of light), `squash` < 1 makes it flatter."""
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float32)
    d = np.sqrt(((xs - cx) / 1.0) ** 2 + ((ys - cy) / squash) ** 2) / radius
    alpha = np.clip(1 - d, 0, 1) ** 2.2 * strength
    return layer(w, h, rgb, alpha)


def blur(a, radius):
    img = to_image(a)
    return from_image(img.filter(ImageFilter.GaussianBlur(radius)))


# ------------------------------------------------------------------ shapes

def rounded_mask(w, h, box, radius, supersample=3):
    """An antialiased rounded rectangle as an h x w array of 0..1."""
    big = Image.new("L", (w * supersample, h * supersample), 0)
    d = ImageDraw.Draw(big)
    x0, y0, x1, y1 = [v * supersample for v in box]
    d.rounded_rectangle([x0, y0, x1, y1], radius=radius * supersample, fill=255)
    return np.asarray(big.resize((w, h), Image.LANCZOS), np.float32) / 255.0


def circle_mask(w, h, cx, cy, r, supersample=3):
    return rounded_mask(w, h, (cx - r, cy - r, cx + r, cy + r), r, supersample)


def ring_from_mask(mask, width):
    """The edge of a mask: the mask minus itself shrunk by `width` pixels."""
    img = Image.fromarray((mask * 255).astype(np.uint8), "L")
    inner = img.filter(ImageFilter.MinFilter(max(3, int(width) * 2 + 1)))
    return np.clip(mask - np.asarray(inner, np.float32) / 255.0, 0, 1)


def glass(canvas, mask, tint=(255, 255, 255), tint_alpha=0.07, frost=14, rim=0.34, highlight=0.20, dark_edge=0.25, rim_width=2):
    """Put a pane of frosted glass with the shape of `mask` (h x w) over `canvas` (RGBA array).

    What is behind the pane is blurred (frost), the pane is tinted, its edge catches light on the top left and
    shades on the bottom right, and the top part carries a soft specular highlight."""
    h, w = mask.shape
    blurred = blur(canvas, frost)
    # brighten and saturate the frosted background a little, the way thick glass does
    frosted = blurred.copy()
    frosted[..., :3] = np.clip(frosted[..., :3] * 1.12 + 0.015, 0, 1)
    out = canvas * (1 - mask[..., None]) + frosted * mask[..., None]
    out[..., 3] = 1.0
    out = over(out, layer(w, h, tint, mask * tint_alpha))
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float32)
    ys_n = ys / max(h - 1, 1)
    xs_n = xs / max(w - 1, 1)
    # specular highlight: a bright cap on the upper part that fades out downwards and to the right
    cap = np.clip(1 - ys_n / 0.55, 0, 1) ** 1.6 * np.clip(1.15 - xs_n * 0.7, 0, 1)
    out = over(out, layer(w, h, WHITE, mask * cap * highlight))
    # the rim: light from the top left, a green-blue shade at the bottom right
    edge = ring_from_mask(mask, rim_width)
    light = np.clip(1 - (xs_n * 0.8 + ys_n * 1.0) / 1.1, 0, 1) ** 1.4
    out = over(out, layer(w, h, WHITE, edge * (0.10 + light * rim)))
    shade = np.clip((xs_n * 0.8 + ys_n * 1.0 - 0.8) / 1.0, 0, 1)
    out = over(out, layer(w, h, (20, 60, 40), edge * shade * dark_edge))
    return out


def text_mask(w, h, xy, text, fnt, anchor="la"):
    img = Image.new("L", (w, h), 0)
    ImageDraw.Draw(img).text(xy, text, font=fnt, fill=255, anchor=anchor)
    return np.asarray(img, np.float32) / 255.0


def draw_text(canvas, xy, text, fnt, rgb=WHITE, alpha=1.0, anchor="la"):
    h, w = canvas.shape[:2]
    return over(canvas, layer(w, h, rgb, text_mask(w, h, xy, text, fnt, anchor) * alpha))


def bubble(canvas, cx, cy, r, tint=(255, 255, 255), strength=1.0):
    """A glass droplet: clear in the middle, bright rim on one side, a soft reflection."""
    h, w = canvas.shape[:2]
    m = circle_mask(w, h, cx, cy, r)
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float32)
    d = np.sqrt((xs - cx) ** 2 + (ys - cy) ** 2) / r
    refl = np.clip(1 - np.sqrt((xs - (cx - r * 0.35)) ** 2 + (ys - (cy - r * 0.4)) ** 2) / (r * 0.55), 0, 1) ** 2
    rim_a = np.clip((d - 0.82) / 0.18, 0, 1) * m
    side = np.clip(0.5 + ((cx - xs) + (cy - ys)) / (2.2 * r), 0, 1)
    out = over(canvas, layer(w, h, tint, m * 0.05 * strength))
    out = over(out, layer(w, h, WHITE, rim_a * (0.15 + 0.5 * side) * strength))
    out = over(out, layer(w, h, WHITE, m * refl * 0.5 * strength))
    return out
