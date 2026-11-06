extends Node
## retrocrunch PS1 speed limit. Caps the frame rate to what a PlayStation could draw:
## every frame it reads how many triangles Godot drew and works out how long the PS1 GPU
## would have needed (Sony's figures: 180 textured, Gouraud-lit polygons per millisecond,
## 360 flat-shaded). Over budget, the game slows down like it would on the console.
##
## With vsync steps on (the default) a frame that misses the 60 Hz refresh waits for the
## next one, as on real hardware: up to 3000 polygons run at 60 fps, up to 6000 at 30,
## up to 9000 at 20, and so on. With them off the rate drops smoothly (180000 / polygons).
##
## Usage: add a Node with this script anywhere inside your 3D scene. Under the psx_screen.gd
## screen pass that means inside its viewport, so it counts the low-resolution render.
##
## Approximate: Godot counts triangles (a PS1 quad was one polygon, so quad-modelled scenes
## read up to twice as heavy) and culls whole objects rather than single back faces.
## Shadow passes are not counted.

enum Shading { TEXTURED_GOURAUD, FLAT }

@export var enabled := true
## Which PS1 drawing rate applies: textured + Gouraud (180/ms) or flat-shaded (360/ms).
@export var shading := Shading.TEXTURED_GOURAUD
## Override the rate (polygons per millisecond). 0 = from shading.
@export var polys_per_ms := 0
## TV refresh rate: 60 (NTSC) or 50 (PAL).
@export_enum("NTSC 60 Hz:60", "PAL 50 Hz:50") var refresh := 60
## Wait for the next refresh like real hardware (60/30/20 fps steps). Off = smooth.
@export var vsync_steps := true
## Draw the polygon count and capped frame rate in the corner.
@export var show_stats := false
## Viewport whose 3D is counted. Empty = the viewport this node is in.
@export var viewport_path: NodePath

## Triangles drawn last frame and the frame rate cap they give.
var polygons := 0
var fps_cap := 0

var _old_max_fps := 0
var _label: Label


func _ready() -> void:
	_old_max_fps = Engine.max_fps
	if show_stats:
		var layer := CanvasLayer.new()
		layer.layer = 128
		_label = Label.new()
		_label.position = Vector2(8, 8)
		_label.add_theme_color_override("font_outline_color", Color.BLACK)
		_label.add_theme_constant_override("outline_size", 4)
		layer.add_child(_label)
		add_child(layer)


func _exit_tree() -> void:
	Engine.max_fps = _old_max_fps


func _process(_delta: float) -> void:
	var vp: Viewport = get_viewport()
	if not viewport_path.is_empty():
		vp = get_node(viewport_path) as Viewport
	polygons = vp.get_render_info(
		Viewport.RENDER_INFO_TYPE_VISIBLE, Viewport.RENDER_INFO_PRIMITIVES_IN_FRAME
	)
	fps_cap = cap_for(polygons)
	Engine.max_fps = fps_cap if enabled else _old_max_fps
	if _label:
		_label.text = "PS1: %d polys -> %d fps (%d%% of a frame)" % [
			polygons, fps_cap, roundi(100.0 * polygons * refresh / per_second())
		]


## Polygons the PS1 could draw in one second.
func per_second() -> int:
	var rate := polys_per_ms
	if rate <= 0:
		rate = 180 if shading == Shading.TEXTURED_GOURAUD else 360
	return rate * 1000


## Frame rate cap for `polys` polygons per frame.
@warning_ignore("integer_division")
func cap_for(polys: int) -> int:
	var rate := per_second()
	if vsync_steps:
		# Whole refreshes the frame needs, rounded up (integer maths: 3000 is exactly one).
		var frames := maxi(1, (polys * refresh + rate - 1) / rate)
		return maxi(1, refresh / frames)
	return clampi(rate / maxi(polys, 1), 1, refresh)
