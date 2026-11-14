#!/usr/bin/env python3
"""Import a VistaForge GLB export into Blender and set up a Cycles render.

Usage (headless):
    blender --background --python vistaforge_import.py -- \
        scene.glb scene.json --render out.png --samples 32

Options after `--`:
    --render PATH     render a still to PATH
    --samples N       Cycles samples (default 32)
    --width / --height
    --gpu             try OptiX/CUDA
    --save-blend PATH save the .blend
    --smart-uv        run Smart UV Project on the terrain
    --remesh          add a voxel Remesh modifier (destructive, preview only)
    --validate        import and print a summary, no render

The GLB carries the actual render mesh (continuous detail included), vertex
colours, material slots, camera and sun. The sidecar carries palette, sun
direction, camera optics, height map and render settings.
"""

import json
import math
import os
import sys

import bpy
from mathutils import Vector


def parse_args() -> dict:
    argv = sys.argv
    if "--" in argv:
        argv = argv[argv.index("--") + 1 :]
    else:
        argv = []
    if len(argv) < 2:
        raise SystemExit("usage: -- scene.glb scene.json [options]")
    opts = {
        "glb": argv[0],
        "sidecar": argv[1],
        "render": None,
        "samples": 32,
        "width": None,
        "height": None,
        "gpu": False,
        "save_blend": None,
        "smart_uv": False,
        "remesh": False,
        "validate": False,
    }
    i = 2
    while i < len(argv):
        key = argv[i]
        if key in ("--render", "--save-blend"):
            opts[key[2:].replace("-", "_")] = argv[i + 1]
            i += 2
        elif key in ("--samples", "--width", "--height"):
            opts[key[2:]] = int(argv[i + 1])
            i += 2
        elif key == "--gpu":
            opts["gpu"] = True
            i += 1
        elif key == "--smart-uv":
            opts["smart_uv"] = True
            i += 1
        elif key == "--remesh":
            opts["remesh"] = True
            i += 1
        elif key == "--validate":
            opts["validate"] = True
            i += 1
        else:
            raise SystemExit(f"unknown option {key}")
    return opts


def load_sidecar(path: str) -> dict:
    with open(path, "r", encoding="utf-8") as handle:
        return json.load(handle)


def clear_scene() -> None:
    bpy.ops.wm.read_factory_settings(use_empty=True)


def scene_units() -> None:
    scene = bpy.context.scene
    scene.unit_settings.system = "METRIC"
    scene.unit_settings.scale_length = 1.0
    try:
        scene.view_settings.view_transform = "AgX"
    except Exception:
        try:
            scene.view_settings.view_transform = "Filmic"
        except Exception:
            pass


def mesh_objects():
    return [obj for obj in bpy.context.scene.objects if obj.type == "MESH"]


def first_color_attribute(mesh):
    if not mesh.color_attributes:
        return None
    return mesh.color_attributes[0].name


def principled(mat):
    if not mat.use_nodes:
        return None
    for node in mat.node_tree.nodes:
        if node.type == "BSDF_PRINCIPLED":
            return node
    return None


def add_vertex_color_multiply(mat, layer_name: str) -> None:
    node = principled(mat)
    if node is None or layer_name is None:
        return
    tree = mat.node_tree
    # glTF base colour factor is already in the Principled node. Multiply it
    # by the exported per-vertex albedo so the Blender view matches the
    # rasteriser even before any texture is assigned.
    vc = tree.nodes.new("ShaderNodeVertexColor")
    vc.layer_name = layer_name
    vc.location = (node.location.x - 420, node.location.y + 180)
    mix = tree.nodes.new("ShaderNodeMixRGB")
    mix.blend_type = "MULTIPLY"
    mix.inputs["Fac"].default_value = 1.0
    mix.location = (node.location.x - 220, node.location.y + 140)
    base = node.inputs["Base Color"]
    if base.is_linked:
        source = base.links[0].from_socket
        tree.links.remove(base.links[0])
    else:
        source = None
    if source is not None:
        tree.links.new(source, mix.inputs["Color1"])
    else:
        mix.inputs["Color1"].default_value = base.default_value
    tree.links.new(vc.outputs["Color"], mix.inputs["Color2"])
    tree.links.new(mix.outputs["Color"], base)


