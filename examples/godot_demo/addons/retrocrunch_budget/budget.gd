@tool
extends RefCounted
## retrocrunch scene budget: measures a scene against PS1 limits. Used by the editor dock
## (plugin.gd); callable from your own tools too: `Budget.analyze(root, camera, 6000)`.
##
## Polygons count a flat quad once, as PS1 budgets did: two triangles sharing an edge (same
## corner positions and UVs), within 10 degrees of flat and convex. Texture memory uses the
## PS1 formats: 4-bit (16 colours) or 8-bit (256) with a 16-bit palette, else 16-bit. A
## texture shared by several objects is counted once.

## Sony's figure for textured, Gouraud-lit polygons per second.
const POLYS_PER_SECOND := 180000
const QUAD_MAX_ANGLE_DEG := 10.0
## Objects covering less than this share of the view height count as small on screen.
const SMALL_ON_SCREEN := 0.05
## Polygons above which a small, far object is worth an LOD or a clip distance.
const HEAVY_POLYS := 200

static var _mesh_cache := {}


## Measure every visible MeshInstance3D under `root`. `camera` (may be null) decides what
## is in view and how far away it is. Returns a Dictionary: objects, textures, totals and
## the objects to clip (farthest first) to get the view under `target_polys`.
static func analyze(root: Node, camera: Camera3D, target_polys: int) -> Dictionary:
	_mesh_cache.clear()
	var objects: Array[Dictionary] = []
	var textures := {}
	for mi: MeshInstance3D in root.find_children("*", "MeshInstance3D", true, false):
		if mi.mesh == null or not mi.is_visible_in_tree():
			continue
		var counts := mesh_counts(mi.mesh)
		var tex_list: Array[Texture2D] = []
		for s in mi.mesh.get_surface_count():
			for t in material_textures(mi.get_active_material(s)):
				if not tex_list.has(t):
					tex_list.append(t)
				if not textures.has(t):
					textures[t] = texture_info(t)
		var box: AABB = mi.global_transform * mi.get_aabb()
		var o := {
			"node": mi,
			"name": str(root.get_path_to(mi)) if root != mi else str(mi.name),
			"tris": counts.tris,
			"polys": counts.tris - counts.quads,
			"textures": tex_list,
			"distance": -1.0,
			"in_view": true,
			"screen": 1.0,
			"notes": [],
		}
		if camera:
			var eye := camera.global_position
			o.distance = eye.distance_to(box.get_center())
			o.in_view = _in_frustum(box, camera.get_frustum())
			var half_view := tan(deg_to_rad(camera.fov) * 0.5) * maxf(o.distance, 0.001)
			o.screen = box.size.length() * 0.5 / half_view
		objects.append(o)
	objects.sort_custom(func(a, b): return a.polys > b.polys)

	var view_polys := 0
	var view_tris := 0
	var scene_polys := 0
	for o in objects:
		scene_polys += o.polys
		if o.in_view:
			view_polys += o.polys
			view_tris += o.tris
	var vram := 0.0
	for t in textures:
		vram += textures[t].kb

	# Heaviest objects, and heavy objects that are tiny on screen.
	for i in mini(3, objects.size()):
		if objects[i].polys > 0:
			objects[i].notes.append("most detailed")
	for o in objects:
		if o.in_view and o.polys >= HEAVY_POLYS and o.screen < SMALL_ON_SCREEN:
			o.notes.append("small on screen: LOD")

	# Clip the farthest in-view objects until the view fits the target.
	var clip: Array[Dictionary] = []
	if camera and view_polys > target_polys:
		var far := objects.filter(func(o): return o.in_view and o.polys > 0)
		far.sort_custom(func(a, b): return a.distance > b.distance)
		var left := view_polys
		for o in far:
			if left <= target_polys:
				break
			left -= o.polys
			clip.append(o)
			o.notes.insert(0, "clip: range_end %.1f" % (o.distance * 0.9))

	return {
		"objects": objects,
		"textures": textures,
		"view_polys": view_polys,
		"view_tris": view_tris,
		"scene_polys": scene_polys,
		"vram_kb": vram,
		"fps": fps_for(view_polys),
		"clip": clip,
	}


## Texture memory left in `vram_mb` of VRAM after `framebuffers` 16-bit buffers of
## `size`. The PS1 default, 1 MB with two 320x240 buffers, leaves 724 KB.
static func vram_budget_kb(vram_mb: int, framebuffers: int, size: Vector2i) -> float:
	return vram_mb * 1024.0 - framebuffers * size.x * size.y * 2 / 1024.0


## PS1 frame rate for `polys` per frame at 60 Hz, waiting for vsync like the hardware.
@warning_ignore("integer_division")
static func fps_for(polys: int) -> int:
	var frames := maxi(1, (polys * 60 + POLYS_PER_SECOND - 1) / POLYS_PER_SECOND)
	return 60 / frames


## Triangle and quad counts of a mesh (cached per mesh resource during one analysis).
@warning_ignore("integer_division")
static func mesh_counts(mesh: Mesh) -> Dictionary:
	if _mesh_cache.has(mesh):
		return _mesh_cache[mesh]
	var tris := 0
	var quads := 0
	for s in mesh.get_surface_count():
		# Only ArrayMesh can hold lines or points; primitive meshes are triangles.
		if mesh is ArrayMesh and mesh.surface_get_primitive_type(s) != Mesh.PRIMITIVE_TRIANGLES:
			continue
		var arrays := mesh.surface_get_arrays(s)
		var verts: PackedVector3Array = arrays[Mesh.ARRAY_VERTEX]
		var idx: PackedInt32Array = arrays[Mesh.ARRAY_INDEX]
		if idx.is_empty():
			idx = PackedInt32Array(range(verts.size()))
		var uvs: PackedVector2Array = arrays[Mesh.ARRAY_TEX_UV] if arrays[Mesh.ARRAY_TEX_UV] else PackedVector2Array()
		tris += idx.size() / 3
		quads += quad_count(verts, uvs, idx)
	var r := {"tris": tris, "quads": quads}
	_mesh_cache[mesh] = r
	return r


