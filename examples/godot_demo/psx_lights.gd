extends Node
## retrocrunch PS1 lights: lets Godot's own lights move over retrocrunch's PS1 materials.
## Those materials are unshaded (lighting is baked into vertex colours), so Godot's lights
## don't reach them. This node collects the DirectionalLight3D, OmniLight3D and
## SpotLight3D nodes in the scene every frame and passes them to every psx_* material,
## which lights each vertex and blends the result across the triangle (Gouraud), before
## the 15-bit colour and dither.
##
## Two modes:
## - Extended (default): up to 8 lights per material, with real point and spot falloff
##   at every vertex. Easy to light with, but more than a PS1 could do.
## - Authentic: what PS1 games did. The GTE held three directional lights plus ambient;
##   point and spot lights didn't exist in hardware, so games turned each one into a
##   direction from the light to the object, dimmed by distance, once per object on the
##   CPU. Each object then gets its three strongest lights. A big object lit by a nearby
##   lamp is lit evenly from the lamp's direction, not brighter at the near end.
##
## The light multiplies the baked vertex colours: ambient 1 and no lights leaves a model
## as converted. Lights can brighten up to 2x, like PS1 colour modulation.
##
## Usage: add a Node with this script anywhere in the scene and light it with ordinary
## Godot lights (colour, energy, range, spot angle are used; shadows are not).

enum Mode { EXTENDED, AUTHENTIC }

## Pass lights to the materials. Off restores the plain baked colours.
@export var enabled := true:
	set(value):
		enabled = value
		if not value and is_inside_tree():
			_switch_off()
## Extended: 8 lights per vertex. Authentic: 3 per object, as on a PS1 (see above).
@export var mode := Mode.EXTENDED:
	set(value):
		if value != mode and is_inside_tree():
			_switch_off()
		mode = value
## Light every surface gets, multiplying the baked vertex colours. White keeps a model
## as bright as it was converted; darker lets the dynamic lights stand out.
@export var ambient := Color(0.35, 0.35, 0.4)
## Only lights in this group, if set; otherwise every visible light in the scene.
@export var light_group: StringName = &""
## Seconds between scans for new psx materials (models streamed in).
@export var rescan_seconds := 1.0

const MAX_LIGHTS := 8
## Lights per object in Authentic mode: the GTE's light matrix holds three.
const GTE_LIGHTS := 3

## Lights passed last frame (Extended: per material; Authentic: most on one object).
var lights_used := 0

var _materials: Array[ShaderMaterial] = []
var _objects: Array[GeometryInstance3D] = []
var _scan_timer := INF


func _exit_tree() -> void:
	_switch_off()


func _process(delta: float) -> void:
	if not enabled:
		return
	_scan_timer += delta
	if _scan_timer >= rescan_seconds:
		_scan_timer = 0.0
		_scan()
	if mode == Mode.AUTHENTIC:
		_light_objects()
	else:
		_light_materials()


## Extended: the same 8 lights for every material, evaluated per vertex.
func _light_materials() -> void:
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


## Authentic: per object, every light becomes a direction towards it with a colour
## (point and spot lights dimmed by distance and cone at the object's centre), and the
## three strongest go into the object's light matrix.
func _light_objects() -> void:
	var lights := _lights_in_scene()
	var amb := Vector4(ambient.r, ambient.g, ambient.b, 1.0)
	lights_used = 0
	for gi in _objects:
		if not is_instance_valid(gi):
			continue
		var centre := (gi.global_transform * gi.get_aabb()).get_center()
		var picked := []  # [strength, direction, colour]
		for light in lights:
			var c := light.light_color * light.light_energy
			var dir: Vector3
			var k := 1.0
			if light is DirectionalLight3D:
				dir = light.global_basis.z.normalized()
			else:
				var to_light := light.global_position - centre
				var d := to_light.length()
				dir = to_light / maxf(d, 1e-4)
				var reach: float = (light as SpotLight3D).spot_range if light is SpotLight3D else (light as OmniLight3D).omni_range
				k = clampf(1.0 - d / maxf(reach, 1e-4), 0.0, 1.0)
				if light is SpotLight3D:
					var sl := light as SpotLight3D
					var cos_cone := cos(deg_to_rad(sl.spot_angle))
					var along := (-dir).dot(-sl.global_basis.z.normalized())
					k *= smoothstep(cos_cone, lerpf(cos_cone, 1.0, 0.1), along)
			if k <= 0.0:
				continue
			picked.append([(c.r + c.g + c.b) * k, dir, Vector4(c.r * k, c.g * k, c.b * k, 0.0)])
		picked.sort_custom(func(a, b): return a[0] > b[0])
		lights_used = maxi(lights_used, mini(picked.size(), GTE_LIGHTS))
		gi.set_instance_shader_parameter("psx_obj_lights", 1)
		gi.set_instance_shader_parameter("psx_obj_ambient", amb)
		for i in GTE_LIGHTS:
			var has := i < picked.size()
			var dv: Vector3 = picked[i][1] if has else Vector3.ZERO
			gi.set_instance_shader_parameter("psx_obj_dir%d" % i, Vector4(dv.x, dv.y, dv.z, 0.0))
			gi.set_instance_shader_parameter("psx_obj_col%d" % i, picked[i][2] if has else Vector4.ZERO)


func _lights_in_scene() -> Array[Light3D]:
	var found: Array[Light3D] = []
	for l: Light3D in get_tree().root.find_children("*", "Light3D", true, false):
		if not l.is_visible_in_tree() or l.light_energy <= 0.0:
			continue
		if light_group != &"" and not l.is_in_group(light_group):
			continue
		if l is DirectionalLight3D or l is OmniLight3D or l is SpotLight3D:
			found.append(l)
	return found


## Up to MAX_LIGHTS visible lights: directional first, then the nearest to the camera.
func _pick_lights() -> Array[Light3D]:
	var cam := get_viewport().get_camera_3d()
	var eye := cam.global_position if cam else Vector3.ZERO
	var found := _lights_in_scene()
	found.sort_custom(func(a, b):
		var da := -1.0 if a is DirectionalLight3D else eye.distance_to(a.global_position)
		var db := -1.0 if b is DirectionalLight3D else eye.distance_to(b.global_position)
		return da < db)
	return found.slice(0, MAX_LIGHTS)


## Every retrocrunch PS1 material in the scene (they have the psx_light_count uniform)
## and the objects that use them.
func _scan() -> void:
	var seen := {}
	var objects: Array[GeometryInstance3D] = []
	for mi: MeshInstance3D in get_tree().root.find_children("*", "MeshInstance3D", true, false):
		if mi.mesh == null:
			continue
		var uses := false
		for s in mi.mesh.get_surface_count():
			var m := mi.get_active_material(s) as ShaderMaterial
			if m and m.shader and _is_psx(m.shader):
				uses = true
				seen[m] = true
		if uses:
			objects.append(mi)
	_materials.assign(seen.keys())
	_objects = objects


static func _is_psx(shader: Shader) -> bool:
	for u in shader.get_shader_uniform_list():
		if u.name == "psx_light_count":
			return true
	return false


func _switch_off() -> void:
	for m in _materials:
		if is_instance_valid(m):
			m.set_shader_parameter("psx_light_count", 0)
	for gi in _objects:
		if is_instance_valid(gi):
			gi.set_instance_shader_parameter("psx_obj_lights", 0)
	lights_used = 0