def add_procedural_detail(mat, kind: str) -> None:
    node = principled(mat)
    if node is None:
        return
    tree = mat.node_tree
    noise = tree.nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = {
        "rock": 7.0,
        "strata": 9.0,
        "scree": 22.0,
        "snow": 3.0,
        "vegetation": 14.0,
        "water": 2.0,
    }.get(kind, 8.0)
    noise.inputs["Detail"].default_value = 8.0
    noise.inputs["Roughness"].default_value = 0.65
    noise.location = (node.location.x - 520, node.location.y - 220)
    bump = tree.nodes.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = {
        "rock": 0.28,
        "strata": 0.22,
        "scree": 0.18,
        "snow": 0.08,
        "vegetation": 0.10,
        "water": 0.05,
    }.get(kind, 0.15)
    bump.inputs["Distance"].default_value = 0.08
    bump.location = (node.location.x - 260, node.location.y - 260)
    tree.links.new(noise.outputs["Fac"], bump.inputs["Height"])
    normal = node.inputs["Normal"]
    tree.links.new(bump.outputs["Normal"], normal)


def load_image(directory: str, name: str, colorspace: str):
    if not name:
        return None
    path = os.path.join(directory, name)
    if not os.path.exists(path):
        return None
    image = bpy.data.images.load(path, check_existing=True)
    try:
        image.colorspace_settings.name = colorspace
    except Exception:
        pass
    return image


def add_image_texture(mat, image, label: str, non_color: bool):
    node = principled(mat)
    if node is None or image is None:
        return None
    tree = mat.node_tree
    tex = tree.nodes.new("ShaderNodeTexImage")
    tex.image = image
    tex.label = label
    tex.interpolation = "Smart"
    tex.extension = "EXTEND"
    tex.location = (node.location.x - 650, node.location.y - 40)
    if non_color:
        try:
            image.colorspace_settings.name = "Non-Color"
        except Exception:
            pass
    return tex


def add_normal_map(mat, image) -> None:
    node = principled(mat)
    tex = add_image_texture(mat, image, "VistaForge normal", True)
    if node is None or tex is None:
        return
    tree = mat.node_tree
    normal_map = tree.nodes.new("ShaderNodeNormalMap")
    normal_map.location = (node.location.x - 300, node.location.y - 180)
    tree.links.new(tex.outputs["Color"], normal_map.inputs["Color"])
    tree.links.new(normal_map.outputs["Normal"], node.inputs["Normal"])


def enhance_materials(sidecar: dict, texture_dir: str) -> None:
    kinds = {m["name"]: m.get("kind", "rock") for m in sidecar.get("materials", [])}
    textures = sidecar.get("textures", {})
    albedo = load_image(texture_dir, textures.get("albedo"), "sRGB")
    normal = load_image(texture_dir, textures.get("normal"), "Non-Color")
    masks = load_image(texture_dir, textures.get("masks"), "Non-Color")
    for obj in mesh_objects():
        layer = first_color_attribute(obj.data) if obj.data else None
        for slot in obj.material_slots:
            mat = slot.material
            if mat is None:
                continue
            mat.use_nodes = True
            kind = kinds.get(mat.name, "rock")
            node = principled(mat)
            if kind != "water" and albedo is not None:
                tex = add_image_texture(mat, albedo, "VistaForge albedo", False)
                node = principled(mat)
                if tex is not None and node is not None:
                    tree = mat.node_tree
                    base = node.inputs["Base Color"]
                    for link in list(base.links):
                        tree.links.remove(link)
                    tree.links.new(tex.outputs["Color"], base)
            elif layer:
                add_vertex_color_multiply(mat, layer)
            if kind != "water" and normal is not None:
                add_normal_map(mat, normal)
            else:
                add_procedural_detail(mat, kind)
            if masks is not None and kind != "water":
                mask_tex = add_image_texture(mat, masks, "VistaForge masks R=snow G=vegetation B=scree", True)
                if mask_tex is not None:
                    mask_tex.location = (node.location.x - 650, node.location.y - 360) if node else mask_tex.location
            node = principled(mat)
            if node is not None:
                if kind == "water":
                    node.inputs["Roughness"].default_value = 0.07
                    node.inputs["IOR"].default_value = 1.33
                    node.inputs["Alpha"].default_value = 1.0
                    try:
                        node.inputs["Transmission Weight"].default_value = 0.15
                    except Exception:
                        pass
                elif kind == "snow":
                    node.inputs["Roughness"].default_value = 0.55
                    node.inputs["Subsurface Weight"].default_value = 0.08
                else:
                    node.inputs["Roughness"].default_value = max(
                        0.55, float(node.inputs["Roughness"].default_value)
                    )


