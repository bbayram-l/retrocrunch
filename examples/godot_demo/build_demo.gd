extends SceneTree
## Builds demo.tscn: a street at night that is far over a PS1 polygon budget, so the
## speed limit, draw distance and budget dock all have something to show.
## Rebuild with:  godot --headless --path . -s build_demo.gd
## Only Godot's built-in meshes and generated textures: no third-party assets.

var demo: Node3D


func _initialize() -> void:
	demo = Node3D.new()
	demo.name = "Demo"
	demo.set_script(load("res://demo.gd"))

	var env := Environment.new()
	env.background_mode = Environment.BG_COLOR
	env.background_color = Color(0.06, 0.06, 0.12)
	env.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	env.ambient_light_color = Color(0.35, 0.35, 0.5)
	env.tonemap_mode = Environment.TONE_MAPPER_LINEAR
	var world := WorldEnvironment.new()
	world.environment = env
	_add(world, "Environment")

	var moon := DirectionalLight3D.new()
	moon.rotation_degrees = Vector3(-55, 35, 0)
	moon.light_color = Color(0.8, 0.85, 1.0)
	_add(moon, "Moon")

	var brick := _material(_brick_texture(), Vector3(1, 1, 1))
	var road := _material(_road_texture(), Vector3(6, 32, 1))
	var bark := _material(null, Vector3.ONE, Color(0.35, 0.22, 0.12))
	var leaves := _material(null, Vector3.ONE, Color(0.2, 0.5, 0.25))
	var stone := _material(null, Vector3.ONE, Color(0.75, 0.72, 0.65))

	var ground_mesh := PlaneMesh.new()
	ground_mesh.size = Vector2(60, 320)
	ground_mesh.material = road
	var ground := MeshInstance3D.new()
	ground.mesh = ground_mesh
	ground.position = Vector3(0, 0, -150)
	_add(ground, "Road")

	# Buildings: cheap boxes (12 triangles), a few shared sizes.
	var boxes: Array[BoxMesh] = []
	for size in [Vector3(7, 8, 9), Vector3(8, 14, 10), Vector3(6, 20, 8), Vector3(9, 11, 9)]:
		var b := BoxMesh.new()
		b.size = size
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

	var limiter := Node.new()
	limiter.set_script(load("res://psx_speed_limit.gd"))
	limiter.set("show_stats", true)
	_add(limiter, "SpeedLimit")

	var draw := Node.new()
	draw.set_script(load("res://psx_draw_distance.gd"))
	draw.set("show_stats", true)
	_add(draw, "DrawDistance")

	var scene := PackedScene.new()
	var err := scene.pack(demo)
	if err == OK:
		err = ResourceSaver.save(scene, "res://demo.tscn")
	print("demo.tscn: ", error_string(err))
	demo.free()
	quit(0 if err == OK else 1)


func _add(node: Node, node_name: String, parent: Node = null) -> void:
	node.name = node_name
	(parent if parent else demo).add_child(node)
	node.owner = demo


func _material(tex: Texture2D, uv_scale: Vector3, color := Color.WHITE) -> StandardMaterial3D:
	var m := StandardMaterial3D.new()
	m.albedo_color = color
	if tex:
		m.albedo_texture = tex
		m.uv1_scale = uv_scale
	m.texture_filter = BaseMaterial3D.TEXTURE_FILTER_NEAREST
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
