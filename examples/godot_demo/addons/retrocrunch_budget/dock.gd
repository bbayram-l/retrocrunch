@tool
extends VBoxContainer
## retrocrunch scene budget dock: shows a budget.gd analysis as a summary and a table.

signal analyze_requested
signal node_picked(node: Node)

const Budget := preload("budget.gd")
const RED := Color(1.0, 0.45, 0.4)
const ORANGE := Color(1.0, 0.75, 0.35)
const VRAM_MB := [1, 2, 4, 8]
## PS1 display modes (16-bit framebuffers). 640x480 is the interlaced hi-res mode.
const RESOLUTIONS := [
	Vector2i(256, 240), Vector2i(320, 240), Vector2i(368, 240),
	Vector2i(512, 240), Vector2i(640, 480),
]
## Settings are kept per project in the editor's metadata, not in project.godot.
const META := "retrocrunch_budget"
## Objects named in the "To fit" line; the rest are marked in the list.
const CLIP_NAMES_SHOWN := 4

var polys_box: SpinBox
var vram_option: OptionButton
var buffers_option: OptionButton
var resolution_option: OptionButton
var scene_camera: CheckBox
var summary: Label
var tree: Tree
var _has_result := false


func _init() -> void:
	name = "PS1 Budget"
	var bar := HBoxContainer.new()
	add_child(bar)
	var button := Button.new()
	button.text = "Analyze"
	button.tooltip_text = "Measure the open scene against PS1 limits"
	button.pressed.connect(func(): analyze_requested.emit())
	bar.add_child(button)
	scene_camera = CheckBox.new()
	scene_camera.text = "Scene camera"
	scene_camera.tooltip_text = "Measure from the scene's camera instead of the editor view"
	bar.add_child(scene_camera)

	var grid := GridContainer.new()
	grid.columns = 2
	add_child(grid)
	polys_box = SpinBox.new()
	polys_box.min_value = 100
	polys_box.max_value = 200000
	polys_box.step = 100
	polys_box.value = _load("polys", 6000)
	polys_box.suffix = "polys"
	polys_box.tooltip_text = "Polygons in view per frame: 3000 runs at 60 fps on a PS1, 6000 at 30"
	_row(grid, "Budget", polys_box)
	vram_option = _option(["1 MB (PS1)", "2 MB", "4 MB", "8 MB"], "vram", 0,
		"VRAM size: 1 MB is a PS1; more for arcade boards or a higher-detail mode")
	_row(grid, "VRAM", vram_option)
	buffers_option = _option(["0", "1", "2 (double buffered)"], "buffers", 2,
		"Framebuffers kept in VRAM: most games used 2, some 1; 0 frees all of it for textures")
	_row(grid, "Framebuffers", buffers_option)
	var names: Array = RESOLUTIONS.map(func(r): return "%dx%d" % [r.x, r.y])
	resolution_option = _option(names, "resolution", 1, "Framebuffer size (16-bit)")
	_row(grid, "Resolution", resolution_option)
	polys_box.value_changed.connect(func(v): _changed("polys", int(v)))

	summary = Label.new()
	summary.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	summary.text = "Press Analyze to measure the open scene."
	add_child(summary)
	tree = Tree.new()
	tree.columns = 5
	tree.column_titles_visible = true
	tree.hide_root = true
	tree.size_flags_vertical = Control.SIZE_EXPAND_FILL
	for c in 5:
		tree.set_column_title(c, ["Object", "Polys", "KB", "Dist", "Note"][c])
		# Names and notes share the spare width; numbers stay just wide enough.
		tree.set_column_expand(c, c == 0 or c == 4)
		tree.set_column_clip_content(c, c == 0 or c == 4)
		if c in [1, 2, 3]:
			var width: int = {1: 64, 2: 48, 3: 56}[c]
			tree.set_column_custom_minimum_width(c, roundi(width * _ui_scale()))
	tree.item_selected.connect(_on_selected)
	add_child(tree)


## The editor's display scale (1 outside the editor).
func _ui_scale() -> float:
	if Engine.is_editor_hint() and Engine.has_singleton("EditorInterface"):
		return Engine.get_singleton("EditorInterface").get_editor_scale()
	return 1.0


func _row(grid: GridContainer, label: String, control: Control) -> void:
	var l := Label.new()
	l.text = label
	grid.add_child(l)
	control.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	grid.add_child(control)


func _option(items: Array, key: String, default: int, tip: String) -> OptionButton:
	var o := OptionButton.new()
	for item in items:
		o.add_item(item)
	o.select(clampi(_load(key, default), 0, items.size() - 1))
	o.tooltip_text = tip
	o.item_selected.connect(func(i): _changed(key, i))
	return o


func _settings() -> EditorSettings:
	if Engine.is_editor_hint() and Engine.has_singleton("EditorInterface"):
		return Engine.get_singleton("EditorInterface").get_editor_settings()
	return null