def setup_world(sidecar: dict) -> None:
    palette = sidecar.get("palette", {})
    horizon = palette.get("sky_horizon", [0.27, 0.45, 0.62])
    zenith = palette.get("sky_zenith", [0.01, 0.04, 0.13])
    world = bpy.data.worlds.new("VistaForge Sky")
    bpy.context.scene.world = world
    world.use_nodes = True
    nodes = world.node_tree.nodes
    links = world.node_tree.links
    for node in list(nodes):
        nodes.remove(node)
    out = nodes.new("ShaderNodeOutputWorld")
    background = nodes.new("ShaderNodeBackground")
    gradient = nodes.new("ShaderNodeTexGradient")
    gradient.gradient_type = "LINEAR"
    mapping = nodes.new("ShaderNodeMapping")
    texcoord = nodes.new("ShaderNodeTexCoord")
    ramp = nodes.new("ShaderNodeValToRGB")
    ramp.color_ramp.elements[0].color = (*horizon, 1.0)
    ramp.color_ramp.elements[1].color = (*zenith, 1.0)
    mapping.inputs["Rotation"].default_value[0] = math.radians(90.0)
    links.new(texcoord.outputs["Generated"], mapping.inputs["Vector"])
    links.new(mapping.outputs["Vector"], gradient.inputs["Vector"])
    links.new(gradient.outputs["Fac"], ramp.inputs["Fac"])
    links.new(ramp.outputs["Color"], background.inputs["Color"])
    background.inputs["Strength"].default_value = 0.45
    links.new(background.outputs["Background"], out.inputs["Surface"])


def setup_sun(sidecar: dict) -> None:
    sun_data = sidecar.get("sun", {})
    direction = sun_data.get("direction", [0.0, -0.6, 0.5])
    color = sun_data.get("color", [1.0, 0.9, 0.7])
    intensity = float(sun_data.get("intensity", 1.0))
    light = bpy.data.lights.new("VistaForge Sun", "SUN")
    light.energy = max(0.1, intensity * 8.0)
    light.color = (color[0], color[1], color[2])
    light.angle = math.radians(float(sun_data.get("angular_radius_deg", 0.53)))
    obj = bpy.data.objects.new("VistaForge Sun", light)
    bpy.context.scene.collection.objects.link(obj)
    # Sidecar direction is Z-up, toward the sun. Blender is Y-up after import.
    direction_blender = Vector((direction[0], direction[2], -direction[1])).normalized()
    # Rays travel from the sun to the scene: -direction. The light's local -Z
    # must point along the rays.
    obj.rotation_euler = (-direction_blender).to_track_quat("-Z", "Y").to_euler()
    obj.location = (0.0, 0.0, 500.0)


