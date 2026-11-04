"""Tiny 5x7 pixel font and retro image helpers for the README art."""
from PIL import Image

G = {
    "A": ".###.|#...#|#...#|#####|#...#|#...#|#...#",
    "B": "####.|#...#|#...#|####.|#...#|#...#|####.",
    "C": ".###.|#...#|#....|#....|#....|#...#|.###.",
    "D": "####.|#...#|#...#|#...#|#...#|#...#|####.",
    "E": "#####|#....|#....|####.|#....|#....|#####",
    "F": "#####|#....|#....|####.|#....|#....|#....",
    "G": ".###.|#...#|#....|#.###|#...#|#...#|.####",
    "H": "#...#|#...#|#...#|#####|#...#|#...#|#...#",
    "I": ".###.|..#..|..#..|..#..|..#..|..#..|.###.",
    "J": "..###|...#.|...#.|...#.|...#.|#..#.|.##..",
    "K": "#...#|#..#.|#.#..|##...|#.#..|#..#.|#...#",
    "L": "#....|#....|#....|#....|#....|#....|#####",
    "M": "#...#|##.##|#.#.#|#.#.#|#...#|#...#|#...#",
    "N": "#...#|#...#|##..#|#.#.#|#..##|#...#|#...#",
    "O": ".###.|#...#|#...#|#...#|#...#|#...#|.###.",
    "P": "####.|#...#|#...#|####.|#....|#....|#....",
    "Q": ".###.|#...#|#...#|#...#|#.#.#|#..#.|.##.#",
    "R": "####.|#...#|#...#|####.|#.#..|#..#.|#...#",
    "S": ".####|#....|#....|.###.|....#|....#|####.",
    "T": "#####|..#..|..#..|..#..|..#..|..#..|..#..",
    "U": "#...#|#...#|#...#|#...#|#...#|#...#|.###.",
    "V": "#...#|#...#|#...#|#...#|#...#|.#.#.|..#..",
    "W": "#...#|#...#|#...#|#.#.#|#.#.#|#.#.#|.#.#.",
    "X": "#...#|#...#|.#.#.|..#..|.#.#.|#...#|#...#",
    "Y": "#...#|#...#|.#.#.|..#..|..#..|..#..|..#..",
    "Z": "#####|....#|...#.|..#..|.#...|#....|#####",
    "0": ".###.|#...#|#..##|#.#.#|##..#|#...#|.###.",
    "1": "..#..|.##..|..#..|..#..|..#..|..#..|.###.",
    "2": ".###.|#...#|....#|...#.|..#..|.#...|#####",
    "3": "####.|....#|....#|.###.|....#|....#|####.",
    "4": "...#.|..##.|.#.#.|#..#.|#####|...#.|...#.",
    "5": "#####|#....|####.|....#|....#|#...#|.###.",
    "6": "..##.|.#...|#....|####.|#...#|#...#|.###.",
    "7": "#####|....#|...#.|..#..|.#...|.#...|.#...",
    "8": ".###.|#...#|#...#|.###.|#...#|#...#|.###.",
    "9": ".###.|#...#|#...#|.####|....#|...#.|.##..",
    " ": ".....|.....|.....|.....|.....|.....|.....",
    ".": ".....|.....|.....|.....|.....|.....|..#..",
    ",": ".....|.....|.....|.....|..#..|..#..|.#...",
    "-": ".....|.....|.....|.###.|.....|.....|.....",
    ":": ".....|..#..|.....|.....|.....|..#..|.....",
    "/": "....#|...#.|...#.|..#..|.#...|.#...|#....",
    ">": ".#...|..#..|...#.|....#|...#.|..#..|.#...",
    "+": ".....|..#..|..#..|#####|..#..|..#..|.....",
    "%": "##..#|##..#|...#.|..#..|.#...|#..##|#..##",
    "*": ".....|.....|.....|..#..|.....|.....|.....",  # middle dot
    "!": "..#..|..#..|..#..|..#..|..#..|.....|..#..",
    "_": ".....|.....|.....|.....|.....|.....|#####",
    "(": "...#.|..#..|.#...|.#...|.#...|..#..|...#.",
    ")": ".#...|..#..|...#.|...#.|...#.|..#..|.#...",
}


def text_width(s, scale=1):
    return (len(s) * 6 - 1) * scale if s else 0


def draw_text(img, x, y, s, color, scale=1, shadow=(10, 8, 24), gradient=None):
    """Draw upper-case text. gradient: (top_rgb, bottom_rgb) per glyph row."""
    px = img.load()
    for layer in ((1, shadow), (0, None)) if shadow else ((0, None),):
        off, sc = layer
        cx = x
        for ch in s.upper():
            rows = G.get(ch, G[" "]).split("|")
            for gy, row in enumerate(rows):
                if gradient and sc is None:
                    t = gy / 6
                    c = tuple(int(a + (b - a) * t) for a, b in zip(*gradient))
                else:
                    c = sc or color
                for gx, bit in enumerate(row):
                    if bit != "#":
                        continue
                    for dy in range(scale):
                        for dx in range(scale):
                            X = cx + gx * scale + dx + off * scale
                            Y = y + gy * scale + dy + off * scale
                            if 0 <= X < img.width and 0 <= Y < img.height:
                                px[X, Y] = c + ((255,) if img.mode == "RGBA" else ())
            cx += 6 * scale
    return cx


def background(w, h, preview_path):
    """Vertical gradient taken from a preview render's left column (same height)."""
    p = Image.open(preview_path).convert("RGB")
    col = [p.getpixel((0, int(y * p.height / h))) for y in range(h)]
    bg = Image.new("RGB", (w, h))
    px = bg.load()
    for y in range(h):
        for x in range(w):
            px[x, y] = col[y]
    return bg


def crt(img, scale=2, scan=0.78):
    """Nearest-neighbour upscale with darker scanline rows (the CRT look)."""
    big = img.convert("RGB").resize((img.width * scale, img.height * scale), Image.NEAREST)
    px = big.load()
    for y in range(scale - 1, big.height, scale):
        for x in range(big.width):
            r, g, b = px[x, y]
            px[x, y] = (int(r * scan), int(g * scan), int(b * scan))
    return big


def frame(img, color=(90, 80, 150)):
    """1 px border."""
    px = img.load()
    for x in range(img.width):
        px[x, 0] = color
        px[x, img.height - 1] = color
    for y in range(img.height):
        px[0, y] = color
        px[img.width - 1, y] = color
    return img


def save_gif(frames, path, ms):
    pal = [f.convert("P", palette=Image.ADAPTIVE, colors=255, dither=Image.NONE) for f in frames]
    pal[0].save(path, save_all=True, append_images=pal[1:], duration=ms, loop=0, optimize=True, disposal=1)
