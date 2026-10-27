#!/usr/bin/env python3
"""Outline a line of text into an SVG path (used for the "Markise" wordmark).

usage: outline_text.py FONT.ttf WGHT SIZE TRACKING TEXT
prints: width<TAB>cap-height<TAB>path-d (origin at the left end of the baseline, y down)

Needs `fonttools` and `uharfbuzz` (pip). The lockups use Playfair Display (wordmark) and
Nunito (tagline), both SIL Open Font License 1.1 (github.com/google/fonts, ofl/playfairdisplay and
ofl/nunito); outlined glyphs only, the font files are not committed.
"""
import sys

import uharfbuzz as hb
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont


def main() -> None:
    font_path, wght, size, tracking, text = sys.argv[1], float(sys.argv[2]), float(sys.argv[3]), float(sys.argv[4]), sys.argv[5]
    font = TTFont(font_path)
    if "fvar" in font:
        font = instantiateVariableFont(font, {"wght": wght})
    upem = font["head"].unitsPerEm
    scale = size / upem

    blob = hb.Blob.from_file_path(font_path)
    hb_font = hb.Font(hb.Face(blob))
    hb_font.set_variations({"wght": wght})
    buf = hb.Buffer()
    buf.add_str(text)
    buf.guess_segment_properties()
    hb.shape(hb_font, buf, {"kern": True, "liga": True})

    glyph_set = font.getGlyphSet()
    order = font.getGlyphOrder()
    pen = SVGPathPen(glyph_set, ntos=lambda v: f"{v:.1f}".rstrip("0").rstrip("."))
    x = 0.0
    n = len(buf.glyph_infos)
    for i, (info, pos) in enumerate(zip(buf.glyph_infos, buf.glyph_positions)):
        name = order[info.codepoint]
        # font units -> px, flip y (font y up, SVG y down)
        t = TransformPen(pen, (scale, 0, 0, -scale, x + pos.x_offset * scale, -pos.y_offset * scale))
        glyph_set[name].draw(t)
        x += pos.x_advance * scale + (tracking if i < n - 1 else 0)
    cap = font["OS/2"].sCapHeight * scale
    print(f"{x:.1f}\t{cap:.1f}\t{pen.getCommands()}")


if __name__ == "__main__":
    main()
