#!/usr/bin/env python3
"""Generate the Markise brand SVGs from one vector geometry.

The geometry is a vector redraw of the owner's concept images in ../source/ (1254 x 1254 px;
coordinates below are in that pixel grid, measured from the dark image). Shapes are polygons
whose corners are filleted with circular arcs of the given radius.

usage: make_svgs.py PlayfairDisplay.ttf Nunito.ttf   (variable fonts, SIL OFL 1.1)
Writes into the parent folder (assets/brand/). Run via build.sh --svg.
"""
import math
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.dirname(HERE)

# Palettes: (background, letter, crown)
DARK = ("#1a1b26", "#bbc8f2", "#e0af68")
LIGHT = ("#faf6ee", "#1a1b26", "#daab55")

CX = 626.5  # vertical symmetry axis of the artwork

# Squircle background: superellipse |x|^n + |y|^n = a^n, centred on the artwork.
SQ_CENTER = (CX, 630.0)
SQ_A = 580.0
SQ_N = 3.8


def mirror(points):
    """Left-half outline (top to bottom along the axis side) -> full symmetric outline."""
    right = [(2 * CX - x, y, r) for (x, y, r) in reversed(points)]
    return points + right


# "M": left half, clockwise from the bottom of the inner left stem edge; (x, y, fillet radius).
M_LEFT = [
    (430, 1042, 60),  # bottom-right of left stem
    (281, 1042, 60),  # bottom-left
    (281, 505, 72),  # top-left (outer)
    (454, 505, 24),  # top, where the diagonal starts
    (CX, 740, 8),  # top notch of the V (on the axis; mirrored point dropped below)
]
M_INNER_LEFT = [
    (CX, 972, 45),  # bottom apex of the V (on the axis)
    (430, 724, 16),  # crotch between diagonal and left stem
]

# Crown: 5 points; left half from the bottom-left corner to the centre tip.
CROWN_LEFT = [
    (398, 461, 12),  # bottom-left corner
    (330, 313, 6),  # outer tip
    (414, 349, 4),  # valley
    (452, 261, 6),  # inner tip
    (530, 318, 4),  # valley
    (CX, 208, 7),  # centre tip
]
# Diamond cut-outs: (cx, top, mid_y, half_width, bottom)
DIAMONDS = [(478, 340, 380, 24, 424), (CX, 286, 356, 40, 426), (2 * CX - 478, 340, 380, 24, 424)]


def m_outline():
    outer = M_LEFT[:-1]
    top_notch = M_LEFT[-1]
    right_outer = [(2 * CX - x, y, r) for (x, y, r) in reversed(outer)]
    inner_right = [(2 * CX - x, y, r) for (x, y, r) in reversed(M_INNER_LEFT[1:])]
    # clockwise: left stem bottom-right, around the left stem, over the top notch, down the
    # right stem, back along the bottom V.
    return outer + [top_notch] + right_outer + inner_right + [M_INNER_LEFT[0]] + M_INNER_LEFT[1:]


def crown_outline():
    pts = CROWN_LEFT[:-1] + [CROWN_LEFT[-1]] + [(2 * CX - x, y, r) for (x, y, r) in reversed(CROWN_LEFT[:-1])]
    return pts


def fmt(v: float) -> str:
    s = f"{v:.1f}"
    return s[:-2] if s.endswith(".0") else s


def rounded_path(points) -> str:
    """Closed polygon with circular fillets (radius r at each vertex)."""
    n = len(points)
    out = []
    for i in range(n):
        px, py, _ = points[i - 1]
        x, y, r = points[i]
        nx, ny, _ = points[(i + 1) % n]
        v1 = (px - x, py - y)
        v2 = (nx - x, ny - y)
        l1, l2 = math.hypot(*v1), math.hypot(*v2)
        u1 = (v1[0] / l1, v1[1] / l1)
        u2 = (v2[0] / l2, v2[1] / l2)
        cos_t = max(-1.0, min(1.0, u1[0] * u2[0] + u1[1] * u2[1]))
        theta = math.acos(cos_t)
        if r <= 0 or theta < 1e-3 or abs(math.pi - theta) < 1e-3:
            out.append(("L" if out else "M") + f"{fmt(x)} {fmt(y)}")
            continue
        t = min(r / math.tan(theta / 2), l1 / 2, l2 / 2)
        rr = t * math.tan(theta / 2)
        a = (x + u1[0] * t, y + u1[1] * t)
        b = (x + u2[0] * t, y + u2[1] * t)
        cross = u1[0] * u2[1] - u1[1] * u2[0]
        sweep = 0 if cross > 0 else 1
        out.append(("L" if out else "M") + f"{fmt(a[0])} {fmt(a[1])}")
        out.append(f"A{fmt(rr)} {fmt(rr)} 0 0 {sweep} {fmt(b[0])} {fmt(b[1])}")
    return "".join(out) + "Z"


