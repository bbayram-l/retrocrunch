extends Node
## retrocrunch VRAM overlay (debug tool): packs the textures on screen into a picture of
## PS1 video memory, the way a PS1 game would lay them out, and draws it over the game.
##
## PS1 VRAM is 1024x512 16-bit words. The framebuffers sit on the left; textures go in the
## rest without crossing a 256x256 texture page; colour tables (CLUTs, 16-word aligned)
## fill gaps, usually the rows under the framebuffers. A texture's width in words depends
## on its format: 4-bit (up to 16 colours) packs 4 texels per word, 8-bit (up to 256) 2,
## and anything with more colours is 16-bit. So a 128x128 4-bit texture is 32 words wide,
## and the overlay shows it squeezed, as a VRAM viewer in an emulator would.
##
## Usage: add a Node with this script anywhere in your scene. F3 shows and hides it.
## Outlines: green 4-bit, yellow 8-bit, red 16-bit, magenta CLUT, grey framebuffer. The
## line above the picture totals it up and counts what doesn't fit.

enum Scope { IN_VIEW, WHOLE_SCENE }

@export var visible := true:
	set(value):
		visible = value
		if _layer:
			_layer.visible = value
## Textures of the objects the camera sees, or of every object in the scene.
@export var scope := Scope.IN_VIEW
## VRAM size: 1 MB on a PS1; 2, 4 or 8 for arcade boards or a higher-detail mode.
@export_enum("1 MB:1", "2 MB:2", "4 MB:4", "8 MB:8") var vram_mb := 1
## Framebuffers kept in VRAM (most games used 2).
@export_range(0, 2) var framebuffers := 2
## Framebuffer size in pixels (16-bit).
@export var resolution := Vector2i(320, 240)
## Width of the overlay on screen, in pixels.
@export var width := 512
## Seconds between updates (packing is cheap, counting a new texture's colours isn't).
@export var refresh_seconds := 0.5
@export var toggle_key := KEY_F3

## Results of the last update: placed rectangles and what didn't fit.
var placements: Array[Dictionary] = []
var unplaced: Array[String] = []

var _layer: CanvasLayer
var _view: Control
var _label: Label
var _timer := 0.0
var _colors := {}  # Texture2D -> colour count (cached: counting reads the whole image)


func _ready() -> void:
	_layer = CanvasLayer.new()
	_layer.layer = 126
	_layer.visible = visible
	add_child(_layer)
	_view = Control.new()
	_view.anchor_left = 1.0
	_view.anchor_right = 1.0
	_view.anchor_top = 1.0
	_view.anchor_bottom = 1.0
	_view.draw.connect(_draw_vram)
	_layer.add_child(_view)
	_label = Label.new()
	_label.add_theme_color_override("font_outline_color", Color.BLACK)
	_label.add_theme_constant_override("outline_size", 4)
	_label.add_theme_font_size_override("font_size", 13)
	_label.anchor_left = 1.0
	_label.anchor_right = 1.0
	_label.anchor_top = 1.0
	_label.anchor_bottom = 1.0
	_layer.add_child(_label)
	_update()


func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo and event.keycode == toggle_key:
		visible = not visible


func _process(delta: float) -> void:
	_timer += delta
	if visible and _timer >= refresh_seconds:
		_timer = 0.0
		_update()


## VRAM height in lines: 512 per megabyte (always 1024 words wide).
func vram_lines() -> int:
	return 512 * vram_mb


func _update() -> void:
	var textures := _collect_textures()
	var out := layout(textures, vram_mb, framebuffers, resolution)
	placements = out.placements
	unplaced = out.unplaced
	var kb := {"framebuffer": 0.0, "texture": 0.0, "clut": 0.0}
	for p in placements:
		kb[p.kind] += p.rect.size.x * p.rect.size.y * 2 / 1024.0
	var free: float = vram_mb * 1024.0 - kb.framebuffer - kb.texture - kb.clut
	_label.text = "VRAM %d MB: framebuffers %.0f KB, %d textures %.1f KB, CLUTs %.1f KB, free %.0f KB%s" % [
		vram_mb, kb.framebuffer, textures.size(), kb.texture, kb.clut, free,
		"" if unplaced.is_empty() else ", %d DON'T FIT" % unplaced.size()
	]
	_label.add_theme_color_override("font_color", Color(1, 0.4, 0.35) if unplaced else Color.WHITE)
	var s := float(width) / 1024.0
	var size := Vector2(1024, vram_lines()) * s
	_view.offset_left = -size.x - 8
	_view.offset_top = -size.y - 8
	_view.offset_right = -8
	_view.offset_bottom = -8
	_label.offset_left = -size.x - 8
	_label.offset_right = -8
	_label.offset_top = -size.y - 30
	_label.offset_bottom = -size.y - 10
	_view.queue_redraw()


