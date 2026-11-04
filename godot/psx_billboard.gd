extends AnimatedSprite3D
## retrocrunch billboard (experimental): shows the sprite-sheet row that faces the camera,
## like Doom-style 2.5D characters. Row d was rendered from yaw d * 360 / directions
## around the model's +Y axis, starting at the front (+Z). Rotate this node (or its
## parent) to turn the character; the animation frame carries over between rows.

@export var directions := 8


func _process(_delta: float) -> void:
	var cam := get_viewport().get_camera_3d()
	if cam == null or directions < 1:
		return
	# Camera direction in this node's own space: yaw = atan2(x, z), like the renderer.
	var local := global_transform.basis.inverse() * (cam.global_position - global_position)
	if Vector2(local.x, local.z).length_squared() < 1e-8:
		return
	var yaw := atan2(local.x, local.z)
	var row := posmod(int(round(yaw / (TAU / directions))), directions)
	var anim := StringName("dir%d" % row)
	if animation != anim and sprite_frames != null and sprite_frames.has_animation(anim):
		var f := frame
		var p := frame_progress
		play(anim)
		set_frame_and_progress(f, p)
