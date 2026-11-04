"""Convert and render everything the README art needs into test_out/readme.

Run from the repo root after `cargo build --release` and fetch_polyhaven.py. Needs
test_assets/Fox.glb (Khronos glTF sample models). Then run compose.py.
"""
import json
import os
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

EXE = os.path.join("target", "release", "retrocrunch.exe" if os.name == "nt" else "retrocrunch")
O = "test_out/readme"
LION = "test_assets/polyhaven/lion_head/lion_head_2k.gltf"
BOOM = "test_assets/polyhaven/boombox/boombox_2k.gltf"
FOX = "test_assets/Fox.glb"
WALK_SECONDS = 0.7083  # length of the Fox's Walk clip


def rc(*args):
    subprocess.run([EXE, *map(str, args)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def preview(model, out, *extra):
    rc("preview", model, "-o", out, *extra)


def build_level():
    """The courtyard, built over MCP because only build_blocks bakes lighting."""
    level = json.load(open(os.path.join(os.path.dirname(__file__), "courtyard.json"), encoding="utf-8"))
    level.update(name="courtyard", output_dir=f"{O}/blocks", subdivide=2, light={})
    msgs = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "readme-art", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "build_blocks", "arguments": level}},
    ]
    out = subprocess.run([EXE, "mcp"], input="\n".join(json.dumps(m) for m in msgs) + "\n",
                         capture_output=True, text=True, check=True).stdout
    reply = json.loads(out.strip().splitlines()[-1])
    if reply.get("result", {}).get("isError"):
        raise SystemExit(f"build_blocks failed: {reply}")


def main():
    shutil.rmtree(O, ignore_errors=True)
    os.makedirs(f"{O}/hero", exist_ok=True)
    os.makedirs(f"{O}/panels", exist_ok=True)

    rc("model", LION, "-o", f"{O}/final", "--engine", "godot", "--tris", 900, "--colors", 32)
    rc("model", FOX, "-o", f"{O}/final", "--engine", "godot", "--tris", 500, "--colors", 16)
    rc("model", BOOM, "-o", f"{O}/conv", "--engine", "godot", "--tris", 400, "--colors", 16, "--tex-size", 128)
    rc("model", BOOM, "-o", f"{O}/pal_pico-8", "--engine", "generic", "--tris", 400,
       "--palette", "builtin:pico-8", "--palette-mode", "all")
    build_level()

    lion = f"{O}/final/lion_head_2k/lion_head_2k.glb"
    fox = f"{O}/final/Fox/Fox.glb"
    jobs = [(fox, f"{O}/hero/f{i:02d}.png", "--clip", "Walk", "--time", round((i % 8) / 8 * WALK_SECONDS, 4),
             "--yaw", i * 15 + 20, "--width", 170, "--height", 150) for i in range(24)]
    jobs += [
        (LION, f"{O}/panels/ba_src.png", "--shading", "pbr", "--yaw", 30, "--width", 136, "--height", 150),
        (lion, f"{O}/panels/ba_ps1.png", "--yaw", 30, "--width", 136, "--height", 150),
        (lion, f"{O}/panels/ba_wire.png", "--yaw", 30, "--wireframe", "--width", 136, "--height", 150),
        (f"{O}/conv/boombox_2k/boombox_2k.glb", f"{O}/panels/t_model.png", "--yaw", 30, "--width", 176, "--height", 118),
        (f"{O}/pal_pico-8/boombox_2k/boombox_2k.glb", f"{O}/panels/t_pal.png", "--yaw", 30, "--width", 176, "--height", 90),
        (f"{O}/blocks/courtyard.glb", f"{O}/panels/t_blocks.png", "--yaw", 35, "--width", 176, "--height", 118),
        (fox, f"{O}/panels/t_rig.png", "--clip", "Run", "--time", 0.35, "--yaw", 75, "--width", 176, "--height", 118),
    ]
    with ThreadPoolExecutor(8) as pool:
        list(pool.map(lambda j: preview(*j), jobs))

    rc("sprites", fox, "-o", f"{O}/sprites", "--directions", 8, "--size", 64, "--clip", "Walk", "--frames", 8)
    from PIL import Image
    Image.open(f"{O}/sprites/Fox_sprites.png").crop((128, 128, 192, 192)).save(f"{O}/fox_cell.png")
    rc("normals", f"{O}/fox_cell.png", "-o", f"{O}/normals")
    rc("scale", f"{O}/fox_cell.png", "-o", f"{O}/scaled", "-a", "scale3x")
    rc("vram", f"{O}/final/Fox/Fox.png", f"{O}/final/lion_head_2k/lion_head_2k.png",
       f"{O}/conv/boombox_2k/boombox_2k.png", f"{O}/blocks/courtyard.png",
       f"{O}/pal_pico-8/boombox_2k/boombox_2k.png", "--map", f"{O}/vram.png")
    with open(f"{O}/palettes.json", "w", encoding="utf-8") as f:
        f.write(subprocess.run([EXE, "palettes", "--json"], capture_output=True, text=True, check=True).stdout)
    print("renders in", O)


if __name__ == "__main__":
    sys.exit(main())