def diamond_path(cx, top, mid, hw, bottom) -> str:
    return f"M{fmt(cx)} {top}L{fmt(cx + hw)} {mid}L{fmt(cx)} {bottom}L{fmt(cx - hw)} {mid}Z"


def squircle_path(cx, cy, a, n, steps=96) -> str:
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        x = cx + a * math.copysign(abs(c) ** (2 / n), c)
        y = cy + a * math.copysign(abs(s) ** (2 / n), s)
        pts.append(f"{fmt(x)} {fmt(y)}")
    return "M" + "L".join(pts) + "Z"


M_D = rounded_path(m_outline())
CROWN_D = rounded_path(crown_outline()) + "".join(diamond_path(*d) for d in DIAMONDS)
SQ_D = squircle_path(*SQ_CENTER, SQ_A, SQ_N)
MARK_BOX = (261, 188, 731, 874)  # x, y, w, h around crown + M with a 20 px margin
ICON_BOX = (SQ_CENTER[0] - SQ_A, SQ_CENTER[1] - SQ_A, 2 * SQ_A, 2 * SQ_A)


def mark(letter: str, crown: str) -> str:
    return f'<path fill="{crown}" fill-rule="evenodd" d="{CROWN_D}"/><path fill="{letter}" d="{M_D}"/>'


def svg(box, inner: str, title: str, w=None, h=None) -> str:
    vb = " ".join(fmt(v) for v in box)
    size = f' width="{fmt(w)}" height="{fmt(h)}"' if w else ""
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}"{size} role="img" aria-label="{title}">'
        f"<title>{title}</title>{inner}</svg>\n"
    )


def icon(palette, scale: float = 1.0, shadow: bool = False) -> str:
    bg, letter, crown = palette
    body = f'<path fill="{bg}" d="{SQ_D}"{" filter=\"url(#sh)\"" if shadow else ""}/>' + mark(letter, crown)
    if scale != 1.0:
        cx, cy = SQ_CENTER
        body = f'<g transform="translate({fmt(cx)} {fmt(cy)}) scale({scale}) translate({fmt(-cx)} {fmt(-cy)})">{body}</g>'
    if shadow:
        body = (
            '<defs><filter id="sh" x="-20%" y="-20%" width="140%" height="140%">'
            '<feDropShadow dx="0" dy="10" stdDeviation="12" flood-color="#000" flood-opacity="0.28"/>'
            "</filter></defs>" + body
        )
    return body


# Simplified artwork for 16-32 px: bigger letter, solid three-point crown (the five points and
# diamond cut-outs turn to mush below ~48 px), squircle at full bleed.
def favicon(palette) -> str:
    bg, letter, crown = palette
    # 64 x 64 grid
    sq = squircle_path(32, 32, 32, 4.2, 64)
    m_pts = [
        (25, 58, 4), (8, 58, 4), (8, 27, 5), (19, 27, 2), (32, 43, 1),
        (45, 27, 2), (56, 27, 5), (56, 58, 4), (39, 58, 4), (39, 46, 1), (32, 54, 3), (25, 46, 1),
    ]
    crown_pts = [(14, 23, 1), (9, 9, 0.5), (20, 14, 0.5), (32, 3, 0.5), (44, 14, 0.5), (55, 9, 0.5), (50, 23, 1)]
    return (
        f'<path fill="{bg}" d="{sq}"/>'
        f'<path fill="{crown}" d="{rounded_path(crown_pts)}"/>'
        f'<path fill="{letter}" d="{rounded_path(m_pts)}"/>'
    )


