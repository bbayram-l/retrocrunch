# README art

The images in `docs/readme/` are made by Retrocrunch itself. To rebuild them (for example
after a release changes the look), from the repo root:

```powershell
cargo build --release
python tools/readme-art/fetch_polyhaven.py      # CC0 lion head + boombox -> test_assets/polyhaven
python tools/readme-art/render.py               # converts and renders into test_out/readme
python tools/readme-art/compose.py              # pixel font, CRT scanlines -> docs/readme
```

Also needs `test_assets/Fox.glb` from the Khronos glTF sample models, and Pillow. The
output is deterministic: the same build gives byte-identical images.
