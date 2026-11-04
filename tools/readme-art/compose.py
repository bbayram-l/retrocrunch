"""Compose the README art from Retrocrunch renders (run from the repo root)."""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from PIL import Image
from pixfont import background, crt, draw_text, frame, save_gif, text_width

O = "test_out/readme"
OUT = "docs/readme"
os.makedirs(OUT, exist_ok=True)

WHITE = (240, 238, 255)
DIM = (150, 140, 200)
CYAN = (120, 230, 220)
GOLD = (255, 220, 120)
TITLE = ((255, 196, 90), (240, 70, 140))
BAND = (14, 11, 30)
HEADER = (44, 34, 92)


def centered(img, y, s, color, x0=0, w=None, **kw):
    w = img.width if w is None else w
    draw_text(img, x0 + (w - text_width(s, kw.get("scale", 1))) // 2, y, s, color, **kw)


# ---------------------------------------------------------------- hero.gif
def hero():
    frames = []
    for i in range(24):
        f = Image.open(f"{O}/hero/f{i:02d}.png").convert("RGB")
        img = background(420, 150, f"{O}/hero/f00.png")
        img.paste(f, (244, 0))
        draw_text(img, 22, 30, "RETROCRUNCH", None, scale=3, shadow=(40, 16, 60), gradient=TITLE)
        draw_text(img, 22, 62, "PS1-STYLE ASSET PIPELINE", CYAN)
        draw_text(img, 22, 76, "MODELS * TEXTURES * SPRITES * LEVELS", DIM)
        cursor = "_" if (i // 4) % 2 == 0 else " "
        draw_text(img, 22, 104, "> CRUNCH ANYTHING" + cursor, GOLD)
        draw_text(img, 22, 128, "GODOT 4 * CLI * MCP FOR AI AGENTS", (110, 100, 160))
        frames.append(crt(img))
    save_gif(frames, f"{OUT}/hero.gif", 90)


# ---------------------------------------------------------------- before_after.png
def before_after():
    w, h, gap = 136, 150, 6
    img = Image.new("RGB", (w * 3 + gap * 2, h + 36), BAND)
    img.paste(background(img.width, h, f"{O}/panels/ba_src.png"), (0, 0))
    caps = [
        ("SOURCE", "47,214 TRIS * 2K PBR", WHITE),
        ("RETROCRUNCH", "898 TRIS * 24 COLOURS", None),
        ("WIREFRAME", "128 PX * 8-BIT CLUT", CYAN),
    ]
    for k, name in enumerate(("ba_src", "ba_ps1", "ba_wire")):
        x = k * (w + gap)
        img.paste(Image.open(f"{O}/panels/{name}.png").convert("RGB"), (x, 0))
        title, sub, color = caps[k]
        if color:
            centered(img, h + 6, title, color, x0=x, w=w)
        else:
            centered(img, h + 6, title, None, x0=x, w=w, gradient=TITLE)
        centered(img, h + 20, sub, DIM, x0=x, w=w)
    for k in (1, 2):
        draw_text(img, k * (w + gap) - 8, h // 2 - 7, ">", GOLD, scale=2)
    crt(img).save(f"{OUT}/before_after.png")


# ---------------------------------------------------------------- sprites.gif
def sprites():
    sheet = Image.open(f"{O}/sprites/Fox_sprites.png").convert("RGBA")
    c = 64
    frames = []
    for fr in range(8):
        img = Image.new("RGB", (4 * c, 2 * c + 16), BAND)
        img.paste(background(4 * c, 2 * c, f"{O}/hero/f00.png"), (0, 0))
        for d in range(8):
            cell = sheet.crop((fr * c, d * c, (fr + 1) * c, (d + 1) * c))
            img.paste(cell, ((d % 4) * c, (d // 4) * c), cell)
        centered(img, 2 * c + 5, "8 DIRECTIONS X 8 FRAMES", DIM)
        frames.append(crt(img))
    save_gif(frames, f"{OUT}/sprites.gif", 90)


# ---------------------------------------------------------------- feature cards
def card(title):
    img = background(176, 132, f"{O}/panels/t_model.png")
    for y in range(12):
        for x in range(176):
            img.putpixel((x, y), HEADER)
    draw_text(img, 4, 3, title, WHITE, shadow=None)
    return img


def caption(img, s, color=GOLD):
    draw_text(img, 5, img.height - 11, s, color)


def paste_rgba(img, path, xy, size=None):
    im = Image.open(path).convert("RGBA")
    if size:
        im = im.resize(size, Image.NEAREST)
    img.paste(im, xy, im)


def cards():
    pals = {p["slug"]: p for p in json.load(open(f"{O}/palettes.json", encoding="utf-8"))["palettes"]}

    c = card("01 MODEL")
    c.paste(Image.open(f"{O}/panels/t_model.png").convert("RGB"), (0, 12))
    caption(c, "10,168 > 392 TRIS")
    yield "model", c

    c = card("02 PALETTES + DITHER")
    c.paste(Image.open(f"{O}/panels/t_pal.png").convert("RGB"), (0, 12))
    for row, slug in enumerate(("pico-8", "darkseed-16")):
        cols = pals[slug]["colors"]
        y = 104 + row * 13
        draw_text(c, 4, y + 1, slug.upper(), DIM)
        for k, hx in enumerate(cols[:16]):
            rgb = tuple(int(hx[i:i + 2], 16) for i in (1, 3, 5))
            for yy in range(y, y + 9):
                for xx in range(78 + k * 6, 78 + k * 6 + 5):
                    c.putpixel((xx, yy), rgb)
    yield "palettes", c

    c = card("03 RIGS + ANIMATION")
    c.paste(Image.open(f"{O}/panels/t_rig.png").convert("RGB"), (0, 12))
    caption(c, "SKINS * CLIPS * MORPHS")
    yield "rigs", c

    c = card("04 SPRITE LIGHTS")
    paste_rgba(c, f"{O}/fox_cell.png", (16, 26))
    paste_rgba(c, f"{O}/normals/fox_cell_n.png", (96, 26))
    centered(c, 94, "SPRITE", DIM, x0=16, w=64)
    centered(c, 94, "NORMALS", DIM, x0=96, w=64)
    caption(c, "NORMAL * SPEC * AO * HEIGHT")
    yield "sprite_lights", c

    c = card("05 PIXEL SCALE")
    cell = Image.open(f"{O}/fox_cell.png").convert("RGBA")
    l, t, r, b = cell.getbbox()
    cx, cy, s = (l + r) // 2, (t + b) // 2, 22
    box = (cx - s // 2, cy - s // 2, cx - s // 2 + s, cy - s // 2 + s)
    near = cell.crop(box).resize((s * 3, s * 3), Image.NEAREST)
    sc3 = Image.open(f"{O}/scaled/fox_cell_3x.png").convert("RGBA").crop(tuple(v * 3 for v in box))
    c.paste(near, (14, 22), near)
    c.paste(sc3, (96, 22), sc3)
    centered(c, 94, "NEAREST", DIM, x0=14, w=66)
    centered(c, 94, "SCALE3X", CYAN, x0=96, w=66)
    caption(c, "NO NEW COLOURS")
    yield "pixel_scale", c

    c = card("06 BLOCK LEVELS")
    c.paste(Image.open(f"{O}/panels/t_blocks.png").convert("RGB"), (0, 12))
    caption(c, "BAKED AO + SUN SHADOWS")
    yield "blocks", c

    c = card("07 VRAM MAP")
    v = Image.open(f"{O}/vram.png").convert("RGB").crop((192, 0, 704, 256)).resize((168, 84), Image.LANCZOS)
    c.paste(v, (4, 20))
    caption(c, "TEXTURE PAGES * CLUTS")
    yield "vram", c

    c = card("08 GODOT 4 READY")
    files = [
        ((240, 150, 60), "FOX.GLB"),
        ((120, 200, 255), "FOX.TSCN"),
        ((200, 120, 255), "FOX.PNG + .IMPORT"),
        ((120, 230, 160), "PSX_OPAQUE.GDSHADER"),
        ((120, 230, 160), "PSX_COMMON.GDSHADERINC"),
        ((255, 220, 120), "PSX_SCREEN.GD"),
        ((150, 140, 200), "IMPORT_NOTES.TXT"),
    ]
    for k, (col, name) in enumerate(files):
        y = 20 + k * 12
        for yy in range(y, y + 7):
            for xx in range(6, 13):
                c.putpixel((xx, yy), col)
        draw_text(c, 18, y, name, WHITE)
    caption(c, "SHADERS ALREADY ASSIGNED", CYAN)
    yield "godot", c


if __name__ == "__main__":
    hero()
    before_after()
    sprites()
    for name, c in cards():
        crt(frame(c)).save(f"{OUT}/card_{name}.png")
    for f in sorted(os.listdir(OUT)):
        print(f, os.path.getsize(os.path.join(OUT, f)) // 1024, "KB")
