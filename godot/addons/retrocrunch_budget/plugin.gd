@tool
extends EditorPlugin
## retrocrunch scene budget: an editor dock that measures the open 3D scene against PS1
## limits (polygons in view, texture VRAM) and suggests what to clip to fit.

const Budget := preload("budget.gd")
const Dock := preload("dock.gd")

var dock: Control


func _enter_tree() -> void:
	dock = Dock.new()
	dock.analyze_requested.connect(_analyze)
	dock.node_picked.connect(_select)
	add_control_to_dock(DOCK_SLOT_RIGHT_UL, dock)


func _exit_tree() -> void:
	if dock:
		remove_control_from_docks(dock)
		dock.queue_free()
		dock = null


func _analyze() -> void:
	var root := EditorInterface.get_edited_scene_root()
	if root == null:
		dock.summary.text = "Open a 3D scene first."
		return
	dock.show_result(Budget.analyze(root, _camera(root), dock.target()))


## The editor's 3D view camera, or the scene's own camera when the dock asks for it.
func _camera(root: Node) -> Camera3D:
	if dock.scene_camera.button_pressed:
		var cams := root.find_children("*", "Camera3D", true, false)
		for c: Camera3D in cams:
			if c.current:
				return c
		if not cams.is_empty():
			return cams[0]
	return EditorInterface.get_editor_viewport_3d(0).get_camera_3d()


func _select(node: Node) -> void:
	var sel := EditorInterface.get_selection()
	sel.clear()
	sel.add_node(node)
	EditorInterface.edit_node(node)