def setup_camera(sidecar: dict) -> None:
    scene = bpy.context.scene
    camera = scene.camera
    if camera is None:
        for obj in scene.objects:
            if obj.type == "CAMERA":
                camera = obj
                scene.camera = obj
                break
    if camera is None:
        return
    data = camera.data
    camera_info = sidecar.get("camera", {})
    fov = float(camera_info.get("fov_y_deg", 50.0))
    data.sensor_fit = "VERTICAL"
    data.angle = math.radians(fov)
    dof = camera_info.get("dof", {})
    if dof.get("enabled"):
        data.dof.use_dof = True
        data.dof.focus_distance = float(dof.get("focus_m", 100.0))
        data.dof.aperture_fstop = float(dof.get("fstop", 8.0))


def setup_cycles(opts: dict, sidecar: dict) -> None:
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.samples = int(opts["samples"])
    scene.cycles.use_denoising = True
    scene.render.resolution_x = int(opts["width"] or 1920)
    scene.render.resolution_y = int(opts["height"] or 1080)
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    scene.render.film_transparent = False
    scene.view_settings.exposure = float(sidecar.get("render", {}).get("exposure", 1.0))
    if opts["gpu"]:
        try:
            prefs = bpy.context.preferences.addons["cycles"].preferences
            prefs.compute_device_type = "OPTIX"
            prefs.get_devices()
            for device in prefs.devices:
                device.use = True
            scene.cycles.device = "GPU"
        except Exception:
            scene.cycles.device = "CPU"
    else:
        scene.cycles.device = "CPU"


def maybe_smart_uv() -> None:
    for obj in mesh_objects():
        bpy.context.view_layer.objects.active = obj
        obj.select_set(True)
        bpy.ops.object.mode_set(mode="EDIT")
        bpy.ops.mesh.select_all(action="SELECT")
        try:
            bpy.ops.uv.smart_project(angle_limit=math.radians(66.0), island_margin=0.02)
        except Exception as exc:
            print(f"smart UV failed: {exc}")
        bpy.ops.object.mode_set(mode="OBJECT")
        obj.select_set(False)


def maybe_remesh() -> None:
    for obj in mesh_objects():
        modifier = obj.modifiers.new("VistaForge Remesh", "REMESH")
        modifier.mode = "VOXEL"
        modifier.voxel_size = max(0.5, min(4.0, obj.dimensions.z / 400.0))
        bpy.context.view_layer.objects.active = obj
        try:
            bpy.ops.object.modifier_apply(modifier=modifier.name)
        except Exception as exc:
            print(f"remesh failed: {exc}")


def main() -> None:
    opts = parse_args()
    sidecar = load_sidecar(opts["sidecar"])
    clear_scene()
    scene_units()
    bpy.ops.import_scene.gltf(filepath=opts["glb"])
    enhance_materials(sidecar, os.path.dirname(os.path.abspath(opts["sidecar"])))
    setup_world(sidecar)
    setup_sun(sidecar)
    setup_camera(sidecar)
    if opts["smart_uv"]:
        maybe_smart_uv()
    if opts["remesh"]:
        maybe_remesh()

    meshes = mesh_objects()
    vertices = sum(len(obj.data.vertices) for obj in meshes)
    triangles = sum(len(obj.data.loop_triangles) for obj in meshes) if meshes else 0
    uv_layers = sum(len(obj.data.uv_layers) for obj in meshes)
    print(
        f"VistaForge import: {len(meshes)} mesh object(s), {vertices} vertices, "
        f"{len(bpy.data.materials)} material(s), {uv_layers} UV layer(s), "
        f"camera={bpy.context.scene.camera is not None}"
    )

    if opts["validate"]:
        return
    if opts["save_blend"]:
        bpy.ops.wm.save_as_mainfile(filepath=opts["save_blend"])
    setup_cycles(opts, sidecar)
    if opts["render"]:
        out = os.path.abspath(opts["render"])
        os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
        bpy.context.scene.render.filepath = out
        bpy.ops.render.render(write_still=True)
        print(f"VistaForge render written to {out}")


if __name__ == "__main__":
    main()
