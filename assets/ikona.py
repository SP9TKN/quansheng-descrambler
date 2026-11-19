"""Rysuje ikonę programu: ucho i fale radiowe. Uruchamiać z katalogu assets; wynik: ikona.ico, ikona_256.png, ikona_64.rgba."""
import math
import sys
from PIL import Image, ImageDraw

R = 1024  # płótno robocze, zmniejszane z wygładzaniem


def bezier(p0, p1, p2, p3, n=60):
    w = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        w.append(tuple(u**3 * a + 3 * u * u * t * b + 3 * u * t * t * c + t**3 * d for a, b, c, d in zip(p0, p1, p2, p3)))
    return w


def krzywa(*odcinki):
    w = []
    for o in odcinki:
        w += bezier(*o)
    return w


def gruba(d, pts, szer, kolor):
    """Linia stemplowana kołami: gładka, bez szczelin na załamaniach."""
    r = szer / 2
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        kroki = max(1, int(math.hypot(x1 - x0, y1 - y0)))
        for i in range(kroki + 1):
            x, y = x0 + (x1 - x0) * i / kroki, y0 + (y1 - y0) * i / kroki
            d.ellipse((x - r, y - r, x + r, y + r), fill=kolor)


# Tło: zaokrąglony kwadrat z pionowym gradientem.
gora, dol = (24, 92, 112), (9, 40, 66)
grad = Image.new("RGBA", (R, R))
for y in range(R):
    t = y / (R - 1)
    grad.paste(tuple(int(a + (b - a) * t) for a, b in zip(gora, dol)) + (255,), (0, y, R, y + 1))
maska = Image.new("L", (R, R), 0)
ImageDraw.Draw(maska).rounded_rectangle((20, 20, R - 21, R - 21), radius=210, fill=255)
img = Image.new("RGBA", (R, R), (0, 0, 0, 0))
img.paste(grad, (0, 0), maska)
d = ImageDraw.Draw(img)

JASNY = (244, 248, 250, 255)
FALA = (92, 220, 190, 255)

# Małżowina: bryła ucha skierowana w prawo, ku falom.
obrys = krzywa(
    ((500, 150), (300, 150), (190, 300), (205, 470)),
    ((205, 470), (215, 590), (300, 640), (335, 720)),
    ((335, 720), (365, 790), (405, 870), (500, 870)),
    ((500, 870), (590, 870), (625, 790), (610, 710)),
    ((610, 710), (600, 640), (690, 570), (725, 480)),
    ((725, 480), (790, 300), (690, 150), (500, 150)),
)
d.polygon(obrys, fill=JASNY)
# Wnętrze: zawinięcie i przewód słuchowy.
CIEMNY = (14, 58, 82, 255)
gruba(d, krzywa(((325, 430), (320, 300), (420, 245), (510, 255)), ((510, 255), (610, 265), (655, 355), (610, 450))), 56, CIEMNY)
gruba(d, krzywa(((610, 450), (575, 515), (490, 525), (465, 600)), ((465, 600), (450, 650), (480, 700), (500, 730))), 56, CIEMNY)
gruba(d, krzywa(((410, 450), (420, 385), (490, 365), (525, 410))), 46, CIEMNY)

# Fale radiowe wchodzące do ucha.
for promien, szer in ((70, 46), (145, 46), (220, 46)):
    sr = (725, 400)
    luk =[(sr[0] + promien * math.cos(math.radians(a)), sr[1] + promien * math.sin(math.radians(a))) for a in range(-48, 49, 2)]
    gruba(d, luk, szer, FALA)

img.resize((256, 256), Image.LANCZOS).save("ikona_256.png")
img.resize((256, 256), Image.LANCZOS).save("ikona.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
with open("ikona_64.rgba", "wb") as f:
    f.write(img.resize((64, 64), Image.LANCZOS).convert("RGBA").tobytes())
# Podgląd w małych rozmiarach na jasnym i ciemnym tle.
pod = Image.new("RGBA", (760, 300), (235, 235, 235, 255))
x = 10
for s in (256, 64, 48, 32, 24, 16):
    m = img.resize((s, s), Image.LANCZOS)
    pod.alpha_composite(m, (x, 20))
    x += s + 14
if len(sys.argv) > 1:
    pod.save(sys.argv[1])
