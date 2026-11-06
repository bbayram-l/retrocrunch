Retrocrunch @VERSION@ for Windows
===============================

Turn modern models, textures and sprites into PS1-era game assets.

Start
-----
Double-click retrocrunch.exe. No installer is needed; keep the folder wherever you like.
Your settings and saved presets are stored in %APPDATA%\retrocrunch.

Command line
------------
  retrocrunch model zombie.fbx -o out --preset "Survival horror character" --engine godot
  retrocrunch texture *.png -o out --tex-size 64 --colors 16 --dither ps1
  retrocrunch --help

Add --json to any command for machine-readable output.

AI coding agents (MCP)
----------------------
  claude mcp add retrocrunch -- C:\path\to\retrocrunch.exe mcp

AGENTS.md explains the tools and the recommended convert / preview loop.

Blender (optional)
------------------
With Blender installed, Retrocrunch also imports .blend, USD, Alembic, STL and PLY,
exports FBX and can remesh. It finds Blender automatically. The Blender add-on is in
blender\retrocrunch_addon.py (Edit > Preferences > Add-ons > Install from Disk).

Godot (optional)
----------------
Godot exports include psx_speed_limit.gd: add it as a Node in your 3D scene and it caps
the frame rate to what a PS1 could draw (60 fps up to 3000 polygons, 30 up to 6000).
psx_draw_distance.gd (also a Node) cuts or tints everything beyond the distance where
the view would pass a polygon budget, to see how far each scene can draw.
For whole scenes, copy godot\addons\retrocrunch_budget into your project's addons
folder and enable "Retrocrunch PS1 Budget" under Project Settings > Plugins. Its dock
measures polygons in view and texture VRAM and suggests what to clip.

Files in this folder
--------------------
  retrocrunch.exe              the app and the command-line tool
  GUIDE.md                     the full guide
  AGENTS.md                    the MCP server and agent workflow
  CHANGELOG.md                 what changed in each version
  blender\                     the Blender add-on (MIT No Attribution)
  godot\                       Godot scripts, shaders and the PS1 Budget add-on (MIT No Attribution)
  LICENSE.md                   Retrocrunch's licence
  THIRD_PARTY_LICENSES.html    licences of the open-source libraries inside

Licence
-------
Everything Retrocrunch creates for you is yours to use in any project, commercial games
included, without attribution. Retrocrunch itself is source-available under the PolyForm
Shield 1.0.0 licence: you may use it for anything, but not resell or redistribute it.
See LICENSE.md.

Copyright (c) 2026 Burak Bayram
