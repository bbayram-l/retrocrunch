extends Node
## Demo controls and help text. The scene itself is made by build_demo.gd.

var screen: Control
var limiter: Node
var draw: Node
var heatmap: Node
var vram: Node
var lights: Node
var flashlight: SpotLight3D

var flat_depth := false
var fog := true
var accuracy := false
var show_help := false

var _materials: Array[ShaderMaterial] = []
var _help: Label
var _fog_end := -1.0


func _ready() -> void:
	# psx_screen.gd moves the 3D world into its low-resolution viewport before this runs.
	screen = $Screen
	limiter = find_child("SpeedLimit", true, false)
	draw = find_child("DrawDistance", true, false)
	heatmap = find_child("DetailHeatmap", true, false)
	vram = find_child("VramOverlay", true, false)
	lights = find_child("PS1Lights", true, false)
	flashlight = find_child("Flashlight", true, false)
	for mi: MeshInstance3D in screen.find_children("*", "MeshInstance3D", true, false):
		for s in mi.mesh.get_surface_count():
			var m := mi.get_active_material(s) as ShaderMaterial
			if m and not _materials.has(m):
				_materials.append(m)
	# Help on the root canvas, so it stays sharp at full resolution.
	var layer := CanvasLayer.new()
	layer.layer = 127
	_help = Label.new()
	_help.add_theme_color_override("font_outline_color", Color.BLACK)
	_help.add_theme_constant_override("outline_size", 4)
	_help.anchor_top = 1.0
	_help.anchor_bottom = 1.0
	_help.grow_vertical = Control.GROW_DIRECTION_BEGIN
	_help.offset_left = 8
	_help.offset_bottom = -8
	layer.add_child(_help)
	add_child(layer)


func _process(_delta: float) -> void:
	_follow_fog()
	if not show_help:
		_help.text = "F1: keys and info   (%d fps)" % Engine.get_frames_per_second()
		return
	var on := func(b: bool) -> String: return "on" if b else "off"
	_help.text = "\n".join([
		"Real frame rate: %d fps" % Engine.get_frames_per_second(),
		"Right mouse: look   WASD: move   Q/E: down/up   Shift: faster   F1: hide this",
		"P: 240-line PS1 screen (%s)   G: fog ending at the draw distance (%s)" % [
			"240 lines" if screen.lines == 240 else "full resolution", on.call(fog)],
		"1 / 2 / 3: draw distance Cut / Tint / Off (%s)   + / -: budget (%d)" % [
			["Cut", "Tint", "Off"][draw.mode], draw.budget],
		"L: PS1 speed limit (%s)   V: vsync steps (%s)" % [
			on.call(limiter.enabled), on.call(limiter.vsync_steps)],
		"F: flashlight (%s)   J: lights %s" % [
			on.call(flashlight.visible),
			"Authentic (3 per object, PS1)" if lights.mode == 1 else "Extended (8 per vertex)"],
		"K: PS1 sorting, flat depth (%s)   O: PS1 fog curve, dither rule, near drop (%s)" % [
			on.call(flat_depth), on.call(accuracy)],
		"H: near-detail heatmap (%s)   F3: VRAM overlay (%s)" % [
			on.call(heatmap.enabled), on.call(vram.visible)],
	])


## Silent Hill's trick: the fog ends where the draw distance cuts, so the cut is hidden.
func _follow_fog() -> void:
	var end := 80.0
	if draw.mode == 0 and draw.draw_distance < INF:
		end = clampf(draw.draw_distance, 15.0, 80.0)
	if not fog:
		end = -2.0
	if absf(end - _fog_end) < 0.5:
		return
	_fog_end = end
	for m in _materials:
		m.set_shader_parameter("fog_enabled", fog)
		if fog:
			m.set_shader_parameter("fog_end", end)
			m.set_shader_parameter("fog_start", end * 0.3)


func _set_all(param: String, value: Variant) -> void:
	for m in _materials:
		m.set_shader_parameter(param, value)


func _unhandled_input(event: InputEvent) -> void:
	if not (event is InputEventKey and event.pressed and not event.echo):
		return
	match event.keycode:
		KEY_F1:
			show_help = not show_help
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
		KEY_F:
			flashlight.visible = not flashlight.visible
		KEY_J:
			lights.mode = 1 - lights.mode
		KEY_P:
			screen.lines = 240 if screen.lines != 240 else int(screen.size.y)
		KEY_G:
			fog = not fog
		KEY_K:
			flat_depth = not flat_depth
			_set_all("flat_depth", flat_depth)
		KEY_O:
			accuracy = not accuracy
			_set_all("fog_ps1_curve", accuracy)
			_set_all("ps1_dither_rule", accuracy)
			_set_all("near_drop", 0.6 if accuracy else 0.0)
