extends Node
## retrocrunch draw distance (debug tool): finds how far the camera can see before the
## objects in view go over a PS1 polygon budget, then cuts everything farther away, like
## Silent Hill without its fog. Use it to see how far each scene can draw and where the
## art can be trimmed.
##
## Every frame it sorts the meshes in view by distance and keeps adding them, nearest
## first, until the next one would pass the budget: that one's distance is the draw
## distance. Farther objects are hidden (mode Cut) or tinted (mode Tint) through the
## RenderingServer, so the nodes' own `visible` and materials are never changed.
##
## Usage: add a Node with this script anywhere inside your 3D scene. Works per object,
## not per triangle: a big mesh that crosses the line counts whole. Counts triangles, as
## psx_speed_limit.gd does.

enum Mode { CUT, TINT, OFF }

## Cut hides objects past the draw distance, Tint colours them, Off only measures.
@export var mode := Mode.CUT
## Triangles allowed in view: 3000 is a 60 fps PS1 scene, 6000 a 30 fps one.
@export var budget := 6000
## Tint for objects past the draw distance (alpha = strength).
@export var tint := Color(1.0, 0.1, 0.1, 0.6)
## Draw the draw distance and counts in the corner.
@export var show_stats := false

## Results of the last frame.
var draw_distance := INF
var polygons_in_view := 0
var polygons_drawn := 0
var objects_cut := 0

var _ui_layer: CanvasLayer
var _tri_cache := {}
var _affected := {}  # instance RID -> MeshInstance3D, for objects currently cut or tinted
var _applied_mode := Mode.OFF
var _tint_material: ShaderMaterial
var _label: Label


func _ready() -> void:
	var shader := Shader.new()
	shader.code = "shader_type spatial;\nrender_mode unshaded, blend_mix, depth_draw_never, cull_disabled;\nuniform vec4 tint : source_color;\nvoid fragment() { ALBEDO = tint.rgb; ALPHA = tint.a; }\n"
	_tint_material = ShaderMaterial.new()
	_tint_material.shader = shader
	if show_stats:
		var layer := CanvasLayer.new()
		layer.layer = 128
		_label = Label.new()
		_label.position = Vector2(8, 34)
		_label.add_theme_color_override("font_outline_color", Color.BLACK)
		_label.add_theme_constant_override("outline_size", 4)
		layer.add_child(_label)
		# On the root canvas: stays sharp under psx_screen.gd's low-resolution pass.
		_ui_layer = layer
		get_tree().root.add_child.call_deferred(layer)


func _exit_tree() -> void:
	_restore_all()


func _process(_delta: float) -> void:
	_tint_material.set_shader_parameter("tint", tint)
	var cam := get_viewport().get_camera_3d()
	if cam == null:
		return
	var planes := cam.get_frustum()
	var eye := cam.global_position
	var in_view: Array = []
	for mi: MeshInstance3D in get_tree().root.find_children("*", "MeshInstance3D", true, false):
		if mi.mesh == null or not mi.is_visible_in_tree():
			continue
		var box: AABB = mi.global_transform * mi.get_aabb()
		if not _in_frustum(box, planes) or not in_visibility_range(mi, box, eye):
			continue
		in_view.append([_nearest_distance(box, eye), mi])
	in_view.sort_custom(func(a, b): return a[0] < b[0])

	polygons_in_view = 0
	polygons_drawn = 0
	objects_cut = 0
	draw_distance = INF
	var keep := {}
	for entry in in_view:
		var mi: MeshInstance3D = entry[1]
		var tris := _triangles(mi.mesh)
		polygons_in_view += tris
		if draw_distance == INF and polygons_drawn + tris <= budget:
			polygons_drawn += tris
			keep[mi.get_instance()] = true
		else:
			if draw_distance == INF:
				draw_distance = entry[0]
			objects_cut += 1
	_apply(in_view, keep)
	if _label:
		var dist := "unlimited" if draw_distance == INF else "%.1f m" % draw_distance
		_label.text = "Draw distance %s: %d of %d polys in view, %d objects cut" % [
			dist, polygons_drawn, polygons_in_view, objects_cut
		]


## Cut or tint everything in view that isn't kept; undo it for objects that are kept
## again or left the view.
func _apply(in_view: Array, keep: Dictionary) -> void:
	# A cut object stays cut in Tint mode unless it's restored first.
	if mode != _applied_mode:
		_restore_all()
		_applied_mode = mode
	var now := {}
	if mode != Mode.OFF:
		for entry in in_view:
			var rid: RID = entry[1].get_instance()
			if keep.has(rid):
				continue
			now[rid] = entry[1]
			if mode == Mode.CUT:
				RenderingServer.instance_set_visible(rid, false)
			else:
				RenderingServer.instance_geometry_set_material_overlay(rid, _tint_material.get_rid())
	for rid: RID in _affected:
		if not now.has(rid):
			_restore(_affected[rid])
	_affected = now


## Put back what the node itself asks for: its visibility and its own overlay material.
func _restore(node: Variant) -> void:
	# A freed node took its instance with it: nothing to restore.
	if not is_instance_valid(node):
		return
	var mi := node as MeshInstance3D
	var rid := mi.get_instance()
	RenderingServer.instance_set_visible(rid, mi.is_visible_in_tree())
	var overlay := mi.material_overlay
	RenderingServer.instance_geometry_set_material_overlay(
		rid, overlay.get_rid() if overlay else RID()
	)


func _restore_all() -> void:
	for rid: RID in _affected:
		_restore(_affected[rid])
	_affected.clear()


@warning_ignore("integer_division")
func _triangles(mesh: Mesh) -> int:
	if _tri_cache.has(mesh):
		return _tri_cache[mesh]
	var tris := 0
	for s in mesh.get_surface_count():
		var arrays := mesh.surface_get_arrays(s)
		var idx: PackedInt32Array = arrays[Mesh.ARRAY_INDEX]
		var verts: PackedVector3Array = arrays[Mesh.ARRAY_VERTEX]
		tris += (idx.size() if not idx.is_empty() else verts.size()) / 3
	_tri_cache[mesh] = tris
	return tris


## Distance from the eye to the nearest point of the box (0 inside it).
## False when the object's visibility range (LOD and near-detail bands) hides it from
## `eye`. Godot measures that range to the centre of the object's bounding box.
static func in_visibility_range(gi: GeometryInstance3D, box: AABB, eye: Vector3) -> bool:
	var d := eye.distance_to(box.get_center())
	if gi.visibility_range_begin > 0.0 and d < gi.visibility_range_begin:
		return false
	return gi.visibility_range_end <= 0.0 or d < gi.visibility_range_end


static func _nearest_distance(box: AABB, eye: Vector3) -> float:
	return eye.distance_to(eye.clamp(box.position, box.end))


static func _in_frustum(box: AABB, planes: Array[Plane]) -> bool:
	for p in planes:
		# Frustum planes face outward: the box is outside when even its innermost corner is.
		var corner := Vector3(
			box.position.x if p.normal.x > 0 else box.end.x,
			box.position.y if p.normal.y > 0 else box.end.y,
			box.position.z if p.normal.z > 0 else box.end.z,
		)
		if p.is_point_over(corner):
			return false
	return true


## The overlay lives on the root canvas, not under this node: free it with the node. Not on
## _exit_tree, which also runs when psx_screen.gd moves the scene into its viewport.
func _notification(what: int) -> void:
	if what == NOTIFICATION_PREDELETE and is_instance_valid(_ui_layer):
		_ui_layer.queue_free()