## Textures used by the visible meshes, with their PS1 format.
func _collect_textures() -> Array[Dictionary]:
	var cam := get_viewport().get_camera_3d()
	var planes: Array[Plane] = []
	if cam:
		planes = cam.get_frustum()
	var seen := {}
	var out: Array[Dictionary] = []
	for mi: MeshInstance3D in get_tree().root.find_children("*", "MeshInstance3D", true, false):
		if mi.mesh == null or not mi.is_visible_in_tree():
			continue
		if scope == Scope.IN_VIEW and cam:
			var box: AABB = mi.global_transform * mi.get_aabb()
			if not _in_frustum(box, planes) or not _in_visibility_range(mi, box, cam.global_position):
				continue
		for s in mi.mesh.get_surface_count():
			for tex in _material_textures(mi.get_active_material(s)):
				if seen.has(tex):
					continue
				seen[tex] = true
				var colors := _color_count(tex)
				var bits := 4 if colors <= 16 else (8 if colors <= 256 else 16)
				out.append({
					"name": tex.resource_path.get_file() if tex.resource_path else "texture",
					"texture": tex, "width": tex.get_width(), "height": tex.get_height(),
					"bits": bits,
				})
	return out


## Pack `textures` (dictionaries with width, height, bits) into `vram_mb` of VRAM next to
## `buffers` framebuffers of `res`. Returns {placements: [{name, kind, rect, bits,
## texture}], unplaced: [names]}; rects are in 16-bit words (x) and lines (y).
static func layout(textures: Array[Dictionary], vram_mb: int, buffers: int, res: Vector2i) -> Dictionary:
	var lines := 512 * vram_mb
	var placements: Array[Dictionary] = []
	var unplaced: Array[String] = []
	var free: Array[Rect2i] = []
	# Framebuffers stacked on the left; the column below them and everything right is free.
	var stacked := 0
	for i in buffers:
		if (i + 1) * res.y <= lines and res.x <= 1024:
			placements.append({"name": "framebuffer %d" % (i + 1), "kind": "framebuffer",
				"rect": Rect2i(0, i * res.y, res.x, res.y), "bits": 16, "texture": null})
			stacked += 1
		else:
			unplaced.append("framebuffer %d" % (i + 1))
	if stacked > 0:
		if res.x < 1024:
			free.append(Rect2i(res.x, 0, 1024 - res.x, lines))
		if stacked * res.y < lines:
			free.append(Rect2i(0, stacked * res.y, res.x, lines - stacked * res.y))
	else:
		free.append(Rect2i(0, 0, 1024, lines))

	# Tallest textures first, like the retrocrunch packer.
	var order := textures.duplicate()
	var words := func(t): return ceili(t.width * t.bits / 16.0) * t.height
	order.sort_custom(func(a, b): return a.height > b.height or (a.height == b.height and words.call(a) > words.call(b)))
	for t in order:
		var w := ceili(t.width * t.bits / 16.0)
		var spot := _find_spot(free, w, t.height, _page_span(t.bits), 1, true)
		if spot == Vector2i(-1, -1):
			unplaced.append(t.name)
			continue
		_take(free, Rect2i(spot, Vector2i(w, t.height)))
		placements.append({"name": t.name, "kind": "texture",
			"rect": Rect2i(spot, Vector2i(w, t.height)), "bits": t.bits, "texture": t.texture})
	# CLUTs: one row each (16 words for 4-bit, 256 for 8-bit), x on a 16-word boundary.
	for t in textures:
		if t.bits == 16:
			continue
		var w := 16 if t.bits == 4 else 256
		var spot := _find_spot(free, w, 1, 1024, 16, false)
		if spot == Vector2i(-1, -1):
			unplaced.append("%s CLUT" % t.name)
			continue
		_take(free, Rect2i(spot, Vector2i(w, 1)))
		placements.append({"name": "%s CLUT" % t.name, "kind": "clut",
			"rect": Rect2i(spot, Vector2i(w, 1)), "bits": 16, "texture": null})
	return {"placements": placements, "unplaced": unplaced}


## Words wide one texture page addresses (256 texels) for a bit depth.
static func _page_span(bits: int) -> int:
	return 64 if bits == 4 else (128 if bits == 8 else 256)


