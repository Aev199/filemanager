# Papka icon: an open yellow folder after the reference picture.
# usage: python3 tools/icon/draw.py crates/desktop/assets/app
import sys
from PIL import Image, ImageDraw
S = 4096                      # supersampled canvas
k = S / 1024
def P(points): return [(x * k, y * k) for x, y in points]
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
BACK, FRONT, EDGE = (222, 154, 34, 255), (255, 200, 58, 255), (240, 178, 40, 255)

def rounded_polygon(points, radius, fill):
    # Fill the polygon, then a thick curved outline rounds its corners.
    pts = P(points)
    d.polygon(pts, fill=fill)
    d.line(pts + [pts[0]], fill=fill, width=int(radius * 2 * k), joint="curve")
    for x, y in pts:
        r = radius * k
        d.ellipse((x - r, y - r, x + r, y + r), fill=fill)

# Back panel with the tab on the left.
rounded_polygon([(96, 176), (350, 176), (420, 250), (850, 250), (850, 840), (96, 840)], 34, BACK)
# Front flap, skewed like an opened folder.
rounded_polygon([(250, 400), (960, 400), (880, 860), (110, 860)], 30, FRONT)
# A thin lighter line along the flap's top edge.
d.line(P([(262, 402), (948, 402)]), fill=(255, 216, 102, 255), width=int(10 * k))

out = sys.argv[1]
big = img.resize((1024, 1024), Image.LANCZOS)
big.resize((256, 256), Image.LANCZOS).save(out + "/icon.png")
big.save(out + "/icon.ico", sizes=[(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)])
