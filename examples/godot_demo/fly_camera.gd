extends Camera3D
## Fly camera: hold the right mouse button to look, WASD to move, Q/E down/up,
## Shift to go faster.

@export var speed := 8.0
@export var mouse_sensitivity := 0.003

var _yaw := 0.0
var _pitch := 0.0


func _ready() -> void:
	_yaw = rotation.y
	_pitch = rotation.x


func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_RIGHT:
		Input.mouse_mode = (
			Input.MOUSE_MODE_CAPTURED if event.pressed else Input.MOUSE_MODE_VISIBLE
		)
	elif event is InputEventMouseMotion and Input.mouse_mode == Input.MOUSE_MODE_CAPTURED:
		_yaw -= event.relative.x * mouse_sensitivity
		_pitch = clampf(_pitch - event.relative.y * mouse_sensitivity, -1.4, 1.4)
		rotation = Vector3(_pitch, _yaw, 0.0)


func _process(delta: float) -> void:
	var dir := Vector3(
		float(Input.is_key_pressed(KEY_D)) - float(Input.is_key_pressed(KEY_A)),
		float(Input.is_key_pressed(KEY_E)) - float(Input.is_key_pressed(KEY_Q)),
		float(Input.is_key_pressed(KEY_S)) - float(Input.is_key_pressed(KEY_W)),
	)
	if dir == Vector3.ZERO:
		return
	var fast := 3.0 if Input.is_key_pressed(KEY_SHIFT) else 1.0
	translate(dir.normalized() * speed * fast * delta)
