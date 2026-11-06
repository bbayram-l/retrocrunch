extends Node
## retrocrunch detail heatmap (debug tool): colours the near-detail and LOD versions of
## retrocrunch models by how many triangles they have against the converted model, so you
## can see where tessellation spends triangles up close and where LODs save them far away.
##
## Yellow to red: tessellated near versions (yellow = 2x the triangles, red = 8x or more).
## Blue: LOD versions with fewer triangles. The converted model itself isn't coloured.
## Godot shows each version only within its distance band, so the colours change as the
## camera moves.
##
## Usage: export a static model with near detail (`tessellate`) and/or `lods`, add a Node
## with this script anywhere in the scene. It reads `retrocrunch_tri_ratio` metadata that
## the exported .tscn puts on each version, and colours through the RenderingServer, so
## the nodes' own materials are never changed. Don't combine with psx_draw_distance.gd's
## Tint mode: both colour objects the same way.

@export var enabled := true:
	set(value):
		enabled = value
		if is_inside_tree():
			_refresh()
## How strongly the colour covers the surface.
@export_range(0.0, 1.0) var strength := 0.55:
	set(value):
		strength = value
		if is_inside_tree():
			_refresh()
## Draw a legend in the corner.
@export var show_legend := true

var _tinted := {}  # instance RID -> MeshInstance3D
var _materials := {}  # colour key -> ShaderMaterial
var _legend: Label


func _ready() -> void:
	get_tree().node_added.connect(_on_node_added)
	if show_legend:
		var layer := CanvasLayer.new()
		layer.layer = 128
		_legend = Label.new()
		_legend.text = "Detail heatmap: yellow 2x, orange 4x, red 8x+ triangles; blue fewer (LOD)"
		_legend.add_theme_color_override("font_outline_color", Color.BLACK)
		_legend.add_theme_constant_override("outline_size", 4)
		_legend.anchor_left = 1.0
		_legend.anchor_right = 1.0
		_legend.offset_left = -560
		_legend.offset_top = 8
		layer.add_child(_legend)
		add_child(layer)
	_refresh()


func _exit_tree() -> void:
	_clear()


## New models (a level streamed in) get coloured too.
func _on_node_added(node: Node) -> void:
	if node is MeshInstance3D and node.has_meta("retrocrunch_tri_ratio"):
		_refresh.call_deferred()


## Colour for a triangle ratio, or transparent for the converted model itself.
static func color_for(ratio: float) -> Color:
	if is_equal_approx(ratio, 1.0):
		return Color(0, 0, 0, 0)
	if ratio < 1.0:
		return Color(0.2, 0.45, 1.0)
	# 2x yellow, 4x orange, 8x and more red.
	var t := clampf((log(ratio) / log(2.0) - 1.0) / 2.0, 0.0, 1.0)
	return Color(1.0, 0.9, 0.1).lerp(Color(1.0, 0.1, 0.05), t)


func _refresh() -> void:
	_clear()
	if _legend:
		_legend.visible = enabled
	if not enabled:
		return
	for mi: MeshInstance3D in get_tree().root.find_children("*", "MeshInstance3D", true, false):
		if not mi.has_meta("retrocrunch_tri_ratio"):
			continue
		var c := color_for(float(mi.get_meta("retrocrunch_tri_ratio")))
		if c.a == 0.0:
			continue
		c.a = strength
		var rid := mi.get_instance()
		RenderingServer.instance_geometry_set_material_overlay(rid, _material(c).get_rid())
		_tinted[rid] = mi


func _clear() -> void:
	for rid: RID in _tinted:
		var mi: Variant = _tinted[rid]
		if is_instance_valid(mi):
			var own: Material = mi.material_overlay
			RenderingServer.instance_geometry_set_material_overlay(rid, own.get_rid() if own else RID())
	_tinted.clear()


func _material(c: Color) -> ShaderMaterial:
	var key := c.to_html()
	if not _materials.has(key):
		var shader := Shader.new()
		shader.code = "shader_type spatial;\nrender_mode unshaded, blend_mix, depth_draw_never;\nuniform vec4 tint : source_color;\nvoid fragment() { ALBEDO = tint.rgb; ALPHA = tint.a; }\n"
		var m := ShaderMaterial.new()
		m.shader = shader
		m.set_shader_parameter("tint", c)
		_materials[key] = m
	return _materials[key]
