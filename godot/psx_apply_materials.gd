@tool
extends Node3D
## psx-forge: assigns the PSX ShaderMaterials to every mesh surface of this imported
## scene, matched by material name. Used for rigged models, where Godot's glTF importer
## moves skinned meshes under the Skeleton3D, so fixed node paths can't be relied on.

@export var psx_materials: Array[Material] = []:
	set(value):
		psx_materials = value
		if is_inside_tree():
			_apply()


func _ready() -> void:
	_apply()


func _apply() -> void:
	var by_name := {}
	for m in psx_materials:
		if m != null:
			by_name[m.resource_name] = m
	for node in find_children("*", "MeshInstance3D", true, false):
		var mi := node as MeshInstance3D
		if mi.mesh == null:
			continue
		for i in mi.mesh.get_surface_count():
			var src := mi.mesh.surface_get_material(i)
			var key: String = src.resource_name if src != null else ""
			if by_name.has(key):
				mi.set_surface_override_material(i, by_name[key])
			elif psx_materials.size() == 1 and psx_materials[0] != null:
				mi.set_surface_override_material(i, psx_materials[0])
