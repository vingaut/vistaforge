# -*- coding: utf-8 -*-
"""VistaForge <-> Blender camera animation bridge.

Install as a Blender add-on (Edit > Preferences > Add-ons > Install, pick this
file) or run it from the Scripting workspace. It adds:

* File > Import > VistaForge Camera Animation (.glb)
* File > Export > VistaForge Camera Animation (.vfcam)

The GLB importer selects the VistaForge camera, restores the scene frame range
and frame rate from the embedded rig, and leaves the target empty animated. The
.vfcam exporter samples the active camera (and an optional target empty) once
per frame and writes the JSON document VistaForge reads with
``rig import``. No terrain is required in either direction.
"""

import json
import math
import os
import struct

import bpy
from mathutils import Vector
from bpy_extras.io_utils import ExportHelper, ImportHelper
from bpy.props import BoolProperty, IntProperty, StringProperty

bl_info = {
    "name": "VistaForge Camera Animation",
    "author": "VistaForge contributors",
    "version": (1, 0, 0),
    "blender": (4, 0, 0),
    "location": "File > Import/Export",
    "description": "Import/export VistaForge camera and target trajectories (.glb/.vfcam)",
    "category": "Import-Export",
}

ANIMATION_FORMAT = "vistaforge.camera-animation.v1"


def read_glb_json(path):
    """Return the JSON chunk of a GLB without decoding the binary buffer."""
    with open(path, "rb") as handle:
        header = handle.read(12)
        if len(header) < 12 or header[:4] != b"glTF":
            return None
        while True:
            chunk_header = handle.read(8)
            if len(chunk_header) < 8:
                break
            length, kind = struct.unpack("<II", chunk_header)
            data = handle.read(length)
            if kind == 0x4E4F534A:  # JSON
                return json.loads(data.decode("utf-8"))
            # keep reading (BIN chunk is skipped)
    return None


def vistaforge_rig_from_glb(path):
    document = read_glb_json(path)
    if not document:
        return None
    scenes = document.get("scenes") or []
    if not scenes:
        return None
    extras = scenes[0].get("extras") or {}
    return extras.get("vistaforge_rig")


def find_vistaforge_camera():
    cameras = [obj for obj in bpy.context.scene.objects if obj.type == "CAMERA"]
    named = [obj for obj in cameras if "vistaforge" in obj.name.lower() or "vistaforge" in obj.name.lower()]
    if named:
        return named[0]
    return cameras[0] if cameras else None


def select_camera(camera):
    if camera is None:
        return
    bpy.context.scene.camera = camera
    for obj in bpy.context.selected_objects:
        obj.select_set(False)
    camera.select_set(True)
    bpy.context.view_layer.objects.active = camera


def action_range(objects):
    start = None
    end = None
    for obj in objects:
        if not obj.animation_data or not obj.animation_data.action:
            continue
        frame_start, frame_end = obj.animation_data.action.frame_range
        start = frame_start if start is None else min(start, frame_start)
        end = frame_end if end is None else max(end, frame_end)
    return start, end


def configure_scene(camera, rig):
    scene = bpy.context.scene
    scene.camera = camera
    if rig:
        fps = int(round(float(rig.get("fps", 24))))
        if fps > 0:
            scene.render.fps = fps
            scene.render.fps_base = 1.0
        duration = float(rig.get("duration_s", 0.0))
        if duration > 0.0:
            scene.frame_start = 0
            scene.frame_end = max(1, int(round(duration * max(1, fps))))
    start, end = action_range([obj for obj in scene.objects if obj.animation_data])
    if start is not None and end is not None:
        scene.frame_start = max(0, int(round(start)))
        scene.frame_end = max(scene.frame_start + 1, int(round(end)))
    scene.frame_set(scene.frame_start)
    if camera:
        camera.data.clip_end = max(camera.data.clip_end, 200000.0)


