# psx-forge Blender add-on: PS1-ify the selected objects without leaving Blender.
#
# Install: Edit > Preferences > Add-ons > Install from Disk... > pick this file.
# Then: 3D Viewport > Sidebar (N) > "PSX Forge".
#
# Exports the selection (with armatures, shape keys and actions) to a temporary GLB,
# runs `psx-forge model ... --json`, and imports the result into a "PSX_<name>"
# collection next to the original, so you can compare and keep iterating.

bl_info = {
    "name": "psx-forge",
    "author": "psx-forge",
    "version": (0, 3, 0),
    "blender": (4, 2, 0),
    "location": "3D Viewport > Sidebar > PSX Forge",
    "description": "Convert selected objects into PS1-style assets with psx-forge",
    "category": "Import-Export",
}

import json
import os
import shutil
import subprocess
import tempfile

import bpy
from bpy.props import BoolProperty, EnumProperty, FloatProperty, IntProperty, StringProperty


def find_psx_forge(prefs_path):
    if prefs_path and os.path.isfile(bpy.path.abspath(prefs_path)):
        return bpy.path.abspath(prefs_path)
    env = os.environ.get("PSXFORGE_EXE")
    if env and os.path.isfile(env):
        return env
    return shutil.which("psx-forge")


class PSXForgePreferences(bpy.types.AddonPreferences):
    bl_idname = __name__
    executable: StringProperty(name="psx-forge executable", subtype="FILE_PATH",
                               description="Path to psx-forge(.exe). Empty = PSXFORGE_EXE or PATH")

    def draw(self, context):
        self.layout.prop(self, "executable")
        found = find_psx_forge(self.executable)
        self.layout.label(text=f"Using: {found}" if found else "psx-forge not found", icon="CHECKMARK" if found else "ERROR")


class PSXForgeSettings(bpy.types.PropertyGroup):
    tris: IntProperty(name="Triangles", default=600, min=12, max=100000)
    tex_size: EnumProperty(name="Texture", default="128", items=[(s, f"{s} px", "") for s in ("16", "32", "64", "128", "256", "512", "1024")])
    colors: IntProperty(name="Colours", default=16, min=2, max=256)
    dither: EnumProperty(name="Dither", default="bayer", items=[("bayer", "Bayer 4x4", ""), ("ps1", "PS1 table", ""), ("off", "Off", "")])
    subdivide: FloatProperty(name="Split polygons over", default=0.0, min=0.0, description="Split edges longer than this (scene units) to limit affine warping. 0 = off")
    remove_hidden: BoolProperty(name="Remove hidden faces", default=True)
    ao: BoolProperty(name="Vertex AO", default=True)
    grounded: BoolProperty(name="Sits on ground", default=False)
    engine: EnumProperty(name="Engine", default="godot", items=[(e, e.title(), "") for e in ("godot", "unity", "unreal", "blender", "generic")])
    export_dir: StringProperty(name="Export folder", subtype="DIR_PATH", default="//psx_export")
    hide_original: BoolProperty(name="Hide original", default=True)
    last_report: StringProperty(default="")


def prefs_executable(context):
    addon = context.preferences.addons.get(__name__)
    return addon.preferences.executable if addon else ""


def selection_with_rigs(context):
    objs = set(context.selected_objects)
    for ob in list(objs):
        arm = ob.find_armature() if ob.type == "MESH" else None
        if arm:
            objs.add(arm)
    return objs


def export_selection_glb(context, path):
    objs = selection_with_rigs(context)
    if not objs:
        raise RuntimeError("Select at least one object")
    prev_sel = list(context.selected_objects)
    for ob in context.view_layer.objects:
        ob.select_set(ob in objs)
    try:
        bpy.ops.export_scene.gltf(filepath=path, export_format="GLB", use_selection=True, export_apply=True,
                                  export_animations=True, export_skins=True, export_morph=True)
    finally:
        for ob in context.view_layer.objects:
            ob.select_set(ob in prev_sel)


def run_psx_forge(exe, glb, out_dir, s, engine):
    cmd = [exe, "model", glb, "-o", out_dir, "--json", "--tris", str(s.tris), "--tex-size", s.tex_size,
           "--colors", str(s.colors), "--dither", s.dither, "--engine", engine, "--subdivide", str(s.subdivide)]
    if not s.remove_hidden:
        cmd.append("--keep-hidden")
    if not s.ao:
        cmd.append("--no-ao")
    if s.grounded:
        cmd.append("--grounded")
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=1800)
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip().splitlines()[-1] if proc.stderr.strip() else "psx-forge failed")
    return json.loads(proc.stdout)


def summarize(report):
    st = report["stats"]
    tex = st["textures"][0] if st["textures"] else None
    text = f"{st['triangles_before']} -> {st['triangles_after']} tris"
    if tex:
        text += f", {tex['width']}x{tex['height']} px, {tex['colors']} colours"
    return text