func _load(key: String, default: int) -> int:
	var s := _settings()
	return int(s.get_project_metadata(META, key, default)) if s else default


## Save the setting and, if results are showing, measure again with it.
func _changed(key: String, value: int) -> void:
	var s := _settings()
	if s:
		s.set_project_metadata(META, key, value)
	if _has_result:
		analyze_requested.emit()


func target() -> int:
	return int(polys_box.value)


## Texture memory left after the framebuffers, from the VRAM settings.
func vram_budget_kb() -> float:
	return Budget.vram_budget_kb(
		VRAM_MB[vram_option.selected], buffers_option.selected,
		RESOLUTIONS[resolution_option.selected]
	)


func show_result(r: Dictionary) -> void:
	_has_result = true
	var goal := target()
	var budget_kb := vram_budget_kb()
	var lines := PackedStringArray()
	lines.append("In view: %d polys (%d tris) of %d -> PS1 about %d fps" % [r.view_polys, r.view_tris, goal, r.fps])
	lines.append("Whole scene: %d polys" % r.scene_polys)
	lines.append("Textures: %.1f KB of %.0f KB free VRAM" % [r.vram_kb, maxf(budget_kb, 0.0)])
	if budget_kb < 0.0:
		lines.append("The framebuffers alone need %.0f KB more than this VRAM." % -budget_kb)
	if r.clip.is_empty():
		lines.append("The view fits the budget." if r.view_polys <= goal else "No camera: can't suggest what to clip.")
	else:
		# Farthest first; a big scene can need over a hundred, so name only a few.
		var names: Array = r.clip.map(func(o): return str(o.node.name))
		var nearest: float = r.clip.map(func(o): return o.distance).min()
		var shown := ", ".join(names.slice(0, CLIP_NAMES_SHOWN))
		if names.size() > CLIP_NAMES_SHOWN:
			shown += " and %d more (red in the list)" % (names.size() - CLIP_NAMES_SHOWN)
		lines.append("To fit, draw only up to %.1f m: clip %s" % [nearest, shown])
	summary.text = "\n".join(lines)
	var over: bool = r.view_polys > goal or r.vram_kb > budget_kb
	summary.add_theme_color_override("font_color", RED if over else Color(0.6, 1.0, 0.6))

	tree.clear()
	var root := tree.create_item()
	var objects := tree.create_item(root)
	objects.set_text(0, "Objects (%d)" % r.objects.size())
	for o in r.objects:
		var item := tree.create_item(objects)
		var kb := 0.0
		for t in o.textures:
			kb += r.textures[t].kb
		var dist := "-" if o.distance < 0.0 else "%.1f" % o.distance
		if not o.in_view:
			dist += " (off)"
		_fill(item, [o.name, str(o.polys), "%.1f" % kb, dist, "; ".join(o.notes)])
		item.set_tooltip_text(1, "%d polygons (%d triangles)" % [o.polys, o.tris])
		item.set_metadata(0, o.node)
		item.set_tooltip_text(0, o.name)
		item.set_tooltip_text(4, _explain(o.notes))
		var clipped: bool = o.notes.any(func(n): return n.begins_with("clip"))
		if clipped or not o.notes.is_empty() and o.in_view:
			for c in 5:
				item.set_custom_color(c, RED if clipped else ORANGE)
	var textures := tree.create_item(root)
	textures.set_text(0, "Textures (%d)" % r.textures.size())
	for t: Texture2D in r.textures:
		var info: Dictionary = r.textures[t]
		var note := "%dx%d, %d-bit" % [info.width, info.height, info.bits]
		if info.compressed:
			note += "; GPU-compressed import: re-import lossless for exact colours"
		var label := t.resource_path.get_file() if t.resource_path else str(t)
		var item := tree.create_item(textures)
		_fill(item, [label, "", "%.1f" % info.kb, "", note])
		if info.compressed:
			item.set_custom_color(4, ORANGE)


## Long form of the short notes, for the tooltip.
func _explain(notes: Array) -> String:
	var out := PackedStringArray()
	for n: String in notes:
		if n.begins_with("clip"):
			out.append("Clip it from here to fit the budget: set its visibility_range_end to about %s m (or fade it out with visibility_range_fade_mode)." % n.get_slice(" ", 2))
		elif n.begins_with("small"):
			out.append("Small on screen for its polygon count: a retrocrunch LOD (lods option) or a lower budget would look the same.")
		else:
			out.append("One of the three most detailed objects in the scene.")
	return "
".join(out)


func _fill(item: TreeItem, cells: Array) -> void:
	for c in cells.size():
		item.set_text(c, cells[c])
		if c in [1, 2, 3]:
			item.set_text_alignment(c, HORIZONTAL_ALIGNMENT_RIGHT)


func _on_selected() -> void:
	var node = tree.get_selected().get_metadata(0)
	if node is Node and is_instance_valid(node):
		node_picked.emit(node)
