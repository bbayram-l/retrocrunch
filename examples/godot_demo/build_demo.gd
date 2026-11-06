extends SceneTree
## Builds demo.tscn: a street at night that is far over a PS1 polygon budget, rendered
## with retrocrunch's PS1 materials under the 240-line screen pass, so every Godot tool
## has something to show.
## Rebuild with:  godot --headless --path . -s build_demo.gd
## Only Godot's built-in meshes and generated textures: no third-party assets.

var demo: Node
var world: Node3D


func _initialize() -> void:
	demo = Node.new()
	demo.name = "Demo"
	demo.set_script(load("res://demo.gd"))
	# The PS1 screen pass: everything 3D renders at 240 lines under it, with the dither
	# and 15-bit colour once per virtual pixel, then scales up with nearest filtering.
	var screen := Control.new()
	screen.name = "Screen"
	screen.set_script(load("res://psx_screen.gd"))
	demo.add_child(screen)
	screen.owner = demo
	world = Node3D.new()
	world.name = "World"
	screen.add_child(world)
	world.owner = demo

	var env := Environment.new()
	env.background_mode = Environment.BG_COLOR
	env.background_color = Color(0.06, 0.06, 0.12)
	env.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	env.ambient_light_color = Color(0.35, 0.35, 0.5)
	env.tonemap_mode = Environment.TONE_MAPPER_LINEAR
	var world := WorldEnvironment.new()
	world.environment = env
	_add(world, "Environment")

	# Every surface uses retrocrunch's PS1 material (psx_opaque.gdshader), which is
	# unshaded: these Godot lights reach it through the PS1Lights node, per vertex.
	var moon := DirectionalLight3D.new()
	moon.rotation_degrees = Vector3(-55, 35, 0)
	moon.light_color = Color(0.7, 0.75, 1.0)
	moon.light_energy = 0.5
	_add(moon, "Moon")

	var brick := _material(_brick_texture(), Vector3(1, 1, 1))
	var road := _material(_road_texture(), Vector3(6, 2, 1))
	var bark := _material(null, Vector3.ONE, Color(0.35, 0.22, 0.12))
	var leaves := _material(null, Vector3.ONE, Color(0.2, 0.5, 0.25))
	var stone := _material(null, Vector3.ONE, Color(0.75, 0.72, 0.65))

	# The road in 20 m tiles, each with near-detail versions like a retrocrunch export
	# with tessellate = 2: the plain tile (2 triangles) from 30 m, 4x the triangles from
	# 15 m, 16x up close. Godot shows each only within its band; the metadata is what the
	# heatmap colours by.
	var bands := [
		# [name, subdivisions, triangle ratio, begin, end]
		["NEAR2", 3, 16.0, 0.0, 15.0],
		["NEAR1", 1, 4.0, 15.0, 30.0],
		["LOD0", 0, 1.0, 30.0, 0.0],
	]
	var tile_meshes := {}
	for b in bands:
		var m := PlaneMesh.new()
		m.size = Vector2(60, 20)
		m.subdivide_width = b[1]
		m.subdivide_depth = b[1]
		m.material = road
		tile_meshes[b[0]] = m
	var road_node := Node3D.new()
	_add(road_node, "Road")
	for i in 16:
		var tile := Node3D.new()
		tile.position = Vector3(0, 0, 10 - i * 20)
		_add(tile, "Tile%d" % i, road_node)
		for b in bands:
			var v := MeshInstance3D.new()
			v.mesh = tile_meshes[b[0]]
			v.visibility_range_begin = b[3]
			v.visibility_range_end = b[4]
			if b[2] != 1.0:
				v.set_meta("retrocrunch_tri_ratio", b[2])
			_add(v, b[0], tile)

	# Buildings: boxes with every face split 3x3, as PS1 artists split big walls so the
	# affine texture warping stays small (108 triangles each).
	var boxes: Array[BoxMesh] = []
	for size in [Vector3(7, 8, 9), Vector3(8, 14, 10), Vector3(6, 20, 8), Vector3(9, 11, 9)]:
		var b := BoxMesh.new()
		b.size = size
		b.subdivide_width = 2
		b.subdivide_height = 2
		b.subdivide_depth = 2
		b.material = brick
		boxes.append(b)
	var street := Node3D.new()
	_add(street, "Street")
	for i in 25:
		for side in [-1, 1]:
			var b: BoxMesh = boxes[(i * 3 + (side + 1)) % boxes.size()]
			var house := MeshInstance3D.new()
			house.mesh = b
			house.position = Vector3(side * 13, b.size.y / 2, -i * 12)
			_add(house, "House%d%s" % [i, "L" if side < 0 else "R"], street)

	# Trees: the heavy part of the street (about 1000 triangles each).
	var trunk := CylinderMesh.new()
	trunk.top_radius = 0.15
	trunk.bottom_radius = 0.2
	trunk.height = 2.5
	trunk.radial_segments = 12
	trunk.material = bark
	var crown := SphereMesh.new()
	crown.radius = 1.4
	crown.height = 2.6
	crown.radial_segments = 32
	crown.rings = 16
	crown.material = leaves
	var trees := Node3D.new()
	_add(trees, "Trees")
	for i in 24:
		for side in [-1, 1]:
			var tree := Node3D.new()
			tree.position = Vector3(side * 6.5, 0, -6 - i * 12)
			_add(tree, "Tree%d%s" % [i, "L" if side < 0 else "R"], trees)
			var t := MeshInstance3D.new()
			t.mesh = trunk
			t.position = Vector3(0, 1.25, 0)
			_add(t, "Trunk", tree)
			var c := MeshInstance3D.new()
			c.mesh = crown
			c.position = Vector3(0, 3.3, 0)
			_add(c, "Crown", tree)

	# Street lamps: warm point lights, and lamp heads in the PS1's additive
	# semi-transparency (psx_add_double.gdshader), so they glow on what's behind them.
	var post := CylinderMesh.new()
	post.top_radius = 0.08
	post.bottom_radius = 0.1
	post.height = 4.0
	post.radial_segments = 8
	post.material = stone
	var head := SphereMesh.new()
	head.radius = 0.35
	head.height = 0.7
	head.radial_segments = 12
	head.rings = 6
	head.material = _material(null, Vector3.ONE, Color(0.55, 0.4, 0.2), "psx_add_double")
	var lamps := Node3D.new()
	_add(lamps, "Lamps")
	for i in 13:
		var side := -1 if i % 2 == 0 else 1
		var lamp := Node3D.new()
		lamp.position = Vector3(side * 9.0, 0, -4 - i * 24)
		_add(lamp, "Lamp%d" % i, lamps)
		var p := MeshInstance3D.new()
		p.mesh = post
		p.position = Vector3(0, 2, 0)
		_add(p, "Post", lamp)
		var h := MeshInstance3D.new()
		h.mesh = head
		h.position = Vector3(0, 4.1, 0)
		_add(h, "Glow", lamp)
		var light := OmniLight3D.new()
		light.position = Vector3(0, 3.8, 0)
		light.light_color = Color(1.0, 0.7, 0.4)
		light.light_energy = 1.4
		light.omni_range = 10.0
		_add(light, "Light", lamp)

	# One very detailed object far away: the dock should suggest clipping it.
	var statue_mesh := SphereMesh.new()
	statue_mesh.radius = 3.0
	statue_mesh.height = 6.0
	statue_mesh.radial_segments = 96
	statue_mesh.rings = 48
	statue_mesh.material = stone
	var statue := MeshInstance3D.new()
	statue.mesh = statue_mesh
	statue.position = Vector3(0, 3, -300)
	_add(statue, "Monument")

	var cam := Camera3D.new()
	cam.position = Vector3(0, 1.7, 8)
	cam.current = true
	cam.set_script(load("res://fly_camera.gd"))
	_add(cam, "Player")
	var flashlight := SpotLight3D.new()
	flashlight.light_color = Color(1.0, 0.95, 0.85)
	flashlight.light_energy = 2.5
	flashlight.spot_range = 25.0
	# Wide and tilted down: lighting is per vertex, so a narrow cone falls between the
	# vertices of big polygons (the subdivided road tiles up close catch it best).
	flashlight.spot_angle = 30.0
	flashlight.rotation_degrees = Vector3(-12, 0, 0)
	flashlight.visible = false
	_add(flashlight, "Flashlight", cam)

	var limiter := Node.new()
	limiter.set_script(load("res://psx_speed_limit.gd"))
	limiter.set("show_stats", true)
	_add(limiter, "SpeedLimit")

	var draw := Node.new()
	draw.set_script(load("res://psx_draw_distance.gd"))
	draw.set("show_stats", true)
	_add(draw, "DrawDistance")

	var heatmap := Node.new()
	heatmap.set_script(load("res://psx_detail_heatmap.gd"))
	heatmap.set("enabled", false)
	_add(heatmap, "DetailHeatmap")

	var vram := Node.new()
	vram.set_script(load("res://psx_vram_overlay.gd"))
	vram.set("visible", false)
	_add(vram, "VramOverlay")

	var lights := Node.new()
	lights.set_script(load("res://psx_lights.gd"))
	lights.set("ambient", Color(0.3, 0.3, 0.38))
	_add(lights, "PS1Lights")

	var scene := PackedScene.new()
	var err := scene.pack(demo)
	if err == OK:
		err = ResourceSaver.save(scene, "res://demo.tscn")
	print("demo.tscn: ", error_string(err))
	demo.free()
	quit(0 if err == OK else 1)


