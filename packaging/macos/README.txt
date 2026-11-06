Retrocrunch @VERSION@ for macOS (Apple Silicon)
===============================================

Turn modern models, textures and sprites into PS1-era game assets.

Start
-----
Drag Retrocrunch.app wherever you like, e.g. Applications.

The app isn't signed with an Apple developer certificate, so the first time macOS
won't open it. Either right-click Retrocrunch.app > Open > Open, or run once in
Terminal:

  xattr -dr com.apple.quarantine Retrocrunch.app

Your settings and saved presets are stored in ~/.config/retrocrunch.

Command line
------------
The command-line tool is inside the app:

  Retrocrunch.app/Contents/MacOS/retrocrunch model zombie.fbx -o out --engine godot
  Retrocrunch.app/Contents/MacOS/retrocrunch --help

Add --json to any command for machine-readable output.

AI coding agents (MCP)
----------------------
  claude mcp add retrocrunch -- /Applications/Retrocrunch.app/Contents/MacOS/retrocrunch mcp

AGENTS.md explains the tools and the recommended convert / preview loop.

Blender (optional)
------------------
With Blender installed, Retrocrunch also imports .blend, USD, Alembic, STL and PLY,
exports FBX and can remesh. It finds Blender in Applications automatically. The
Blender add-on is in blender/retrocrunch_addon.py (Edit > Preferences > Add-ons >
Install from Disk).

Godot (optional)
----------------
Godot exports include psx_speed_limit.gd: add it as a Node in your 3D scene and it caps
the frame rate to what a PS1 could draw (60 fps up to 3000 polygons, 30 up to 6000).
psx_draw_distance.gd (also a Node) cuts or tints everything beyond the distance where
the view would pass a polygon budget, to see how far each scene can draw.
psx_vram_overlay.gd shows PS1 video memory in the corner (F3), and
psx_detail_heatmap.gd colours near-detail and LOD versions by their triangles.
For whole scenes, copy godot/addons/retrocrunch_budget into your project's addons
folder and enable "Retrocrunch PS1 Budget" under Project Settings > Plugins.

Files in this folder
--------------------
  Retrocrunch.app              the app, with the command-line tool inside
  GUIDE.md                     the full guide
  AGENTS.md                    the MCP server and agent workflow
  CHANGELOG.md                 what changed in each version
  blender/                     the Blender add-on (MIT No Attribution)
  godot/                       Godot scripts, shaders and the PS1 Budget add-on (MIT No Attribution)
  LICENSE.md                   Retrocrunch's licence
  THIRD_PARTY_LICENSES.html    licences of the open-source libraries inside

Licence
-------
Everything Retrocrunch creates for you is yours to use in any project, commercial games
included, without attribution. Retrocrunch itself is source-available under the PolyForm
Shield 1.0.0 licence: you may use it for anything, but not resell or redistribute it.
See LICENSE.md.

The app icon is the glTF sample Fox, converted by Retrocrunch: model by PixelMannen
(CC0), rigging and animation by tomkranis (CC BY 4.0).

Copyright (c) 2026 Burak Bayram