class PSXFORGE_OT_convert(bpy.types.Operator):
    bl_idname = "psxforge.convert_selected"
    bl_label = "PSX-ify Selected"
    bl_description = "Convert the selection with psx-forge and import the PS1 version next to it"
    bl_options = {"REGISTER", "UNDO"}

    def execute(self, context):
        exe = find_psx_forge(prefs_executable(context))
        if not exe:
            self.report({"ERROR"}, "psx-forge not found: set it in the add-on preferences")
            return {"CANCELLED"}
        s = context.scene.psx_forge
        tmp = tempfile.mkdtemp(prefix="psxforge_")
        name = bpy.path.clean_name(context.active_object.name if context.active_object else "selection")
        glb = os.path.join(tmp, f"{name}.glb")
        try:
            export_selection_glb(context, glb)
            report = run_psx_forge(exe, glb, tmp, s, "blender")
        except Exception as e:  # noqa: BLE001 - surface any failure to the user
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        result = next(f for f in report["files"] if f.endswith(".glb"))
        before = set(bpy.data.objects)
        coll = bpy.data.collections.new(f"PSX_{name}")
        context.scene.collection.children.link(coll)
        bpy.ops.import_scene.gltf(filepath=result)
        for ob in set(bpy.data.objects) - before:
            # Leave the importer's hidden helpers (bone-shape icosphere) where they are.
            if any(c.name.startswith("glTF_not_exported") for c in ob.users_collection):
                continue
            for c in ob.users_collection:
                c.objects.unlink(ob)
            coll.objects.link(ob)
        if s.hide_original:
            for ob in selection_with_rigs(context):
                ob.hide_set(True)
        s.last_report = summarize(report)
        for w in report.get("warnings", []):
            self.report({"WARNING"}, w)
        self.report({"INFO"}, s.last_report)
        return {"FINISHED"}


class PSXFORGE_OT_export(bpy.types.Operator):
    bl_idname = "psxforge.export_selected"
    bl_label = "Export for Engine"
    bl_description = "Convert the selection and write engine-ready files to the export folder"

    def execute(self, context):
        exe = find_psx_forge(prefs_executable(context))
        if not exe:
            self.report({"ERROR"}, "psx-forge not found: set it in the add-on preferences")
            return {"CANCELLED"}
        s = context.scene.psx_forge
        out_dir = bpy.path.abspath(s.export_dir)
        os.makedirs(out_dir, exist_ok=True)
        name = bpy.path.clean_name(context.active_object.name if context.active_object else "selection")
        glb = os.path.join(tempfile.mkdtemp(prefix="psxforge_"), f"{name}.glb")
        try:
            export_selection_glb(context, glb)
            report = run_psx_forge(exe, glb, out_dir, s, s.engine)
        except Exception as e:  # noqa: BLE001
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        s.last_report = summarize(report) + f" -> {report['output_dir']}"
        self.report({"INFO"}, s.last_report)
        return {"FINISHED"}


class PSXFORGE_OT_open_gui(bpy.types.Operator):
    bl_idname = "psxforge.open_gui"
    bl_label = "Open in psx-forge"
    bl_description = "Export the selection and open it in the psx-forge app for interactive tuning"

    def execute(self, context):
        exe = find_psx_forge(prefs_executable(context))
        if not exe:
            self.report({"ERROR"}, "psx-forge not found")
            return {"CANCELLED"}
        name = bpy.path.clean_name(context.active_object.name if context.active_object else "selection")
        glb = os.path.join(tempfile.mkdtemp(prefix="psxforge_"), f"{name}.glb")
        try:
            export_selection_glb(context, glb)
        except Exception as e:  # noqa: BLE001
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}
        subprocess.Popen([exe, glb])
        return {"FINISHED"}


class PSXFORGE_PT_panel(bpy.types.Panel):
    bl_label = "PSX Forge"
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "PSX Forge"

    def draw(self, context):
        s = context.scene.psx_forge
        col = self.layout.column(align=True)
        for prop in ("tris", "tex_size", "colors", "dither", "subdivide"):
            col.prop(s, prop)
        col = self.layout.column(align=True)
        col.prop(s, "remove_hidden")
        col.prop(s, "ao")
        col.prop(s, "grounded")
        col.prop(s, "hide_original")
        self.layout.operator("psxforge.convert_selected", icon="SHADING_TEXTURE")
        box = self.layout.box()
        box.prop(s, "engine")
        box.prop(s, "export_dir")
        box.operator("psxforge.export_selected", icon="EXPORT")
        self.layout.operator("psxforge.open_gui", icon="WINDOW")
        if s.last_report:
            self.layout.label(text=s.last_report, icon="INFO")


CLASSES = (PSXForgePreferences, PSXForgeSettings, PSXFORGE_OT_convert, PSXFORGE_OT_export, PSXFORGE_OT_open_gui, PSXFORGE_PT_panel)


def register():
    for c in CLASSES:
        bpy.utils.register_class(c)
    bpy.types.Scene.psx_forge = bpy.props.PointerProperty(type=PSXForgeSettings)


def unregister():
    del bpy.types.Scene.psx_forge
    for c in reversed(CLASSES):
        bpy.utils.unregister_class(c)


if __name__ == "__main__":
    register()
