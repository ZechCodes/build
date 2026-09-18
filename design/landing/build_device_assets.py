#!/usr/bin/env python3
"""Build original device assets and product renders for the Build landing page.

Run with Blender 4.5 LTS:
  blender --background --python Build/design/landing/build_device_assets.py

The models use meters. Blender's (X, Z, -Y) axes export as glTF (+X, +Y, +Z),
so every screen faces Blender -Y and runtime glTF +Z.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import shutil
import sys
from pathlib import Path

import bpy
import numpy as np
from mathutils import Matrix, Vector


ROOT = Path(__file__).resolve().parents[3]
SOURCE_DIR = ROOT / "Build" / "design" / "landing"
LANDING_ASSETS = ROOT / "Build" / "skriftapp" / "buildapp" / "landing" / "assets"
OUTPUT_DIR = LANDING_ASSETS / "devices"
SCREENS_DIR = LANDING_ASSETS / "screens"
DEFAULT_LAPTOP_SCREEN = SCREENS_DIR / "ui01-desktop.webp"
DEFAULT_TABLET_SCREEN = SCREENS_DIR / "ui04-desktop.webp"
DEFAULT_PHONE_SCREEN = SCREENS_DIR / "ui02-mobile.webp"
BLEND_PATH = SOURCE_DIR / "build-devices.blend"
SCALE = 0.1  # authored dimensions below are decimeters; Blender/source/export are meters


GRAPHITE = (0.075, 0.080, 0.088, 1.0)
GRAPHITE_EDGE = (0.19, 0.20, 0.215, 1.0)
BLACK = (0.008, 0.010, 0.014, 1.0)
KEY_COLOR = (0.065, 0.073, 0.085, 1.0)
GLASS = (0.012, 0.018, 0.026, 1.0)
ACCENT = (0.0, 1.0, 0.48, 1.0)


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--laptop-screen", type=Path, default=DEFAULT_LAPTOP_SCREEN)
    parser.add_argument("--tablet-screen", type=Path, default=DEFAULT_TABLET_SCREEN)
    parser.add_argument("--phone-screen", type=Path, default=DEFAULT_PHONE_SCREEN)
    blender_args = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    return parser.parse_args(blender_args)


def reset_scene() -> None:
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    for datablocks in (bpy.data.materials, bpy.data.cameras, bpy.data.lights):
        for datablock in list(datablocks):
            datablocks.remove(datablock)
    bpy.context.scene.unit_settings.system = "METRIC"
    bpy.context.scene.unit_settings.scale_length = 1.0
    bpy.context.preferences.filepaths.save_version = 0


def material(name: str, color: tuple[float, float, float, float], metallic=0.0, roughness=0.4):
    mat = bpy.data.materials.new(name)
    mat.diffuse_color = color
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = color
    bsdf.inputs["Metallic"].default_value = metallic
    bsdf.inputs["Roughness"].default_value = roughness
    return mat


def screen_material(name: str, image_path: Path | None):
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    nodes = mat.node_tree.nodes
    links = mat.node_tree.links
    bsdf = nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = GLASS
    bsdf.inputs["Roughness"].default_value = 0.82
    if "Specular IOR Level" in bsdf.inputs:
        bsdf.inputs["Specular IOR Level"].default_value = 0.0
    if "Emission Color" in bsdf.inputs:
        bsdf.inputs["Emission Color"].default_value = (0.015, 0.025, 0.035, 1.0)
        bsdf.inputs["Emission Strength"].default_value = 0.55
    if image_path and image_path.exists():
        image = bpy.data.images.load(str(image_path), check_existing=True)
        tex = nodes.new("ShaderNodeTexImage")
        tex.name = "screen_texture"
        tex.label = "Replaceable screen texture"
        tex.image = image
        image.pack()
        tex.interpolation = "Linear"
        links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
        if "Emission Color" in bsdf.inputs:
            links.new(tex.outputs["Color"], bsdf.inputs["Emission Color"])
            bsdf.inputs["Emission Strength"].default_value = 0.68
    return mat


def set_screen_texture(mat, image_path: Path):
    image_path = image_path.resolve()
    if not image_path.exists():
        raise FileNotFoundError(f"Screen fixture does not exist: {image_path}")
    image = bpy.data.images.load(str(image_path), check_existing=True)
    image.pack()
    mat.node_tree.nodes["screen_texture"].image = image


def rounded_box(name, location, dimensions, mat, radius=0.04, collection=None):
    location = tuple(value * SCALE for value in location)
    dimensions = tuple(value * SCALE for value in dimensions)
    radius *= SCALE
    bpy.ops.mesh.primitive_cube_add(location=location)
    obj = bpy.context.object
    obj.name = name
    obj.dimensions = dimensions
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    bevel = obj.modifiers.new("soft_edges", "BEVEL")
    bevel.width = min(radius, min(dimensions) * 0.42)
    bevel.segments = 6
    bevel.harden_normals = True
    bpy.context.view_layer.objects.active = obj
    bpy.ops.object.modifier_apply(modifier=bevel.name)
    obj.data.materials.append(mat)
    if collection:
        move_to_collection(obj, collection)
    return obj


def plane_screen(name, location, width, height, mat, collection):
    location = tuple(value * SCALE for value in location)
    width *= SCALE
    height *= SCALE
    bpy.ops.mesh.primitive_plane_add(size=2.0, location=location, rotation=(math.radians(90), 0, 0))
    obj = bpy.context.object
    obj.name = name
    obj.scale = (width / 2, height / 2, 1)
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    obj.data.materials.append(mat)
    obj["screen_node"] = True
    move_to_collection(obj, collection)
    return obj


def cylinder(name, location, radius, depth, mat, collection, rotation=(0, 0, 0), vertices=32):
    location = tuple(value * SCALE for value in location)
    radius *= SCALE
    depth *= SCALE
    bpy.ops.mesh.primitive_cylinder_add(vertices=vertices, radius=radius, depth=depth, location=location, rotation=rotation)
    obj = bpy.context.object
    obj.name = name
    obj.data.materials.append(mat)
    bevel = obj.modifiers.new("edge_bevel", "BEVEL")
    bevel.width = 0.012 * SCALE
    bevel.segments = 4
    bevel.harden_normals = True
    bpy.context.view_layer.objects.active = obj
    bpy.ops.object.modifier_apply(modifier=bevel.name)
    move_to_collection(obj, collection)
    return obj


def move_to_collection(obj, collection):
    for owner in list(obj.users_collection):
        owner.objects.unlink(obj)
    collection.objects.link(obj)


def add_collection(name: str):
    collection = bpy.data.collections.new(name)
    bpy.context.scene.collection.children.link(collection)
    return collection


def build_laptop(mats):
    c = add_collection("Laptop")
    # Hinge pivot is the origin: stable for animation and runtime replacement.
    rounded_box("laptop_lid", (0, 0.015, 1.145), (3.32, 0.095, 2.20), mats["graphite"], 0.075, c)
    screen = plane_screen("laptop_screen", (0, -0.035, 1.17), 3.08, 1.925, mats["desktop_screen"], c)
    screen["replaceable_texture"] = True
    screen["runtime_forward"] = "+Z"
    rounded_box("laptop_base", (0, -1.00, 0.005), (3.42, 2.00, 0.13), mats["graphite"], 0.08, c)
    rounded_box("keyboard_well", (0, -1.04, 0.081), (2.86, 1.08, 0.018), mats["black"], 0.035, c)
    key_w, key_h = 0.168, 0.145
    for row in range(5):
        cols = 14 if row < 4 else 11
        offset = -(cols - 1) * 0.202 / 2
        for col in range(cols):
            key = rounded_box(
                f"key_{row:02d}_{col:02d}",
                (offset + col * 0.202, -0.70 - row * 0.188, 0.103),
                (key_w, key_h, 0.022), mats["key"], 0.018, c,
            )
            key["decorative"] = True
    rounded_box("trackpad", (0, -1.625, 0.087), (1.17, 0.50, 0.012), mats["edge"], 0.04, c)
    cylinder("hinge", (0, -0.035, 0.095), 0.072, 2.76, mats["edge"], c, rotation=(0, math.radians(90), 0))
    # Tiny green power indicator supplies a controlled accent in close renders.
    cylinder("status_light", (1.49, -1.75, 0.085), 0.018, 0.008, mats["accent"], c, rotation=(math.radians(90), 0, 0), vertices=20)
    return c


def build_tablet(mats):
    c = add_collection("Tablet")
    rounded_box("tablet_body", (0, 0, 0), (2.52, 0.105, 1.72), mats["graphite"], 0.16, c)
    screen = plane_screen("tablet_screen", (0, -0.057, 0), 2.34, 1.4625, mats["tablet_screen"], c)
    screen["replaceable_texture"] = True
    screen["runtime_forward"] = "+Z"
    cylinder("tablet_camera", (0, -0.061, 0.815), 0.018, 0.008, mats["black"], c, rotation=(math.radians(90), 0, 0), vertices=20)
    return c


def build_phone(mats):
    c = add_collection("Phone")
    rounded_box("phone_body", (0, 0, 0), (0.84, 0.092, 1.74), mats["graphite"], 0.135, c)
    screen = plane_screen("phone_screen", (0, -0.050, -0.005), 0.77, 1.64, mats["phone_screen"], c)
    screen["replaceable_texture"] = True
    screen["runtime_forward"] = "+Z"
    rounded_box("phone_speaker", (0, -0.054, 0.812), (0.16, 0.008, 0.016), mats["black"], 0.008, c)
    return c


def collection_objects(collection):
    return list(collection.all_objects)


def set_collection_visibility(active_collection):
    for collection in bpy.data.collections:
        if collection.name in {"Laptop", "Tablet", "Phone"}:
            collection.hide_render = collection != active_collection


def set_visible_collections(*active_collections):
    active_names = {collection.name for collection in active_collections}
    for collection in bpy.data.collections:
        if collection.name in {"Laptop", "Tablet", "Phone"}:
            collection.hide_render = collection.name not in active_names


def select_collection(collection):
    bpy.ops.object.select_all(action="DESELECT")
    for obj in collection_objects(collection):
        obj.select_set(True)
    bpy.context.view_layer.objects.active = collection_objects(collection)[0]


def disconnect_screen_textures(collection):
    restored = []
    for obj in collection_objects(collection):
        if not obj.get("screen_node") or not obj.data.materials:
            continue
        mat = obj.data.materials[0]
        tree = mat.node_tree
        tex = tree.nodes.get("screen_texture")
        bsdf = tree.nodes.get("Principled BSDF")
        if tex and bsdf:
            for link in list(tree.links):
                if link.from_node == tex:
                    restored.append((tree, tex, link.to_socket))
                    tree.links.remove(link)
    return restored


def reconnect_screen_textures(restored):
    for tree, tex, socket in restored:
        tree.links.new(tex.outputs["Color"], socket)


def export_glb(collection, path, include_decorative=True):
    select_collection(collection)
    if not include_decorative:
        for obj in collection_objects(collection):
            if obj.get("decorative"):
                obj.select_set(False)
    active_screen = next(obj for obj in collection_objects(collection) if obj.get("screen_node"))
    original_name = active_screen.name
    active_screen.name = "screen"
    restored = disconnect_screen_textures(collection)
    bpy.ops.export_scene.gltf(
        filepath=str(path),
        export_format="GLB",
        use_selection=True,
        export_yup=True,
        export_apply=True,
        export_materials="EXPORT",
        export_extras=True,
        export_cameras=False,
        export_lights=False,
        export_image_format="NONE",
    )
    reconnect_screen_textures(restored)
    active_screen.name = original_name


def aim_camera(camera, target):
    direction = Vector(target) - camera.location
    camera.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()


def add_camera(name, location, target, lens=58, ortho_scale=None):
    data = bpy.data.cameras.new(name)
    camera = bpy.data.objects.new(name, data)
    bpy.context.scene.collection.objects.link(camera)
    camera.location = tuple(value * SCALE for value in location)
    if ortho_scale is None:
        data.lens = lens
    else:
        data.type = "ORTHO"
        data.ortho_scale = ortho_scale * SCALE
    aim_camera(camera, tuple(value * SCALE for value in target))
    bpy.context.scene.camera = camera
    return camera


def add_area(name, location, energy, size, color, target):
    data = bpy.data.lights.new(name, "AREA")
    # Preserve the authored exposure when the whole physical scene is scaled to meters.
    data.energy = energy * SCALE * SCALE
    data.shape = "DISK"
    data.size = size * SCALE
    data.color = color
    obj = bpy.data.objects.new(name, data)
    bpy.context.scene.collection.objects.link(obj)
    obj.location = tuple(value * SCALE for value in location)
    aim_camera(obj, tuple(value * SCALE for value in target))
    return obj


def clear_render_rig():
    for obj in list(bpy.context.scene.objects):
        if obj.type in {"CAMERA", "LIGHT"} or obj.name.startswith("render_"):
            bpy.data.objects.remove(obj, do_unlink=True)


def setup_render(width, height, transparent):
    scene = bpy.context.scene
    scene.render.engine = "BLENDER_EEVEE_NEXT"
    scene.render.resolution_x = width
    scene.render.resolution_y = height
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "WEBP"
    scene.render.image_settings.color_mode = "RGBA" if transparent else "RGB"
    scene.render.image_settings.color_depth = "8"
    scene.render.film_transparent = transparent
    scene.render.image_settings.quality = 92
    scene.render.film_transparent = transparent
    scene.render.engine = "BLENDER_EEVEE_NEXT"
    scene.render.image_settings.color_mode = "RGBA" if transparent else "RGB"
    scene.view_settings.look = "AgX - Medium Low Contrast"
    scene.world.use_nodes = True
    background = scene.world.node_tree.nodes.get("Background")
    background.inputs["Color"].default_value = (0.0, 0.0, 0.0, 1.0)
    background.inputs["Strength"].default_value = 0.0


def add_floor(size=14, z=-0.075, black=True):
    bpy.ops.mesh.primitive_plane_add(size=size * SCALE, location=(0, 0, z * SCALE))
    floor = bpy.context.object
    floor.name = "render_floor"
    floor.data.materials.append(bpy.data.materials["RenderFloorBlack" if black else "RenderFloorClear"])
    return floor


def render(path, width, height, camera_location, target, lens, transparent, floor=False, ortho_scale=None):
    clear_render_rig()
    setup_render(width, height, transparent)
    add_camera("render_camera", camera_location, target, lens, ortho_scale=ortho_scale)
    add_area("render_key", (-4.2, -5.8, 5.4), 800, 4.8, (1.0, 0.98, 0.95), target)
    add_area("render_fill", (4.2, -4.4, 3.0), 500, 4.2, (0.82, 0.87, 0.94), target)
    add_area("render_rim", (3.8, 1.6, 4.6), 420, 3.8, (0.90, 0.93, 1.0), target)
    add_area("render_top", (-1.0, 0.2, 7.0), 260, 3.5, (1.0, 0.99, 0.97), target)
    if floor:
        add_floor(14, -0.075, black=True)
    bpy.context.scene.render.filepath = str(path)
    bpy.ops.render.render(write_still=True)


def add_mobile_composition(tablet, phone):
    # Group transforms preserve each screen plane's authored front orientation.
    tablet_state = transform_collection(
        tablet, (0, 0, 0), offset=(-0.50, 0, 0.13), rotation=(0, 0, math.radians(-7)),
    )
    phone_state = transform_collection(
        phone, (0, 0, 0), offset=(0.90, -0.18, -0.38), rotation=(0, 0, math.radians(9)),
    )
    return tablet_state, phone_state


def restore_offsets(collection, offsets):
    restore_matrices(collection, offsets)


def transform_collection(collection, pivot, offset=(0, 0, 0), scale=1.0, rotation=(0, 0, 0)):
    snapshots = {obj.name: obj.matrix_world.copy() for obj in collection_objects(collection)}
    pivot = Vector(tuple(value * SCALE for value in pivot))
    offset = Vector(tuple(value * SCALE for value in offset))
    rotation_matrix = Matrix.Rotation(rotation[2], 4, "Z") @ Matrix.Rotation(rotation[1], 4, "Y") @ Matrix.Rotation(rotation[0], 4, "X")
    transform = Matrix.Translation(offset) @ Matrix.Translation(pivot) @ rotation_matrix @ Matrix.Scale(scale, 4) @ Matrix.Translation(-pivot)
    for obj in collection_objects(collection):
        obj.matrix_world = transform @ obj.matrix_world
    return snapshots


def restore_matrices(collection, snapshots):
    for obj in collection_objects(collection):
        obj.matrix_world = snapshots[obj.name]


def triangle_count(collection, include_decorative=True):
    depsgraph = bpy.context.evaluated_depsgraph_get()
    total = 0
    for obj in collection_objects(collection):
        if obj.type != "MESH":
            continue
        if not include_decorative and obj.get("decorative"):
            continue
        mesh = obj.evaluated_get(depsgraph).to_mesh()
        mesh.calc_loop_triangles()
        total += len(mesh.loop_triangles)
        obj.evaluated_get(depsgraph).to_mesh_clear()
    return total


def image_info(path):
    image = bpy.data.images.load(str(path), check_existing=False)
    dimensions = [image.size[0], image.size[1]]
    content_bounds = None
    if image.channels == 4:
        pixels = np.empty(image.size[0] * image.size[1] * 4, dtype=np.float32)
        image.pixels.foreach_get(pixels)
        alpha = pixels[3::4].reshape((image.size[1], image.size[0]))
        visible_y, visible_x = np.where(alpha > 0.01)
        if visible_x.size:
            min_x, max_x = int(visible_x.min()), int(visible_x.max())
            min_y, max_y = int(visible_y.min()), int(visible_y.max())
            content_bounds = [min_x, min_y, max_x - min_x + 1, max_y - min_y + 1]
    bpy.data.images.remove(image)
    info = {"file": path.name, "width": dimensions[0], "height": dimensions[1], "bytes": path.stat().st_size}
    if content_bounds:
        info["content_bounds_px_bottom_left"] = content_bounds
    return info


def render_scene_posters(collections, mats, default_screens):
    laptop, tablet, phone = collections["laptop"], collections["tablet"], collections["phone"]
    fixture_stems = ["ui01", "ui02", "ui03-resumed", "ui04", "ui05-approval", "ui05-merged"]
    render_paths = []
    for index, stem in enumerate(fixture_stems, start=1):
        desktop_fixture = SCREENS_DIR / f"{stem}-desktop.webp"
        mobile_fixture = SCREENS_DIR / f"{stem}-mobile.webp"
        set_screen_texture(mats["desktop_screen"], desktop_fixture)
        set_screen_texture(mats["tablet_screen"], desktop_fixture)
        set_screen_texture(mats["phone_screen"], mobile_fixture)
        desktop_path = OUTPUT_DIR / f"scene-{index:02d}-desktop.webp"
        mobile_path = OUTPUT_DIR / f"scene-{index:02d}-mobile.webp"

        if index == 1:
            set_visible_collections(laptop)
            render(desktop_path, 1440, 900, (0.84, -6.94, 1.54), (0, -1.0, 1.12), 58, False, ortho_scale=3.0)
            render(mobile_path, 720, 960, (0.84, -6.94, 1.54), (0, -1.0, 1.12), 58, False, ortho_scale=5.1)
        elif index == 2:
            laptop_state = transform_collection(laptop, (0, -1.0, 1.12), offset=(-0.72, 0, 0), scale=0.78)
            phone_state = transform_collection(phone, (0, 0, 0), offset=(1.35, -0.15, -0.20), scale=1.12)
            set_visible_collections(laptop, phone)
            render(desktop_path, 1440, 900, (0.9, -7.2, 1.55), (0.15, -0.55, 0.75), 58, False, ortho_scale=4.2)
            restore_matrices(laptop, laptop_state)
            restore_matrices(phone, phone_state)
            set_visible_collections(phone)
            render(mobile_path, 720, 960, (1.45, -3.95, 1.10), (0, 0, 0), 72, False)
        elif index == 3:
            set_visible_collections(phone)
            render(desktop_path, 1440, 900, (1.7, -4.7, 1.25), (-0.55, 0, 0), 72, False)
            render(mobile_path, 720, 960, (1.45, -3.95, 1.10), (0, 0, 0), 72, False)
        elif index in {4, 5}:
            set_visible_collections(tablet)
            render(desktop_path, 1440, 900, (2.45, -5.0, 1.92), (0, 0, 0), 67, False)
            render(mobile_path, 720, 960, (2.45, -5.0, 1.92), (0, 0, 0), 67, False)
        else:
            laptop_state = transform_collection(laptop, (0, -1.0, 1.12), offset=(-1.2, 0, 0), scale=0.62)
            tablet_state = transform_collection(tablet, (0, 0, 0), offset=(1.18, 0.05, 0.42), scale=0.58)
            phone_state = transform_collection(phone, (0, 0, 0), offset=(2.18, -0.14, -0.34), scale=0.78)
            set_visible_collections(laptop, tablet, phone)
            render(desktop_path, 1440, 900, (1.05, -7.0, 1.65), (0.35, -0.45, 0.76), 60, False, ortho_scale=5.0)
            restore_matrices(laptop, laptop_state)
            restore_matrices(tablet, tablet_state)
            restore_matrices(phone, phone_state)
            tablet_offsets, phone_offsets = add_mobile_composition(tablet, phone)
            set_visible_collections(tablet, phone)
            render(mobile_path, 720, 960, (2.85, -6.6, 2.20), (-0.15, 0, 0.02), 70, False)
            restore_offsets(tablet, tablet_offsets)
            restore_offsets(phone, phone_offsets)

        render_paths.extend([desktop_path, mobile_path])

    set_screen_texture(mats["desktop_screen"], default_screens["laptop"])
    set_screen_texture(mats["tablet_screen"], default_screens["tablet"])
    set_screen_texture(mats["phone_screen"], default_screens["phone"])
    shutil.copyfile(OUTPUT_DIR / "scene-06-desktop.webp", OUTPUT_DIR / "desktop-poster.webp")
    shutil.copyfile(OUTPUT_DIR / "scene-06-mobile.webp", OUTPUT_DIR / "mobile-poster.webp")
    render_paths.extend([OUTPUT_DIR / "desktop-poster.webp", OUTPUT_DIR / "mobile-poster.webp"])
    return render_paths


def build_metadata(collections, render_paths, default_screens):
    specs = {
        "laptop": {
            "dimensions_m": [0.342, 0.220, 0.200],
            "screen_corners_m": [[-0.154, 0.02075, 0.0035], [0.154, 0.02075, 0.0035], [0.154, 0.21325, 0.0035], [-0.154, 0.21325, 0.0035]],
            "hinge_pivot_m": [0, 0.0095, 0.0035],
        },
        "tablet": {
            "dimensions_m": [0.252, 0.172, 0.0105],
            "screen_corners_m": [[-0.117, -0.073125, 0.0057], [0.117, -0.073125, 0.0057], [0.117, 0.073125, 0.0057], [-0.117, 0.073125, 0.0057]],
        },
        "phone": {
            "dimensions_m": [0.084, 0.174, 0.0092],
            "screen_corners_m": [[-0.0385, -0.0825, 0.0050], [0.0385, -0.0825, 0.0050], [0.0385, 0.0815, 0.0050], [-0.0385, 0.0815, 0.0050]],
        },
    }
    devices = {}
    for name, collection in collections.items():
        glb = OUTPUT_DIR / f"{name}.glb"
        item = specs[name]
        item.update({
            "file": glb.name,
            "bytes": glb.stat().st_size,
            "triangles": triangle_count(collection),
            "screen_node": "screen",
            "screen_material": {"laptop": "ScreenDesktop", "tablet": "ScreenTablet", "phone": "ScreenPhone"}[name],
        })
        if name == "laptop":
            low = OUTPUT_DIR / "laptop-low.glb"
            item.update({
                "low_detail_file": low.name,
                "low_detail_bytes": low.stat().st_size,
                "low_detail_triangles": triangle_count(collection, include_decorative=False),
            })
        devices[name] = item
    metadata = {
        "version": 1,
        "units": "meters",
        "coordinate_system": {"up": "+Y", "forward": "+Z", "right": "+X"},
        "origin": "device center; laptop origin is hinge pivot projected to ground",
        "devices": devices,
        "renders": {path.stem: image_info(path) for path in render_paths},
        "source": {"blend": BLEND_PATH.name, "generator": Path(__file__).name, "blender": bpy.app.version_string},
        "screen_fixtures": {
            name: {
                "file": str(path.relative_to(LANDING_ASSETS)),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                "packed_in_blend": True,
            }
            for name, path in default_screens.items()
        },
        "screen_provenance": "Reconstructed demonstration fixtures from current Build renderers; not a live-host capture.",
        "licensing": {
            "geometry": "Original geometry created for Build; no third-party geometry or materials.",
            "screen_ui": "Reconstructed Build demonstration fixtures generated from current project renderers; retain project asset terms.",
            "external_assets": "None.",
        },
    }
    (OUTPUT_DIR / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")


def main():
    args = parse_args()
    SOURCE_DIR.mkdir(parents=True, exist_ok=True)
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    default_screens = {
        "laptop": args.laptop_screen.resolve(),
        "tablet": args.tablet_screen.resolve(),
        "phone": args.phone_screen.resolve(),
    }
    reset_scene()
    mats = {
        "graphite": material("Graphite", GRAPHITE, metallic=0.70, roughness=0.31),
        "edge": material("GraphiteEdge", GRAPHITE_EDGE, metallic=0.64, roughness=0.25),
        "black": material("BlackInset", BLACK, metallic=0.15, roughness=0.28),
        "key": material("KeyGraphite", KEY_COLOR, metallic=0.12, roughness=0.39),
        "accent": material("BuildGreen", ACCENT, metallic=0.05, roughness=0.22),
        "desktop_screen": screen_material("ScreenDesktop", default_screens["laptop"]),
        "tablet_screen": screen_material("ScreenTablet", default_screens["tablet"]),
        "phone_screen": screen_material("ScreenPhone", default_screens["phone"]),
    }
    floor_black = material("RenderFloorBlack", (0.0015, 0.002, 0.004, 1), metallic=0.05, roughness=0.34)
    floor_clear = material("RenderFloorClear", (0.02, 0.02, 0.02, 0), metallic=0, roughness=0.5)
    floor_clear.diffuse_color = (0.02, 0.02, 0.02, 0)

    laptop = build_laptop(mats)
    tablet = build_tablet(mats)
    phone = build_phone(mats)
    collections = {"laptop": laptop, "tablet": tablet, "phone": phone}

    for name, collection in collections.items():
        export_glb(collection, OUTPUT_DIR / f"{name}.glb")
    export_glb(laptop, OUTPUT_DIR / "laptop-low.glb", include_decorative=False)

    # Save editable source before temporary composition transforms are applied.
    bpy.ops.wm.save_as_mainfile(filepath=str(BLEND_PATH))

    render_paths = []
    set_collection_visibility(laptop)
    render(OUTPUT_DIR / "hero-laptop.webp", 1600, 1180, (0.88, -6.94, 1.54), (0.04, -1.0, 1.12), 58, True, ortho_scale=3.45)
    render_paths.append(OUTPUT_DIR / "hero-laptop.webp")
    render(OUTPUT_DIR / "laptop.webp", 1200, 900, (0.88, -6.94, 1.54), (0.04, -1.0, 1.12), 58, True, ortho_scale=3.45)
    render_paths.append(OUTPUT_DIR / "laptop.webp")

    set_collection_visibility(tablet)
    render(OUTPUT_DIR / "tablet.webp", 1000, 760, (2.45, -5.0, 1.92), (0, 0, 0), 67, True)
    render_paths.append(OUTPUT_DIR / "tablet.webp")
    set_collection_visibility(phone)
    render(OUTPUT_DIR / "phone.webp", 640, 1040, (1.45, -3.95, 1.10), (0, 0, 0), 72, True)
    render_paths.append(OUTPUT_DIR / "phone.webp")

    tablet_offsets, phone_offsets = add_mobile_composition(tablet, phone)
    tablet.hide_render = False
    phone.hide_render = False
    laptop.hide_render = True
    render(OUTPUT_DIR / "mobile-hero.webp", 1200, 1350, (2.85, -6.6, 2.20), (-0.15, 0, 0.02), 70, True)
    render_paths.append(OUTPUT_DIR / "mobile-hero.webp")
    restore_offsets(tablet, tablet_offsets)
    restore_offsets(phone, phone_offsets)

    render_paths.extend(render_scene_posters(collections, mats, default_screens))

    set_collection_visibility(laptop)
    render(OUTPUT_DIR / "social-preview.webp", 1200, 630, (4.7, -6.8, 3.45), (-1.18, -0.58, 0.68), 50, False)
    render_paths.append(OUTPUT_DIR / "social-preview.webp")

    build_metadata(collections, render_paths, default_screens)
    bpy.ops.wm.save_as_mainfile(filepath=str(BLEND_PATH))
    BLEND_PATH.with_suffix(".blend1").unlink(missing_ok=True)
    print(f"Built device assets in {OUTPUT_DIR}")


if __name__ == "__main__":
    main()