def outline(font: str, wght: int, size: float, tracking: float, text: str):
    """-> (width, cap height, path d) of `text` outlined at `size` px."""
    out = subprocess.run(
        [sys.executable, os.path.join(HERE, "outline_text.py"), font, str(wght), str(size), str(tracking), text],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    width, cap, d = out.split("\t", 2)
    return float(width), float(cap), d


def write(name: str, content: str) -> None:
    with open(os.path.join(OUT, name), "w", encoding="utf-8") as f:
        f.write(content)
    print("wrote", name)


def main() -> None:
    serif, sans = sys.argv[1], sys.argv[2]
    for name, pal in (("dark", DARK), ("light", LIGHT)):
        write(f"markise-icon-{name}.svg", svg(ICON_BOX, icon(pal), "Markise", 1024, 1024))
        # macOS Big Sur grid: 824/1024 body, centred, soft shadow.
        write(f"markise-icon-{name}-macos.svg", svg(ICON_BOX, icon(pal, 0.8047, True), "Markise", 1024, 1024))
        write(f"markise-favicon-{name}.svg", svg((0, 0, 64, 64), favicon(pal), "Markise", 64, 64))

    # Mark only (transparent): "light" = for light backgrounds, "dark" = for dark backgrounds.
    x, y, w, h = MARK_BOX
    write("markise-mark-light.svg", svg(MARK_BOX, mark(LIGHT[1], LIGHT[2]), "Markise", w, h))
    write("markise-mark-dark.svg", svg(MARK_BOX, mark(DARK[1], DARK[2]), "Markise", w, h))

    # Horizontal lockup after the owner's wordmark concept (source/markise-wordmark-*.png):
    # mark, then "Markise" in a high-contrast serif, then a letter-spaced caps tagline set to
    # the wordmark's width. Proportions relative to the mark height (crown top to M baseline).
    mark_top, mark_bottom = 208, 1042
    mh = mark_bottom - mark_top
    probe_w, probe_cap, _ = outline(serif, 600, 1000, 0, "Markise")
    size = 1000 * (0.66 * mh) / probe_cap  # wordmark cap height = 66 % of the mark
    word_w, word_cap, word_d = outline(serif, 600, size, 0, "Markise")
    tag_text = "THE MARQUISE OF MARKDOWN EDITORS"
    t_size = 1000 * (0.075 * mh) / outline(sans, 500, 1000, 0, tag_text)[1]
    t_w0, _, _ = outline(sans, 500, t_size, 0, tag_text)
    tracking = (word_w - t_w0) / (len(tag_text) - 1)
    tag_w, _, tag_d = outline(sans, 500, t_size, tracking, tag_text)

    gap = 0.22 * mh
    text_x = (CX + (972 - CX)) - x + gap  # right edge of the M + gap
    word_base = 0.87 * mh  # baselines measured from the crown top
    tag_base = mh * 1.10
    pad = 40
    for variant, (bg, letter, crown), word_fill, tag_fill in (
        ("light", LIGHT, LIGHT[1], "#4a4b60"),
        ("dark", DARK, "#c0caf5", "#a9b1d6"),
    ):
        mk = f'<g transform="translate({fmt(pad - x + 20)} {fmt(pad - mark_top)})">{mark(letter, crown)}</g>'
        tx = pad + 20 + text_x - 20
        word = f'<path fill="{word_fill}" transform="translate({fmt(tx)} {fmt(pad + word_base)})" d="{word_d}"/>'
        total_w = tx + word_w + pad
        h1 = pad + mh + pad
        write(f"markise-lockup-{variant}.svg", svg((0, 0, total_w, h1), mk + word, "Markise", total_w / 4, h1 / 4))
        tag = f'<path fill="{tag_fill}" transform="translate({fmt(tx + 4)} {fmt(pad + tag_base)})" d="{tag_d}"/>'
        h2 = pad + tag_base + pad
        write(
            f"markise-lockup-tagline-{variant}.svg",
            svg((0, 0, total_w, h2), mk + word + tag, "Markise: the marquise of Markdown editors", total_w / 4, h2 / 4),
        )


if __name__ == "__main__":
    main()