## Triangle pairs that can be drawn as one quad (same rule as retrocrunch's reports).
@warning_ignore("integer_division")
static func quad_count(verts: PackedVector3Array, uvs: PackedVector2Array, idx: PackedInt32Array) -> int:
	# Weld corners that only differ in normal (flat-shaded exports split every corner).
	var first := {}
	var weld := PackedInt32Array()
	weld.resize(verts.size())
	for i in verts.size():
		var key := [verts[i], uvs[i] if i < uvs.size() else Vector2.ZERO]
		weld[i] = first.get_or_add(key, i)
	var n_tris := idx.size() / 3
	var tris: Array[PackedInt32Array] = []
	var normals: Array[Vector3] = []
	for t in n_tris:
		var tri := PackedInt32Array([weld[idx[t * 3]], weld[idx[t * 3 + 1]], weld[idx[t * 3 + 2]]])
		tris.append(tri)
		normals.append((verts[tri[1]] - verts[tri[0]]).cross(verts[tri[2]] - verts[tri[0]]).normalized())
	var edges := {}
	for t in n_tris:
		for k in 3:
			var a := tris[t][k]
			var b := tris[t][(k + 1) % 3]
			var key := Vector2i(mini(a, b), maxi(a, b))
			if not edges.has(key):
				edges[key] = []
			edges[key].append([t, tris[t][(k + 2) % 3]])
	var min_dot := cos(deg_to_rad(QUAD_MAX_ANGLE_DEG))
	var pairs := []
	for key: Vector2i in edges:
		var users: Array = edges[key]
		if users.size() != 2 or users[0][0] == users[1][0]:
			continue
		var t1: int = users[0][0]
		var t2: int = users[1][0]
		var n1 := normals[t1]
		var n2 := normals[t2]
		if n1 == Vector3.ZERO or n2 == Vector3.ZERO:
			continue
		var d := n1.dot(n2)
		if d < min_dot:
			continue
		# Convex: the other diagonal must cross the shared edge.
		var c := verts[users[0][1]]
		var e := verts[users[1][1]] - c
		var n := n1 + n2
		if e.cross(verts[key.x] - c).dot(n) * e.cross(verts[key.y] - c).dot(n) >= 0.0:
			continue
		pairs.append([d, mini(t1, t2), maxi(t1, t2)])
	pairs.sort_custom(func(x, y):
		if x[0] != y[0]:
			return x[0] > y[0]
		return x[1] < y[1] if x[1] != y[1] else x[2] < y[2])
	var used := PackedByteArray()
	used.resize(n_tris)
	var quads := 0
	for p in pairs:
		if used[p[1]] == 0 and used[p[2]] == 0:
			used[p[1]] = 1
			used[p[2]] = 1
			quads += 1
	return quads


## Textures a material samples (StandardMaterial3D albedo, any ShaderMaterial texture).
static func material_textures(mat: Material) -> Array[Texture2D]:
	var out: Array[Texture2D] = []
	if mat is BaseMaterial3D and mat.albedo_texture:
		out.append(mat.albedo_texture)
	elif mat is ShaderMaterial and mat.shader:
		for u in mat.shader.get_shader_uniform_list():
			var v = mat.get_shader_parameter(u.name)
			if v is Texture2D and not out.has(v):
				out.append(v)
	return out


## PS1 format and size of a texture: 4-bit with <= 16 colours, 8-bit with <= 256, else 16.
static func texture_info(tex: Texture2D) -> Dictionary:
	var w := tex.get_width()
	var h := tex.get_height()
	var colors := 257
	var compressed := false
	var img := tex.get_image()
	if img:
		# GPU-compressed imports blend palette colours, so the count is only a guess.
		compressed = img.is_compressed()
		if compressed:
			img.decompress()
		img.convert(Image.FORMAT_RGBA8)
		colors = _count_colors(img.get_data(), 257)
	var bits := 4 if colors <= 16 else (8 if colors <= 256 else 16)
	var clut := 16 if bits == 4 else (256 if bits == 8 else 0)
	var kb := (w * h * bits / 8.0 + clut * 2.0) / 1024.0
	return {"width": w, "height": h, "colors": colors, "bits": bits, "kb": kb, "compressed": compressed}


static func _count_colors(rgba: PackedByteArray, cap: int) -> int:
	var seen := {}
	var words := rgba.to_int32_array()
	for c in words:
		seen[c] = true
		if seen.size() >= cap:
			return cap
	return seen.size()


static func _in_frustum(box: AABB, planes: Array[Plane]) -> bool:
	for p in planes:
		# Godot's frustum planes face outward: outside when the box is fully in front.
		var corner := Vector3(
			box.position.x if p.normal.x > 0 else box.end.x,
			box.position.y if p.normal.y > 0 else box.end.y,
			box.position.z if p.normal.z > 0 else box.end.z,
		)
		if p.is_point_over(corner):
			return false
	return true
