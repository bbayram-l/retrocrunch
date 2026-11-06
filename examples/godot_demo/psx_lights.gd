extends Node
## retrocrunch PS1 lights: lets Godot's own lights move over retrocrunch's PS1 materials.
## Those materials are unshaded (lighting is baked into vertex colours), so Godot's lights
## don't reach them. This node collects the DirectionalLight3D, OmniLight3D and
## SpotLight3D nodes in the scene every frame and passes them to every psx_* material,
## which lights each vertex and blends the result across the triangle (Gouraud), like the
## PS1's geometry coprocessor did, before the 15-bit colour and dither.
##
## The light multiplies the baked vertex colours: ambient 1 and no lights leaves a model
## as converted. Lights can brighten up to 2x, like PS1 colour modulation.
##
## Usage: add a Node with this script anywhere in the scene and light it with ordinary
## Godot lights (colour, energy, range, spot angle are used; shadows are not). At most 8
## lights reach a material: directional lights first, then the point and spot lights
## nearest the camera.

## Pass lights to the materials. Off restores the plain baked colours.
@export var enabled := true:
	set(value):
		enabled = value
		if not value and is_inside_tree():
			_switch_off()
## Light every surface gets, multiplying the baked vertex colours. White keeps a model
## as bright as it was converted; darker lets the dynamic lights stand out.
@export var ambient := Color(0.35, 0.35, 0.4)
## Only lights in this group, if set; otherwise every visible light in the scene.
@export var light_group: StringName = &""
## Seconds between scans for new psx materials (models streamed in).
@export var rescan_seconds := 1.0

const MAX_LIGHTS := 8

## Lights passed last frame.
var lights_used := 0

var _materials: Array[ShaderMaterial] = []
var _scan_timer := INF


func _exit_tree() -> void:
	_switch_off()


func _process(delta: float) -> void:
	if not enabled:
		return
	_scan_timer += delta
	if _scan_timer >= rescan_seconds:
		_scan_timer = 0.0
		_scan_materials()
	var pos := PackedVector4Array()
	var col := PackedVector4Array()
	var spot := PackedVector4Array()
	for light in _pick_lights():
		var energy := light.light_color * light.light_energy
		if light is DirectionalLight3D:
			# Direction towards the light: the node shines along -Z.
			var z := light.global_basis.z.normalized()
			pos.append(Vector4(z.x, z.y, z.z, 0.0))
			col.append(Vector4(energy.r, energy.g, energy.b, 0.0))
			spot.append(Vector4.ZERO)
		elif light is SpotLight3D:
			var sl := light as SpotLight3D
			var p := sl.global_position
			var axis := -sl.global_basis.z.normalized()
			pos.append(Vector4(p.x, p.y, p.z, 2.0))
			col.append(Vector4(energy.r, energy.g, energy.b, sl.spot_range))
			spot.append(Vector4(axis.x, axis.y, axis.z, cos(deg_to_rad(sl.spot_angle))))
		else:
			var p := light.global_position
			pos.append(Vector4(p.x, p.y, p.z, 1.0))
			col.append(Vector4(energy.r, energy.g, energy.b, (light as OmniLight3D).omni_range))
			spot.append(Vector4.ZERO)
	lights_used = pos.size()
	# Uniform arrays have a fixed size: pad the rest.
	while pos.size() < MAX_LIGHTS:
		pos.append(Vector4.ZERO)
		col.append(Vector4.ZERO)
		spot.append(Vector4.ZERO)
	for m in _materials:
		m.set_shader_parameter("psx_light_count", lights_used)
		m.set_shader_parameter("psx_ambient", Vector4(ambient.r, ambient.g, ambient.b, 1.0))
		m.set_shader_parameter("psx_light_pos", pos)
		m.set_shader_parameter("psx_light_color", col)
		m.set_shader_parameter("psx_light_spot", spot)


## Up to MAX_LIGHTS visible lights: directional first, then the nearest to the camera.
func _pick_lights() -> Array[Light3D]:
	var cam := get_viewport().get_camera_3d()
	var eye := cam.global_position if cam else Vector3.ZERO
	var found: Array[Light3D] = []
	for l: Light3D in get_tree().root.find_children("*", "Light3D", true, false):
		if not l.is_visible_in_tree() or l.light_energy <= 0.0:
			continue
		if light_group != &"" and not l.is_in_group(light_group):
			continue
		if l is DirectionalLight3D or l is OmniLight3D or l is SpotLight3D:
			found.append(l)
	found.sort_custom(func(a, b):
		var da := -1.0 if a is DirectionalLight3D else eye.distance_to(a.global_position)
		var db := -1.0 if b is DirectionalLight3D else eye.distance_to(b.global_position)
		return da < db)
	return found.slice(0, MAX_LIGHTS)


## Every retrocrunch PS1 material in the scene (they have the psx_light_count uniform).
func _scan_materials() -> void:
	var seen := {}
	for mi: MeshInstance3D in get_tree().root.find_children("*", "MeshInstance3D", true, false):
		if mi.mesh == null:
			continue
		for s in mi.mesh.get_surface_count():
			var m := mi.get_active_material(s) as ShaderMaterial
			if m and m.shader and not seen.has(m) and _is_psx(m.shader):
				seen[m] = true
	_materials.assign(seen.keys())


static func _is_psx(shader: Shader) -> bool:
	for u in shader.get_shader_uniform_list():
		if u.name == "psx_light_count":
			return true
	return false


func _switch_off() -> void:
	for m in _materials:
		if is_instance_valid(m):
			m.set_shader_parameter("psx_light_count", 0)
	lights_used = 0
