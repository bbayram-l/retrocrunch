@tool
extends VBoxContainer
## retrocrunch scene budget dock: shows a budget.gd analysis as a summary and a table.

signal analyze_requested
signal node_picked(node: Node)

const Budget := preload("budget.gd")
const TARGETS := [3000, 6000]
const RED := Color(1.0, 0.45, 0.4)
const ORANGE := Color(1.0, 0.75, 0.35)

var target_option: OptionButton
var scene_camera: CheckBox
var summary: Label
var tree: Tree


func _init() -> void:
	name = "PS1 Budget"
	var bar := HBoxContainer.new()
	add_child(bar)
	var button := Button.new()
	button.text = "Analyze"
	button.tooltip_text = "Measure the open scene against PS1 limits"
	button.pressed.connect(func(): analyze_requested.emit())
	bar.add_child(button)
	target_option = OptionButton.new()
	target_option.add_item("3000 polys (60 fps)")
	target_option.add_item("6000 polys (30 fps)")
	target_option.select(1)
	target_option.tooltip_text = "Polygons a PS1 draws per frame at 60 or 30 fps"
	bar.add_child(target_option)
	scene_camera = CheckBox.new()
	scene_camera.text = "Scene camera"
	scene_camera.tooltip_text = "Measure from the scene's camera instead of the editor view"
	bar.add_child(scene_camera)
	summary = Label.new()
	summary.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	summary.text = "Press Analyze to measure the open scene."
	add_child(summary)
	tree = Tree.new()
	tree.columns = 6
	tree.column_titles_visible = true
	tree.hide_root = true
	tree.size_flags_vertical = Control.SIZE_EXPAND_FILL
	for c in 6:
		tree.set_column_title(c, ["Object", "Polys", "Tris", "KB", "Dist", "Note"][c])
		tree.set_column_expand(c, c == 0 or c == 5)
		if c in [1, 2, 3, 4]:
			tree.set_column_custom_minimum_width(c, 64)
	tree.item_selected.connect(_on_selected)
	add_child(tree)


func target() -> int:
	return TARGETS[target_option.selected]


func show_result(r: Dictionary) -> void:
	var goal := target()
	var lines := PackedStringArray()
	lines.append("In view: %d polys (%d tris) of %d -> PS1 about %d fps" % [r.view_polys, r.view_tris, goal, r.fps])
	lines.append("Whole scene: %d polys" % r.scene_polys)
	lines.append("Textures: %.1f KB of %d KB VRAM" % [r.vram_kb, Budget.VRAM_BUDGET_KB])
	if r.clip.is_empty():
		lines.append("The view fits the budget." if r.view_polys <= goal else "No camera: can't suggest what to clip.")
	else:
		lines.append("To fit, clip: " + ", ".join(r.clip.map(func(o): return str(o.node.name))))
	summary.text = "\n".join(lines)
	var over: bool = r.view_polys > goal or r.vram_kb > Budget.VRAM_BUDGET_KB
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
		_fill(item, [o.name, str(o.polys), str(o.tris), "%.1f" % kb, dist, "; ".join(o.notes)])
		item.set_metadata(0, o.node)
		item.set_tooltip_text(5, _explain(o.notes))
		var clipped: bool = o.notes.any(func(n): return n.begins_with("clip"))
		if clipped or not o.notes.is_empty() and o.in_view:
			for c in 6:
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
		_fill(item, [label, "", "", "%.1f" % info.kb, "", note])
		if info.compressed:
			item.set_custom_color(5, ORANGE)


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
		if c in [1, 2, 3, 4]:
			item.set_text_alignment(c, HORIZONTAL_ALIGNMENT_RIGHT)


func _on_selected() -> void:
	var node = tree.get_selected().get_metadata(0)
	if node is Node and is_instance_valid(node):
		node_picked.emit(node)