## Free spot for a w x h block: top-left first for textures, which must stay inside one
## texture page (`span` words wide from a 64-word column, 256 lines tall); bottom first
## for CLUTs, so they land under the framebuffers like in most games. x steps by `align`.
@warning_ignore("integer_division")
static func _find_spot(free: Array[Rect2i], w: int, h: int, span: int, align: int, paged: bool) -> Vector2i:
	if paged and (w > span or h > 256):
		return Vector2i(-1, -1)
	var cands := free.duplicate()
	if paged:
		cands.sort_custom(func(a, b): return a.position.y < b.position.y or (a.position.y == b.position.y and a.position.x < b.position.x))
	else:
		cands.sort_custom(func(a, b): return a.position.y > b.position.y or (a.position.y == b.position.y and a.position.x < b.position.x))
	for r: Rect2i in cands:
		if w > r.size.x or h > r.size.y:
			continue
		var x := ceili(r.position.x / float(align)) * align
		while x + w <= r.end.x:
			if not paged:
				return Vector2i(x, r.position.y)
			var page_x := x / 64 * 64
			var page_y := r.position.y / 256 * 256
			if x + w <= page_x + span and r.position.y + h <= page_y + 256:
				return Vector2i(x, r.position.y)
			x = (x / 64 + 1) * 64
	return Vector2i(-1, -1)


## Remove `used` from the free rectangle it sits in (guillotine split: left gap, right
## part, below).
static func _take(free: Array[Rect2i], used: Rect2i) -> void:
	for i in free.size():
		var r := free[i]
		if not r.encloses(used):
			continue
		free.remove_at(i)
		if used.position.x > r.position.x:
			free.append(Rect2i(r.position.x, r.position.y, used.position.x - r.position.x, used.size.y))
		if r.end.x > used.end.x:
			free.append(Rect2i(used.end.x, r.position.y, r.end.x - used.end.x, used.size.y))
		if r.end.y > used.end.y:
			free.append(Rect2i(r.position.x, used.end.y, r.size.x, r.end.y - used.end.y))
		return


func _draw_vram() -> void:
	var s := float(width) / 1024.0
	var size := Vector2(1024, vram_lines()) * s
	_view.draw_rect(Rect2(Vector2.ZERO, size), Color(0.06, 0.06, 0.09, 0.92))
	# Texture page grid: 64-word columns, 256-line rows.
	for x in range(64, 1024, 64):
		_view.draw_line(Vector2(x * s, 0), Vector2(x * s, size.y), Color(1, 1, 1, 0.07))
	for y in range(256, vram_lines(), 256):
		_view.draw_line(Vector2(0, y * s), Vector2(size.x, y * s), Color(1, 1, 1, 0.12))
	var outline := {4: Color(0.3, 1, 0.4), 8: Color(1, 0.9, 0.2), 16: Color(1, 0.3, 0.25)}
	for p in placements:
		var r := Rect2(Vector2(p.rect.position) * s, Vector2(p.rect.size) * s)
		match p.kind:
			"framebuffer":
				_view.draw_rect(r, Color(0.35, 0.35, 0.4, 0.8))
				_view.draw_string(ThemeDB.fallback_font, r.position + Vector2(4, 14), "FB", HORIZONTAL_ALIGNMENT_LEFT, -1, 12)
			"texture":
				_view.draw_texture_rect(p.texture, r, false)
				_view.draw_rect(r, outline[p.bits], false, 1.0)
			"clut":
				_view.draw_rect(Rect2(r.position, Vector2(r.size.x, maxf(r.size.y, 1.0))), Color(1, 0.3, 1))
	_view.draw_rect(Rect2(Vector2.ZERO, size), Color(1, 1, 1, 0.5), false, 1.0)


func _color_count(tex: Texture2D) -> int:
	if _colors.has(tex):
		return _colors[tex]
	var n := 257
	var img := tex.get_image()
	if img:
		if img.is_compressed():
			img.decompress()
		img.convert(Image.FORMAT_RGBA8)
		var seen := {}
		for c in img.get_data().to_int32_array():
			seen[c] = true
			if seen.size() > 256:
				break
		n = seen.size()
	_colors[tex] = n
	return n


static func _material_textures(mat: Material) -> Array[Texture2D]:
	var out: Array[Texture2D] = []
	if mat is BaseMaterial3D and mat.albedo_texture:
		out.append(mat.albedo_texture)
	elif mat is ShaderMaterial and mat.shader:
		for u in mat.shader.get_shader_uniform_list():
			var v = mat.get_shader_parameter(u.name)
			if v is Texture2D and not out.has(v):
				out.append(v)
	return out


static func _in_visibility_range(gi: GeometryInstance3D, box: AABB, eye: Vector3) -> bool:
	var d := eye.distance_to(box.get_center())
	if gi.visibility_range_begin > 0.0 and d < gi.visibility_range_begin:
		return false
	return gi.visibility_range_end <= 0.0 or d < gi.visibility_range_end


static func _in_frustum(box: AABB, planes: Array[Plane]) -> bool:
	for p in planes:
		var corner := Vector3(
			box.position.x if p.normal.x > 0 else box.end.x,
			box.position.y if p.normal.y > 0 else box.end.y,
			box.position.z if p.normal.z > 0 else box.end.z,
		)
		if p.is_point_over(corner):
			return false
	return true
