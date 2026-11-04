"""Download the CC0 Poly Haven models used for the README art into test_assets/polyhaven.

Usage (from the repo root): python tools/readme-art/fetch_polyhaven.py lion_head boombox
"""
import json
import os
import sys
import urllib.request

UA = {"User-Agent": "retrocrunch-readme"}
DEST = os.path.join("test_assets", "polyhaven")


def get(url):
    return urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60).read()


for aid in sys.argv[1:] or ["lion_head", "boombox"]:
    files = json.loads(get(f"https://api.polyhaven.com/files/{aid}").decode("utf-8"))
    g = files["gltf"]["2k"]["gltf"]
    out = os.path.join(DEST, aid)
    os.makedirs(out, exist_ok=True)
    open(os.path.join(out, os.path.basename(g["url"])), "wb").write(get(g["url"]))
    for rel, inc in g.get("include", {}).items():
        p = os.path.normpath(os.path.join(out, rel))
        if not p.startswith(os.path.normpath(out) + os.sep):
            raise SystemExit(f"refusing path outside {out}: {rel}")
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "wb").write(get(inc["url"]))
    print(aid, "downloaded to", out)
