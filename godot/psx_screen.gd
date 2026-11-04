extends Control
## retrocrunch screen pass. Renders this node's 3D children at a PS1 resolution (240 lines
## by default, width from the window's aspect) and scales the result up with nearest
## filtering. Dither and 15-bit colour run here, once per virtual pixel, so retrocrunch
## materials below it get theirs switched off.
##
## Usage: add a Control with this script to your main scene and put the 3D level (camera
## included) under it. UI Controls placed under it stay at full resolution on top.

## Virtual screen height in pixels (240 = PS1, 480 = interlaced hi-res).
@export var lines := 240:
	set(value):
		lines = maxi(value, 16)
		_fit()
@export var dither := true
## Levels per channel: 32 = 15-bit RGB555, 256 = off.
@export var color_levels := 32.0
## Experimental CRT look: scanlines, composite colour bleed, aperture grille.
@export var crt := false
## Switch off per-pixel dither/colour depth in retrocrunch materials under this node, so
## they aren't applied twice.
@export var take_over_materials := true

var viewport: SubViewport
var screen: TextureRect
var _post: ShaderMaterial


func _ready() -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	viewport = SubViewport.new()
	viewport.name = "PSXViewport"
	viewport.render_target_update_mode = SubViewport.UPDATE_ALWAYS
	viewport.msaa_3d = Viewport.MSAA_DISABLED
	viewport.screen_space_aa = Viewport.SCREEN_SPACE_AA_DISABLED
	viewport.use_taa = false
	viewport.audio_listener_enable_3d = true
	viewport.physics_object_picking = true
	add_child(viewport)
	for child in get_children():
		_adopt(child)
	# Levels spawned later (change_scene, add_child at runtime) move in too.
	child_entered_tree.connect(func(child: Node) -> void: _adopt.call_deferred(child))

	_post = ShaderMaterial.new()
	_post.shader = load(get_script().resource_path.get_base_dir().path_join("psx_post.gdshader"))
	screen = TextureRect.new()
	screen.name = "PSXScreen"
	screen.texture = viewport.get_texture()
	screen.texture_filter = CanvasItem.TEXTURE_FILTER_NEAREST
	screen.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	screen.stretch_mode = TextureRect.STRETCH_SCALE
	screen.mouse_filter = Control.MOUSE_FILTER_IGNORE
	screen.material = _post
	add_child(screen)
	move_child(screen, 0)
	screen.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)

	resized.connect(_fit)
	_fit()
	if take_over_materials:
		_take_over(viewport)
		get_tree().node_added.connect(_on_node_added)


func _process(_delta: float) -> void:
	_post.set_shader_parameter("dither", dither)
	_post.set_shader_parameter("color_levels", color_levels)
	_post.set_shader_parameter("crt", crt)


func _adopt(child: Node) -> void:
	if not is_instance_valid(child) or child.get_parent() != self:
		return
	if child != viewport and child != screen and not child is Control:
		child.reparent(viewport, false)


func _fit() -> void:
	if viewport == null:
		return
	var aspect := size.x / maxf(size.y, 1.0)
	viewport.size = Vector2i(maxi(1, roundi(lines * aspect)), lines)


## Forward input to the low-resolution viewport, scaling mouse positions into it.
func _input(event: InputEvent) -> void:
	if viewport == null:
		return
	if event is InputEventMouse:
		event = event.duplicate()
		var local: Vector2 = (event.position - global_position) / maxf(size.x, 1.0)
		event.position = local * float(viewport.size.x)
	viewport.push_input(event, true)


func _on_node_added(node: Node) -> void:
	if viewport != null and viewport.is_ancestor_of(node):
		# Deferred: rigged scenes assign their materials in _ready.
		_take_over.call_deferred(node)


func _take_over(root: Node) -> void:
	if not is_instance_valid(root):
		return
	var nodes: Array[Node] = root.find_children("*", "GeometryInstance3D", true, false)
	if root is GeometryInstance3D:
		nodes.append(root)
	for node in nodes:
		var gi := node as GeometryInstance3D
		_quiet(gi.material_override)
		var mi := gi as MeshInstance3D
		if mi != null and mi.mesh != null:
			for i in mi.mesh.get_surface_count():
				_quiet(mi.get_surface_override_material(i))
				_quiet(mi.mesh.surface_get_material(i))


func _quiet(material: Material) -> void:
	var sm := material as ShaderMaterial
	if sm == null or sm.shader == null or not sm.shader.resource_path.get_file().begins_with("psx_"):
		return
	sm.set_shader_parameter("dither", false)
	sm.set_shader_parameter("color_levels", 256.0)