class VISTAFORGE_OT_import_camera_animation(bpy.types.Operator, ImportHelper):
    bl_idname = "vistaforge.import_camera_animation"
    bl_label = "Import VistaForge Camera Animation"
    bl_description = "Import a .glb flyover (camera + target), select the camera and restore the frame range"
    bl_options = {"REGISTER", "UNDO"}

    filename_ext = ".glb"
    filter_glob: StringProperty(default="*.glb", options={"HIDDEN"})
    configure_scene: BoolProperty(
        name="Configure scene",
        description="Select the camera and set the frame range / frame rate",
        default=True,
    )
    replace_existing_camera: BoolProperty(
        name="Replace existing VistaForge camera",
        description="Remove a static VistaForge Camera/Target from the scene export before importing the animation",
        default=True,
    )

    def execute(self, context):
        rig = vistaforge_rig_from_glb(self.filepath)
        if self.replace_existing_camera:
            for obj in list(context.scene.objects):
                if obj.type == "CAMERA" and obj.name.startswith("VistaForge Camera"):
                    bpy.data.objects.remove(obj, do_unlink=True)
                elif obj.type == "EMPTY" and obj.name.startswith("VistaForge Target"):
                    bpy.data.objects.remove(obj, do_unlink=True)
        # Set the frame rate before importing so glTF times map to the right
        # number of frames.
        if rig:
            fps = int(round(float(rig.get("fps", 24))))
            if fps > 0:
                context.scene.render.fps = fps
                context.scene.render.fps_base = 1.0
        before = set(context.scene.objects)
        bpy.ops.import_scene.gltf(filepath=self.filepath)
        imported = [obj for obj in context.scene.objects if obj not in before]
        camera = None
        for obj in imported:
            if obj.type == "CAMERA":
                camera = obj
                break
        if camera is None:
            camera = find_vistaforge_camera()
        if self.configure_scene:
            configure_scene(camera, rig)
        self.report(
            {"INFO"},
            "VistaForge animation imported"
            + (f" ({os.path.basename(self.filepath)})" if self.filepath else ""),
        )
        return {"FINISHED"}


def _path_key(time_s, position):
    return {
        "time_s": float(time_s),
        "position": [float(position[0]), float(position[1]), float(position[2])],
        "in_handle": [0.0, 0.0, 0.0],
        "out_handle": [0.0, 0.0, 0.0],
        "ease": 0.0,
    }


def _manual_body():
    return {
        "preset": "manual",
        "max_speed_mps": 200.0,
        "accel_forward_mps2": 200.0,
        "accel_lateral_mps2": 200.0,
        "accel_vertical_mps2": 200.0,
        "jerk_mps3": 500.0,
        "drag": 0.0,
        "gravity": 0.0,
        "bank_gain": 0.0,
        "max_bank_deg": 0.0,
        "roll_response": 1.0,
        "pitch_response": 8.0,
        "look_lag": 0.0,
        "shake_deg": 0.0,
        "turbulence": 0.0,
        "manual_orientation": True,
    }


def _optics(camera):
    try:
        fov = math.degrees(camera.data.angle_y)
    except Exception:
        fov = math.degrees(camera.data.angle)
    return {
        "link": "eye",
        "fov_y_deg": max(1.0, min(170.0, float(fov))),
        "near": float(camera.data.clip_start),
        "far": float(camera.data.clip_end),
        "spring_stiffness": 6.0,
        "spring_damping": 0.8,
        "offset": [0.0, 0.0, 0.0],
        "roll_deg": 0.0,
        "dof_enabled": False,
        "focus_distance_m": 100.0,
        "fstop": 8.0,
        "focus_on_target": True,
    }


