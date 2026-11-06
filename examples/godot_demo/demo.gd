extends Node3D
## Demo controls and help text. The scene itself is made by build_demo.gd.

@onready var limiter: Node = $SpeedLimit
@onready var draw: Node = $DrawDistance
@onready var heatmap: Node = $DetailHeatmap
@onready var vram: Node = $VramOverlay

var _help: Label


func _ready() -> void:
	var layer := CanvasLayer.new()
	layer.layer = 127
	_help = Label.new()
	_help.add_theme_color_override("font_outline_color", Color.BLACK)
	_help.add_theme_constant_override("outline_size", 4)
	_help.anchor_top = 1.0
	_help.anchor_bottom = 1.0
	_help.offset_left = 8
	_help.offset_top = -190
	layer.add_child(_help)
	add_child(layer)


func _process(_delta: float) -> void:
	var modes := ["Cut", "Tint", "Off"]
	_help.text = "\n".join([
		"Real frame rate: %d fps" % Engine.get_frames_per_second(),
		"Right mouse: look   WASD: move   Q/E: down/up   Shift: faster",
		"1 / 2 / 3: draw distance Cut / Tint / Off   (now: %s; Tint turns the speed limit off)" % modes[draw.mode],
		"+ / -: draw distance budget (now: %d)" % draw.budget,
		"L: PS1 speed limit (now: %s)   V: vsync steps (now: %s)" % [
			"on" if limiter.enabled else "off", "on" if limiter.vsync_steps else "off"
		],
		"H: near-detail heatmap on the road (now: %s)   F3: VRAM overlay (now: %s)" % [
			"on" if heatmap.enabled else "off", "on" if vram.visible else "off"
		],
	])


func _unhandled_input(event: InputEvent) -> void:
	if not (event is InputEventKey and event.pressed and not event.echo):
		return
	match event.keycode:
		KEY_1:
			draw.mode = 0
		KEY_2:
			draw.mode = 1
			# Tint draws every cut object a second time, which the speed limit counts,
			# and colours objects the same way as the heatmap.
			limiter.enabled = false
			heatmap.enabled = false
		KEY_3:
			draw.mode = 2
		KEY_EQUAL, KEY_KP_ADD:
			draw.budget += 1000
		KEY_MINUS, KEY_KP_SUBTRACT:
			draw.budget = maxi(1000, draw.budget - 1000)
		KEY_L:
			limiter.enabled = not limiter.enabled
		KEY_V:
			limiter.vsync_steps = not limiter.vsync_steps
		KEY_H:
			heatmap.enabled = not heatmap.enabled
			if heatmap.enabled and draw.mode == 1:
				draw.mode = 0
