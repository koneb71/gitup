#!/usr/bin/env python3
"""Generate the app icon.

Draws the same git-commit mark the app uses in its toolbar — a circle on a
horizontal line — so the icon in the Dock matches what you see once the window
opens. Rendered at 4x and downsampled, which is cheaper than hinting the small
sizes by hand and looks better than nearest-neighbour scaling.
"""

from PIL import Image, ImageDraw
import os
import sys

BG_TOP = (24, 30, 39)
BG_BOTTOM = (14, 17, 22)
ACCENT = (242, 133, 63)

SIZES = [16, 32, 48, 64, 128, 256, 512, 1024]

# The sizes the freedesktop hicolor theme looks in, and the ones Windows picks
# between for the taskbar, the title bar and Explorer's various views.
HICOLOR_SIZES = [16, 32, 48, 64, 128, 256, 512]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def rounded_mask(size: int, radius: int) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [(0, 0), (size - 1, size - 1)], radius=radius, fill=255
    )
    return mask


def render(size: int) -> Image.Image:
    # Supersample, then reduce: the diagonal edges of the rounded corners and
    # the circle both need it at 16px.
    scale = 4
    s = size * scale
    image = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    # Vertical gradient background.
    for y in range(s):
        t = y / max(s - 1, 1)
        colour = tuple(
            round(BG_TOP[i] + (BG_BOTTOM[i] - BG_TOP[i]) * t) for i in range(3)
        )
        draw.line([(0, y), (s, y)], fill=colour + (255,))

    # The commit mark.
    cx = cy = s / 2
    radius = s * 0.16
    stroke = max(2, round(s * 0.055))
    arm = s * 0.30

    draw.line(
        [(cx - arm, cy), (cx - radius, cy)],
        fill=ACCENT + (255,),
        width=stroke,
    )
    draw.line(
        [(cx + radius, cy), (cx + arm, cy)],
        fill=ACCENT + (255,),
        width=stroke,
    )
    draw.ellipse(
        [(cx - radius, cy - radius), (cx + radius, cy + radius)],
        outline=ACCENT + (255,),
        width=stroke,
    )

    image.putalpha(rounded_mask(s, round(s * 0.22)))
    return image.resize((size, size), Image.LANCZOS)


def main() -> int:
    out = sys.argv[1] if len(sys.argv) > 1 else "assets/icon"
    iconset = os.path.join(out, "Gitup.iconset")
    os.makedirs(iconset, exist_ok=True)

    rendered = {size: render(size) for size in SIZES}

    # macOS: an .iconset directory that `iconutil` turns into an .icns. 48 is
    # not one of the sizes it accepts, so it is skipped here.
    for size in SIZES:
        if size not in (48, 1024) and size <= 512:
            rendered[size].save(os.path.join(iconset, f"icon_{size}x{size}.png"))
        if size >= 64 or size == 32:
            # Retina variants are the next size up, named for the previous one.
            half = size // 2
            if half in (16, 32, 128, 256, 512):
                rendered[size].save(os.path.join(iconset, f"icon_{half}x{half}@2x.png"))

    # Linux: the freedesktop hicolor layout, copied into place verbatim by
    # scripts/package-linux.sh.
    hicolor = os.path.join(out, "hicolor")
    for size in HICOLOR_SIZES:
        directory = os.path.join(hicolor, f"{size}x{size}", "apps")
        os.makedirs(directory, exist_ok=True)
        rendered[size].save(os.path.join(directory, "gitup.png"))

    # Windows: a single multi-resolution .ico, embedded into the executable by
    # build.rs so the binary is not blank in Explorer.
    ico = os.path.join(out, "gitup.ico")
    rendered[256].save(
        ico, format="ICO", sizes=[(size, size) for size in ICO_SIZES]
    )

    # A plain PNG too, for the window icon at runtime.
    rendered[512].save(os.path.join(out, "gitup.png"))
    print(f"wrote {iconset}, {hicolor} and {ico}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