func _add(node: Node, node_name: String, parent: Node = null) -> void:
	node.name = node_name
	(parent if parent else world).add_child(node)
	node.owner = demo


## A retrocrunch PS1 material (the shaders an export writes). Tint and colours are sRGB
## values, passed as Vector4 so Godot doesn't colour-convert them.
func _material(tex: Texture2D, uv_scale: Vector3, color := Color.WHITE, shader := "psx_opaque") -> ShaderMaterial:
	var m := ShaderMaterial.new()
	m.shader = load("res://%s.gdshader" % shader)
	m.set_shader_parameter("use_texture", tex != null)
	if tex:
		m.set_shader_parameter("albedo_texture", tex)
	m.set_shader_parameter("uv_scale", Vector2(uv_scale.x, uv_scale.y))
	m.set_shader_parameter("tint", Vector4(color.r, color.g, color.b, 1.0))
	# Night fog in the sky's colour; demo.gd keeps it ending at the draw distance.
	m.set_shader_parameter("fog_enabled", true)
	m.set_shader_parameter("fog_color", Vector4(0.06, 0.06, 0.12, 1.0))
	m.set_shader_parameter("fog_start", 12.0)
	m.set_shader_parameter("fog_end", 40.0)
	return m


## 64x64 bricks in 16 colours (a 4-bit PS1 texture: 2 KB + a 16-colour palette).
func _brick_texture() -> ImageTexture:
	var img := Image.create(64, 64, false, Image.FORMAT_RGBA8)
	for y in 64:
		for x in 64:
			var row := y / 8
			var mortar := y % 8 == 0 or (x + (row % 2) * 8) % 16 == 0
			var shade := ((x * 7 + y * 13) % 5) * 0.03
			var c := Color(0.55 + shade, 0.25 + shade, 0.2) if not mortar else Color(0.6, 0.6, 0.55)
			img.set_pixel(x, y, Color(snappedf(c.r, 0.03), snappedf(c.g, 0.03), c.b))
	return ImageTexture.create_from_image(img)


## 32x32 asphalt in 8 colours, tiled along the road.
func _road_texture() -> ImageTexture:
	var img := Image.create(32, 32, false, Image.FORMAT_RGBA8)
	for y in 32:
		for x in 32:
			var n := ((x * 31 + y * 17) ^ (x * y)) % 4
			img.set_pixel(x, y, Color(0.18 + n * 0.03, 0.18 + n * 0.03, 0.2 + n * 0.03))
	for y in 32:
		if y % 16 < 8:
			img.set_pixel(15, y, Color(0.85, 0.8, 0.4))
			img.set_pixel(16, y, Color(0.85, 0.8, 0.4))
	return ImageTexture.create_from_image(img)