class VISTAFORGE_OT_export_camera_animation(bpy.types.Operator, ExportHelper):
    bl_idname = "vistaforge.export_camera_animation"
    bl_label = "Export VistaForge Camera Animation"
    bl_description = "Sample the active camera and optional target empty, then write a .vfcam file"
    bl_options = {"REGISTER"}

    filename_ext = ".vfcam"
    filter_glob: StringProperty(default="*.vfcam", options={"HIDDEN"})
    use_scene_range: BoolProperty(
        name="Use scene frame range",
        description="Export scene.frame_start..frame_end; disable to use the action range",
        default=True,
    )
    step: IntProperty(
        name="Frame step",
        description="Sample every N frames (1 = every frame)",
        default=1,
        min=1,
        max=30,
    )

    def _target(self, camera):
        candidates = [
            obj
            for obj in bpy.context.selected_objects
            if obj.type == "EMPTY" and obj != camera
        ]
        if not candidates:
            candidates = [
                obj
                for obj in bpy.context.scene.objects
                if obj.type == "EMPTY"
                and ("target" in obj.name.lower() or "cible" in obj.name.lower())
            ]
        return candidates[0] if candidates else None

    def execute(self, context):
        scene = context.scene
        camera = scene.camera
        if camera is None:
            cameras = [obj for obj in context.selected_objects if obj.type == "CAMERA"]
            camera = cameras[0] if cameras else None
        if camera is None:
            self.report({"ERROR"}, "Select a camera or set scene.camera first")
            return {"CANCELLED"}
        target = self._target(camera)
        if self.use_scene_range:
            start, end = scene.frame_start, scene.frame_end
        else:
            start, end = action_range([camera] + ([target] if target else []))
            if start is None:
                start, end = scene.frame_start, scene.frame_end
        step = max(1, int(self.step))
        fps = max(1, int(round(scene.render.fps / max(1e-6, scene.render.fps_base))))
        original_frame = scene.frame_current
        frames = []
        for frame in range(int(start), int(end) + 1, step):
            scene.frame_set(frame)
            matrix = camera.matrix_world
            position = matrix.translation
            if target is not None:
                aim = target.matrix_world.translation
            else:
                forward = matrix.to_quaternion() @ Vector((0.0, 0.0, -1.0))
                aim = position + forward * 500.0
            frames.append(
                (
                    (frame - start) / fps,
                    (position.x, position.y, position.z),
                    (aim.x, aim.y, aim.z),
                )
            )
        scene.frame_set(original_frame)
        if len(frames) < 2:
            self.report({"ERROR"}, "The frame range must contain at least two frames")
            return {"CANCELLED"}
        duration = frames[-1][0]
        eye_keys = [_path_key(time, position) for time, position, _ in frames]
        target_keys = [_path_key(time, aim) for time, _, aim in frames]
        document = {
            "format": ANIMATION_FORMAT,
            "version": 1,
            "name": f"{camera.name} (Blender)",
            "rig": {
                "enabled": True,
                "name": f"{camera.name} (Blender)",
                "duration_s": max(0.1, duration),
                "fps": fps,
                "eye": {"keys": eye_keys, "closed": False},
                "target": {
                    "mode": "path",
                    "fixed": [0.0, 0.0, 0.0],
                    "path": {"keys": target_keys, "closed": False},
                    "lead_s": 0.35,
                    "offset": [0.0, 0.0, 0.0],
                    "smoothing": 0.0,
                },
                "optics": _optics(camera),
                "body": _manual_body(),
                "constraints": {
                    "enabled": False,
                    "reference_enabled": False,
                    "reference_z_m": 0.0,
                    "ground_clearance_m": 25.0,
                    "wall_distance_m": 40.0,
                    "wall_avoidance": False,
                    "apply_eye": True,
                    "apply_camera": True,
                    "apply_target": False,
                    "softness": 0.35,
                    "look_ahead_s": 0.6,
                },
                "seed": 7,
            },
            "terrain": None,
        }
        os.makedirs(os.path.dirname(os.path.abspath(self.filepath)) or ".", exist_ok=True)
        with open(self.filepath, "w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2)
        self.report(
            {"INFO"},
            f"Wrote {len(frames)} frames ({duration:.2f}s) to {self.filepath}",
        )
        return {"FINISHED"}


def menu_import(self, context):
    self.layout.operator(
        VISTAFORGE_OT_import_camera_animation.bl_idname,
        text="VistaForge Camera Animation (.glb)",
    )


def menu_export(self, context):
    self.layout.operator(
        VISTAFORGE_OT_export_camera_animation.bl_idname,
        text="VistaForge Camera Animation (.vfcam)",
    )


CLASSES = (
    VISTAFORGE_OT_import_camera_animation,
    VISTAFORGE_OT_export_camera_animation,
)


def register():
    for cls in CLASSES:
        bpy.utils.register_class(cls)
    bpy.types.TOPBAR_MT_file_import.append(menu_import)
    bpy.types.TOPBAR_MT_file_export.append(menu_export)


def unregister():
    bpy.types.TOPBAR_MT_file_import.remove(menu_import)
    bpy.types.TOPBAR_MT_file_export.remove(menu_export)
    for cls in reversed(CLASSES):
        bpy.utils.unregister_class(cls)


if __name__ == "__main__":
    register()
