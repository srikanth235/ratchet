#!/usr/bin/env python3
"""Writes the app icon's layers from one geometry table, so the Icon Composer file and the flat reference cannot drift.

  python3 design/tools/build-icon.py

Single source of the icon: apps/mac/Ratchet/Icon/AppIcon.icon (icon.json, written by hand; Assets/*.svg, written here).
Also writes design/icon/ratchet-icon.svg, the flat 1024 x 1024 reference with a tile. The in-app mark (RatchetMark.swift,
design/icon/ratchet-mark.svg) keeps all six chambers; the icon is its small-size cut:

  * the two smallest chambers (radius 31.5) are left out: at 16 pt they are half a pixel wide and read as noise;
  * the other four are scaled so the glyph is about 50% of the canvas width (it was 57%) and centred;
  * the glass chambers are brighter, so the three sealed ones still separate from the brass newest chamber at 16 pt.
"""
import os

ROOT = os.path.join(os.path.dirname(__file__), "..", "..")
ICON = os.path.join(ROOT, "apps", "mac", "Ratchet", "Icon", "AppIcon.icon", "Assets")
FLAT = os.path.join(ROOT, "design", "icon", "ratchet-icon.svg")

# (center, start, end, radius, sweep, opacity of the glass chamber). Source: design/icon/ratchet-mark.svg.
CHAMBERS = [
    ((361.6, 424.9), (439.2, 424.9), (361.6, 347.3), 77.6, 0, 0.55),
    ((351.7, 471.0), (227.9, 471.0), (351.7, 347.3), 123.8, 1, 0.78),
    ((444.0, 480.9), (444.0, 697.0), (227.9, 480.9), 216.1, 1, 1.0),
]
NEWEST = ((453.9, 342.5), (453.9, 697.0), (808.4, 342.5), 354.5, 0)

CANVAS = 1024.0
TARGET = 0.50  # glyph width as a share of the canvas

xs = [227.9, 808.4]
ys = [342.5, 697.0]
SCALE = TARGET * CANVAS / (xs[1] - xs[0])
CX, CY = (xs[0] + xs[1]) / 2, (ys[0] + ys[1]) / 2


def pt(p):
    return (SCALE * (p[0] - CX) + CANVAS / 2, SCALE * (p[1] - CY) + CANVAS / 2)


def path(chamber, extra=""):
    c, a, b, r, sweep = chamber[:5]
    (cx, cy), (ax, ay), (bx, by) = pt(c), pt(a), pt(b)
    rr = r * SCALE
    return (f'<path d="M{cx:.1f} {cy:.1f}L{ax:.1f} {ay:.1f}A{rr:.1f} {rr:.1f} 0 0 {sweep} {bx:.1f} {by:.1f}Z"{extra}/>')


def svg(body):
    return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">\n'
            + body + "\n</svg>\n")


def write(path_, text):
    os.makedirs(os.path.dirname(path_), exist_ok=True)
    with open(path_, "w") as f:
        f.write(text)


front = path(NEWEST, ' fill="#E2BE7C"')
back = "\n".join(path(ch, f' fill="#FFFFFF" fill-opacity="{ch[5]:.2f}"') for ch in CHAMBERS)
write(os.path.join(ICON, "front.svg"), svg("  " + front))
write(os.path.join(ICON, "back.svg"), svg("\n".join("  " + line for line in back.split("\n"))))

glass = "\n".join("  " + path(ch, f' fill="#DCE3EE" fill-opacity="{ch[5]:.2f}"') for ch in CHAMBERS)
flat = (
    "<title>Ratchet</title>\n<defs>\n"
    '  <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1B2539"/><stop offset="1" stop-color="#05080E"/></linearGradient>\n'
    '  <linearGradient id="brass" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#F8E4B8"/><stop offset=".6" stop-color="#E2BE7C"/><stop offset="1" stop-color="#BE8F49"/></linearGradient>\n'
    '  <clipPath id="tile"><rect width="1024" height="1024" rx="230"/></clipPath>\n</defs>\n'
    '<g clip-path="url(#tile)">\n  <rect width="1024" height="1024" fill="url(#bg)"/>\n'
    + glass + "\n  " + path(NEWEST, ' fill="url(#brass)"') + "\n</g>"
)
write(FLAT, svg(flat))
print(f"glyph scale {SCALE:.3f}, width {SCALE * (xs[1] - xs[0]) / CANVAS:.0%} of the canvas")
