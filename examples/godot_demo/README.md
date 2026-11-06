# Retrocrunch PS1 budget demo (Godot 4)

A street at night that is far over a PS1 polygon budget: 50 buildings, 48 trees of about
1000 triangles each, and one very detailed monument 300 m away. Use it to try the three
Godot tools before adding them to your own project.

Open this folder in Godot 4 (tested with 4.7.2) and press Play (F5).

## Controls

| Key | Does |
|---|---|
| Right mouse (hold) | look around |
| W A S D / Q E | move / down, up (Shift: faster) |
| 1 / 2 / 3 | draw distance: Cut / Tint / Off |
| + / - | draw distance budget, in steps of 1000 |
| L | PS1 speed limit on/off |
| V | speed limit: wait for vsync (60/30/20 steps) or slow down smoothly |

Tint draws every cut object a second time, so it turns the speed limit off.

## What to look for

- **PS1 speed limit** (`SpeedLimit` node, `psx_speed_limit.gd`): the top line shows the
  triangles drawn and the frame rate a PS1 would manage. Press 3 to draw the whole
  street: about 69,000 triangles in view, and the game drops to 2 fps.
- **Draw distance** (`DrawDistance` node, `psx_draw_distance.gd`): the second line shows
  how far you can see within the budget (6000 triangles: about 37 m at the start). Cut
  hides everything farther, like Silent Hill without its fog; Tint paints it red. Walk
  down the street and watch the distance change.
- **PS1 Budget dock** (editor add-on, enabled already): open `demo.tscn`, find the PS1
  Budget tab next to the Inspector, tick Scene camera and press Analyze. It lists every
  object with its polygons and texture memory, and says how far the view can draw to fit.
  Try the settings: budget, VRAM size, framebuffers and resolution.

The dock counts polygons the PS1 way (a flat quad counts once), while the two runtime
nodes count triangles, as Godot draws them. These boxy meshes pair into many quads, so
the dock's distance (about 62 m) is longer than the node's (37 m).

## Files

- `demo.tscn`, made by `build_demo.gd` (rebuild: `godot --headless --path . -s build_demo.gd`)
- `demo.gd`: the keys and help text; `fly_camera.gd`: the camera
- `psx_speed_limit.gd`, `psx_draw_distance.gd` and `addons/retrocrunch_budget`: copies of
  the files in Retrocrunch's `godot/` folder (a test keeps them identical)

Everything here uses Godot's built-in meshes and generated textures, so there are no
third-party assets. MIT No Attribution, like the rest of the Godot files.
