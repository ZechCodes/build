#!/usr/bin/env python3
"""Build original device assets and product renders for the Build landing page.

Run with Blender 4.5 LTS:
  blender --background --python design/landing/build_device_assets.py

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

import bmesh
import bpy
import numpy as np
from bpy_extras.object_utils import world_to_camera_view
from mathutils import Matrix, Vector

ROOT = Path(__file__).resolve().parents[2]
SOURCE_DIR = ROOT / "design" / "landing"
LANDING_ASSETS = ROOT / "skriftapp" / "buildapp" / "landing" / "assets"
OUTPUT_DIR = LANDING_ASSETS / "devices"
SCREENS_DIR = LANDING_ASSETS / "screens"
DEFAULT_LAPTOP_SCREEN = SCREENS_DIR / "ui01-macbook.webp"
DEFAULT_TABLET_SCREEN = SCREENS_DIR / "ui04-ipad.webp"
DEFAULT_PHONE_SCREEN = SCREENS_DIR / "ui02-iphone.webp"
BLEND_PATH = SOURCE_DIR / "build-devices.blend"
SCALE = 0.1  # authored dimensions below are decimeters; Blender/source/export are meters


SILVER = (0.65, 0.66, 0.68, 1.0)
SATIN_DARK = (0.12, 0.13, 0.14, 1.0)
SILVER_EDGE = (0.72, 0.73, 0.75, 1.0)
BLACK = (0.008, 0.010, 0.014, 1.0)
KEY_COLOR = (0.0035, 0.0038, 0.0045, 1.0)
GLASS = (0.012, 0.018, 0.026, 1.0)
LAPTOP_LID_ANGLE_DEG = -15.0  # 105 degrees measured from the keyboard deck
# The visible open pose was authored around the lower barrel center. The
# animation parent uses the actual linkage axis above the deck so its 90-degree
# closed pose clears the keyboard while preserving that accepted open pose.
LAPTOP_OPEN_GEOMETRY_PIVOT = Vector((0.0, -0.004, 0.096))
LAPTOP_HINGE = Vector((0.0, -0.004, 0.1175))
LAPTOP_SCREEN_SIZE_M = (3024 / 254 * 0.0254, 1964 / 254 * 0.0254)
TABLET_SCREEN_SIZE_M = (2420 / 264 * 0.0254, 1668 / 264 * 0.0254)
PHONE_SCREEN_SIZE_M = (1320 / 460 * 0.0254, 2868 / 460 * 0.0254)
LAPTOP_SCREEN_Y = -0.0130
TABLET_SCREEN_Y = -0.0285
PHONE_SCREEN_Y = -0.0459
LAPTOP_RENDER_CAMERA = (0.0, -6.40, 0.82)
LAPTOP_RENDER_TARGET = (0.0, -0.92, 1.10)
LAPTOP_RENDER_LENS = 70


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--laptop-screen", type=Path, default=DEFAULT_LAPTOP_SCREEN)
    parser.add_argument("--tablet-screen", type=Path, default=DEFAULT_TABLET_SCREEN)
    parser.add_argument("--phone-screen", type=Path, default=DEFAULT_PHONE_SCREEN)
    parser.add_argument("--preview-dir", type=Path)
    parser.add_argument(
        "--render-only", choices=("posters", "closing", "cutouts", "social", "all"),
        help="render from the checked-in Blender source without rebuilding or exporting geometry",
    )
    parser.add_argument(
        "--models-only", "--skip-renders", action="store_true", dest="models_only",
        help="export GLBs, the Blender source, metadata, and contract without poster renders",
    )
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


def material(
    name: str,
    color: tuple[float, float, float, float],
    metallic=0.0,
    roughness=0.4,
    coat=0.0,
    coat_roughness=None,
    anisotropic=0.0,
    specular=None,
):
    mat = bpy.data.materials.new(name)
    mat.diffuse_color = color
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = color
    bsdf.inputs["Metallic"].default_value = metallic
    bsdf.inputs["Roughness"].default_value = roughness
    if "Coat Weight" in bsdf.inputs:
        bsdf.inputs["Coat Weight"].default_value = coat
        bsdf.inputs["Coat Roughness"].default_value = (
            coat_roughness if coat_roughness is not None else max(0.08, roughness * 0.55)
        )
    if "Anisotropic IOR Level" in bsdf.inputs:
        bsdf.inputs["Anisotropic IOR Level"].default_value = anisotropic
    if specular is not None and "Specular IOR Level" in bsdf.inputs:
        bsdf.inputs["Specular IOR Level"].default_value = specular
    return mat


def screen_material(name: str, image_path: Path | None):
    mat = bpy.data.materials.new(name)
    mat["display_response"] = "emission_only"
    mat.use_nodes = True
    nodes = mat.node_tree.nodes
    links = mat.node_tree.links
    bsdf = nodes.get("Principled BSDF")
    # Active pixels should reproduce the UI without inheriting the studio rig.
    # Keep a Principled surface so glTF export and texture detachment retain the
    # existing material contract, but remove its reflective response entirely.
    bsdf.inputs["Base Color"].default_value = (0.0, 0.0, 0.0, 1.0)
    bsdf.inputs["Metallic"].default_value = 0.0
    bsdf.inputs["Roughness"].default_value = 1.0
    if "Specular IOR Level" in bsdf.inputs:
        bsdf.inputs["Specular IOR Level"].default_value = 0.0
    if "Coat Weight" in bsdf.inputs:
        bsdf.inputs["Coat Weight"].default_value = 0.0
    if "Emission Color" in bsdf.inputs:
        bsdf.inputs["Emission Color"].default_value = (1.0, 1.0, 1.0, 1.0)
        bsdf.inputs["Emission Strength"].default_value = 1.0
    coordinates = nodes.new("ShaderNodeTexCoord")
    mapping = nodes.new("ShaderNodeMapping")
    mapping.name = "screen_contain"
    tex = nodes.new("ShaderNodeTexImage")
    tex.name = "screen_texture"
    tex.label = "Replaceable screen texture"
    tex.extension = "CLIP"
    tex.interpolation = "Linear"
    links.new(coordinates.outputs["UV"], mapping.inputs["Vector"])
    links.new(mapping.outputs["Vector"], tex.inputs["Vector"])
    if "Emission Color" in bsdf.inputs:
        links.new(tex.outputs["Color"], bsdf.inputs["Emission Color"])
    if image_path and image_path.exists():
        image = bpy.data.images.load(str(image_path), check_existing=True)
        tex.image = image
        image.pack()
    return mat


def configure_screen_contain(mat, image):
    if not image or not image.size[1] or "screen_aspect" not in mat:
        return
    screen_aspect = mat["screen_aspect"]
    image_aspect = image.size[0] / image.size[1]
    scale_x = max(1.0, screen_aspect / image_aspect)
    scale_y = max(1.0, image_aspect / screen_aspect)
    mapping = mat.node_tree.nodes["screen_contain"]
    mapping.inputs["Scale"].default_value = (scale_x, scale_y, 1.0)
    mapping.inputs["Location"].default_value = ((1 - scale_x) / 2, (1 - scale_y) / 2, 0.0)


def set_screen_texture(mat, image_path: Path):
    image_path = image_path.resolve()
    if not image_path.exists():
        raise FileNotFoundError(f"Screen fixture does not exist: {image_path}")
    image = bpy.data.images.load(str(image_path), check_existing=True)
    image.pack()
    mat.node_tree.nodes["screen_texture"].image = image
    configure_screen_contain(mat, image)


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


def rounded_outline(width, height, radius, segments=20):
    radius = min(radius, width / 2, height / 2)
    centers = (
        (width / 2 - radius, -height / 2 + radius, -90),
        (width / 2 - radius, height / 2 - radius, 0),
        (-width / 2 + radius, height / 2 - radius, 90),
        (-width / 2 + radius, -height / 2 + radius, 180),
    )
    points = []
    for center_x, center_y, start_angle in centers:
        for step in range(segments + 1):
            angle = math.radians(start_angle + 90 * step / segments)
            points.append((center_x + radius * math.cos(angle), center_y + radius * math.sin(angle)))
    deduplicated = []
    for point in points:
        if not deduplicated or math.dist(point, deduplicated[-1]) > 1e-12:
            deduplicated.append(point)
    if len(deduplicated) > 1 and math.dist(deduplicated[0], deduplicated[-1]) <= 1e-12:
        deduplicated.pop()
    return deduplicated


def assert_convex_outward_normals(mesh, name):
    """Fail the build when a generated convex shell contains an inward face."""
    inward = [
        polygon.index
        for polygon in mesh.polygons
        if polygon.center.dot(polygon.normal) <= 1e-10
    ]
    if inward:
        raise AssertionError(f"{name} has inward-facing polygons: {inward[:8]}")


def rounded_prism(name, location, dimensions, radius, chamfer, mat, collection, plane="XY", segments=20):
    width, height, depth = (value * SCALE for value in dimensions)
    location = Vector(tuple(value * SCALE for value in location))
    outline = rounded_outline(width, height, radius * SCALE, segments=segments)
    half_depth = depth / 2
    if plane == "XY":
        front = [(x, y, half_depth) for x, y in outline]
        back = [(x, y, -half_depth) for x, y in outline]
    else:
        front = [(x, -half_depth, y) for x, y in outline]
        back = [(x, half_depth, y) for x, y in outline]
    count = len(outline)
    faces = [tuple(range(count)), tuple(range(2 * count - 1, count - 1, -1))]
    # The outline is CCW. Walk front -> back before advancing around the
    # outline so side normals face away from the shell in both supported planes.
    faces.extend((index, index + count, (index + 1) % count + count, (index + 1) % count) for index in range(count))
    mesh = bpy.data.meshes.new(f"{name}_mesh")
    mesh.from_pydata(front + back, [], faces)
    mesh.update()
    assert_convex_outward_normals(mesh, name)
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    obj.location = location
    obj.data.materials.append(mat)
    if chamfer:
        bevel = obj.modifiers.new("micro_chamfer", "BEVEL")
        bevel.width = chamfer * SCALE
        bevel.segments = 3
        bevel.harden_normals = True
        bpy.context.view_layer.objects.active = obj
        bpy.ops.object.modifier_apply(modifier=bevel.name)
    cap_axis = 2 if plane == "XY" else 1
    for polygon in obj.data.polygons:
        # Flat caps keep crisp screen/deck silhouettes; curved perimeter and
        # bevel faces interpolate their normals instead of showing segments.
        polygon.use_smooth = abs(polygon.normal[cap_axis]) < 0.999
    return obj


def assign_front_band(body, material):
    """Match the runtime's glTF normal.z > 0.5 band on the finished shell."""
    body.data.materials.append(material)
    band_count = 0
    for polygon in body.data.polygons:
        # glTF +Z is Blender -Y after export_yup; the bevel is already applied.
        if polygon.normal.y < -0.5:
            polygon.material_index = 1
            band_count += 1
    if not band_count or band_count == len(body.data.polygons):
        raise AssertionError(f"{body.name}: missing front band or chassis faces")


def profiled_rounded_shell(
    name, location, dimensions, radius, profile, mat, collection, segments=20,
):
    """Build a solid enclosure from a continuous rounded horizontal section."""
    width, length, height = (value * SCALE for value in dimensions)
    location = Vector(tuple(value * SCALE for value in location))
    radius *= SCALE
    levels = [(z * SCALE, inset * SCALE) for z, inset in profile]
    if levels[0][0] != 0 or abs(levels[-1][0] - height) > 1e-10:
        raise ValueError(f"{name} profile must span the full enclosure height")
    outlines = [
        rounded_outline(
            width - 2 * inset,
            length - 2 * inset,
            max(0.001 * SCALE, radius - inset),
            segments,
        )
        for _, inset in levels
    ]
    count = len(outlines[0])
    if any(len(outline) != count for outline in outlines):
        raise AssertionError(f"{name} profile rings do not share a vertex count")
    vertices = [
        (x, y, z - height / 2)
        for (z, _), outline in zip(levels, outlines, strict=True)
        for x, y in outline
    ]
    faces = [tuple(range(count - 1, -1, -1))]
    for ring_index in range(len(outlines) - 1):
        lower = ring_index * count
        upper = (ring_index + 1) * count
        faces.extend(
            (
                lower + index,
                lower + (index + 1) % count,
                upper + (index + 1) % count,
                upper + index,
            )
            for index in range(count)
        )
    faces.append(tuple(range((len(outlines) - 1) * count, len(outlines) * count)))
    mesh = bpy.data.meshes.new(f"{name}_mesh")
    mesh.from_pydata(vertices, [], faces)
    mesh.update()
    assert_convex_outward_normals(mesh, name)
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    obj.location = location
    obj.data.materials.append(mat)
    obj["profile_kind"] = "rolled_underbody"
    obj["profile_levels_m"] = [round(z, 7) for z, _ in levels]
    obj["profile_insets_m"] = [round(inset, 7) for _, inset in levels]
    for polygon in mesh.polygons:
        polygon.use_smooth = abs(polygon.normal.z) < 0.999
    return obj


def rounded_ring(name, location, outer_dimensions, inner_dimensions, radius, mat, collection, segments=16):
    """Create a fine, upward-facing rounded seam in the XY plane."""
    outer_width, outer_height = (value * SCALE for value in outer_dimensions)
    inner_width, inner_height = (value * SCALE for value in inner_dimensions)
    outer = rounded_outline(outer_width, outer_height, radius * SCALE, segments)
    inset = (outer_width - inner_width) / 2
    inner = rounded_outline(
        inner_width,
        inner_height,
        max(0, radius * SCALE - inset),
        segments,
    )
    count = len(outer)
    vertices = [(x, y, 0) for x, y in outer + inner]
    faces = [
        (index, (index + 1) % count, (index + 1) % count + count, index + count)
        for index in range(count)
    ]
    mesh = bpy.data.meshes.new(f"{name}_mesh")
    mesh.from_pydata(vertices, [], faces)
    mesh.update()
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    obj.location = tuple(value * SCALE for value in location)
    obj.data.materials.append(mat)
    return obj


def keycap(name, location, dimensions, radius, mat, collection, segments=6):
    """Build a tapered key with a shallow dished top instead of a flat tile."""
    width, height, depth = (value * SCALE for value in dimensions)
    radius *= SCALE
    z_bottom = -depth / 2
    z_top = depth / 2
    rings = (
        (width - 0.008 * SCALE, height - 0.008 * SCALE, z_bottom),
        (width, height, z_bottom + 0.0025 * SCALE),
        (width - 0.008 * SCALE, height - 0.008 * SCALE, z_top - 0.0025 * SCALE),
        (width - 0.014 * SCALE, height - 0.014 * SCALE, z_top - 0.0012 * SCALE),
        (width - 0.040 * SCALE, height - 0.040 * SCALE, z_top - 0.0022 * SCALE),
    )
    outlines = [
        rounded_outline(ring_width, ring_height, max(0.001 * SCALE, radius - inset), segments)
        for ring_width, ring_height, _, inset in (
            (*rings[0], 0.004 * SCALE),
            (*rings[1], 0),
            (*rings[2], 0.004 * SCALE),
            (*rings[3], 0.007 * SCALE),
            (*rings[4], 0.020 * SCALE),
        )
    ]
    count = len(outlines[0])
    vertices = [
        (x, y, rings[ring_index][2])
        for ring_index, outline in enumerate(outlines)
        for x, y in outline
    ]
    faces = [tuple(range(count - 1, -1, -1))]
    for ring_index in range(len(outlines) - 1):
        lower = ring_index * count
        upper = (ring_index + 1) * count
        faces.extend(
            (
                lower + index,
                lower + (index + 1) % count,
                upper + (index + 1) % count,
                upper + index,
            )
            for index in range(count)
        )
    faces.append(tuple(range((len(outlines) - 1) * count, len(outlines) * count)))
    mesh = bpy.data.meshes.new(f"{name}_mesh")
    mesh.from_pydata(vertices, [], faces)
    mesh.update()
    bottom = mesh.polygons[0]
    top = mesh.polygons[-1]
    shell_faces = mesh.polygons[1:1 + count * (len(outlines) - 2)]
    dish_faces = mesh.polygons[1 + count * (len(outlines) - 2):-1]
    if bottom.normal.z > -0.999 or top.normal.z < 0.999:
        raise AssertionError(f"{name} has an inward-facing cap")
    if any(Vector((polygon.center.x, polygon.center.y)).dot(Vector((polygon.normal.x, polygon.normal.y))) <= 0 for polygon in shell_faces):
        raise AssertionError(f"{name} has an inward-facing key side")
    bad_dish = [(polygon.index, tuple(round(value, 4) for value in polygon.normal)) for polygon in dish_faces if polygon.normal.z <= 0]
    if bad_dish:
        raise AssertionError(f"{name} has an inward-facing dish surface: {bad_dish[:8]}")
    side_faces = [polygon for polygon in shell_faces if abs(polygon.normal.z) < 0.9]
    if not side_faces:
        raise AssertionError(f"{name} has no modeled side surface")
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    obj.location = tuple(value * SCALE for value in location)
    obj.data.materials.append(mat)
    obj["sculpted_keycap"] = True
    obj["dish_depth_m"] = 0.00010
    for polygon in mesh.polygons:
        polygon.use_smooth = abs(polygon.normal.z) < 0.999
    return obj


def cut_recess(target, name, location, dimensions, radius, collection):
    """Cut a real pocket into an enclosure and discard the temporary cutter."""
    cutter = rounded_prism(name, location, dimensions, radius, 0, target.data.materials[0], collection)
    boolean = target.modifiers.new(f"{name}_boolean", "BOOLEAN")
    boolean.operation = "DIFFERENCE"
    boolean.solver = "EXACT"
    boolean.object = cutter
    bpy.context.view_layer.objects.active = target
    bpy.ops.object.modifier_apply(modifier=boolean.name)
    bpy.data.objects.remove(cutter, do_unlink=True)
    manifold = bmesh.new()
    manifold.from_mesh(target.data)
    nonmanifold = [edge.index for edge in manifold.edges if not edge.is_manifold]
    signed_volume = manifold.calc_volume(signed=True)
    manifold.free()
    if nonmanifold:
        raise AssertionError(f"{target.name} recess produced non-manifold edges: {nonmanifold[:8]}")
    if signed_volume <= 0:
        raise AssertionError(f"{target.name} recess reversed the enclosure volume")


def rounded_screen(name, location, width, height, radius, mat, collection):
    width_m, height_m = width * SCALE, height * SCALE
    outline = rounded_outline(width_m, height_m, radius * SCALE, segments=20)
    vertices = [(x, 0, z) for x, z in outline]
    mesh = bpy.data.meshes.new(f"{name}_mesh")
    mesh.from_pydata(vertices, [], [tuple(range(len(vertices)))])
    mesh.update()
    uv_layer = mesh.uv_layers.new(name="UVMap")
    for loop in mesh.loops:
        x, _, z = mesh.vertices[loop.vertex_index].co
        uv_layer.data[loop.index].uv = (x / width_m + 0.5, z / height_m + 0.5)
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    obj.location = tuple(value * SCALE for value in location)
    obj.data.materials.append(mat)
    obj["screen_node"] = True
    mat["screen_aspect"] = width / height
    configure_screen_contain(mat, mat.node_tree.nodes["screen_texture"].image)
    return obj


def join_meshes(objects, name):
    objects = [obj for obj in objects if obj and obj.type == "MESH"]
    if not objects:
        return None
    if len(objects) == 1:
        objects[0].name = name
        return objects[0]
    bpy.ops.object.select_all(action="DESELECT")
    for obj in objects:
        obj.select_set(True)
    bpy.context.view_layer.objects.active = objects[0]
    bpy.ops.object.join()
    objects[0].name = name
    return objects[0]


def cylinder(
    name, location, radius, depth, mat, collection, rotation=(0, 0, 0),
    vertices=32, bevel=0.012,
):
    location = tuple(value * SCALE for value in location)
    radius *= SCALE
    depth *= SCALE
    bpy.ops.mesh.primitive_cylinder_add(vertices=vertices, radius=radius, depth=depth, location=location, rotation=rotation)
    obj = bpy.context.object
    obj.name = name
    obj.data.materials.append(mat)
    if bevel:
        edge_bevel = obj.modifiers.new("edge_bevel", "BEVEL")
        edge_bevel.width = bevel * SCALE
        edge_bevel.segments = 3
        edge_bevel.harden_normals = True
        bpy.context.view_layer.objects.active = obj
        bpy.ops.object.modifier_apply(modifier=edge_bevel.name)
    move_to_collection(obj, collection)
    return obj


def torus(name, location, major_radius, minor_radius, mat, collection, rotation=(0, 0, 0)):
    bpy.ops.mesh.primitive_torus_add(
        major_radius=major_radius * SCALE,
        minor_radius=minor_radius * SCALE,
        major_segments=28,
        minor_segments=6,
        location=tuple(value * SCALE for value in location),
        rotation=rotation,
    )
    obj = bpy.context.object
    obj.name = name
    obj.data.materials.append(mat)
    move_to_collection(obj, collection)
    return obj


def text_mesh(name, body, location, size, mat, collection, rotation=(0, 0, 0)):
    """Create a small, joined-ready legend with no external font dependency."""
    curve = bpy.data.curves.new(f"{name}_curve", "FONT")
    curve.body = body
    curve.align_x = "CENTER"
    curve.align_y = "CENTER"
    curve.size = size * SCALE
    curve.extrude = 0.0002 * SCALE
    curve.resolution_u = 2
    curve.render_resolution_u = 2
    obj = bpy.data.objects.new(name, curve)
    collection.objects.link(obj)
    obj.location = tuple(value * SCALE for value in location)
    obj.rotation_euler = rotation
    obj.data.materials.append(mat)
    bpy.ops.object.select_all(action="DESELECT")
    bpy.context.view_layer.objects.active = obj
    obj.select_set(True)
    bpy.ops.object.convert(target="MESH")
    obj["shadow_excluded"] = True
    return obj


def transform_objects(objects, pivot, rotation=(0, 0, 0)):
    pivot = Vector(tuple(value * SCALE for value in pivot))
    rotation_matrix = (
        Matrix.Rotation(rotation[2], 4, "Z")
        @ Matrix.Rotation(rotation[1], 4, "Y")
        @ Matrix.Rotation(rotation[0], 4, "X")
    )
    transform = Matrix.Translation(pivot) @ rotation_matrix @ Matrix.Translation(-pivot)
    for obj in objects:
        obj.matrix_world = transform @ obj.matrix_world


def parent_rotating_parts(name, objects, pivot, rotation, collection, rest_pivot=None):
    """Parent moving parts at a physical pivot without baking their rest pose."""
    pivot_m = Vector(tuple(value * SCALE for value in pivot))
    parent = bpy.data.objects.new(name, None)
    collection.objects.link(parent)
    parent.empty_display_type = "PLAIN_AXES"
    parent.empty_display_size = 0.08
    parent.location = pivot_m
    parent.rotation_euler = rotation
    parent["hinge_axis"] = "+X"
    parent["closed_rotation_deg"] = 90.0
    parent["default_rotation_deg"] = math.degrees(rotation[0])
    parent["default_open_angle_deg"] = 105.0
    rest_pivot_m = Vector(tuple(value * SCALE for value in (rest_pivot or pivot)))
    rotation_matrix = (
        Matrix.Rotation(rotation[2], 4, "Z")
        @ Matrix.Rotation(rotation[1], 4, "Y")
        @ Matrix.Rotation(rotation[0], 4, "X")
    )
    parent_world = Matrix.Translation(pivot_m) @ rotation_matrix
    rest_transform = Matrix.Translation(rest_pivot_m) @ rotation_matrix @ Matrix.Translation(-rest_pivot_m)
    for obj in objects:
        original_world = obj.matrix_world.copy()
        obj.parent = parent
        obj.matrix_parent_inverse = Matrix.Identity(4)
        obj.matrix_basis = parent_world.inverted() @ rest_transform @ original_world
    return parent


def laptop_lid_point(point):
    """Return a Blender-space point after the authored lid opening transform."""
    pivot = LAPTOP_OPEN_GEOMETRY_PIVOT
    angle = math.radians(LAPTOP_LID_ANGLE_DEG)
    local = Vector(point) - pivot
    return pivot + Matrix.Rotation(angle, 4, "X") @ local


def move_to_collection(obj, collection):
    for owner in list(obj.users_collection):
        owner.objects.unlink(obj)
    collection.objects.link(obj)


def add_collection(name: str):
    collection = bpy.data.collections.new(name)
    bpy.context.scene.collection.children.link(collection)
    return collection


def add_key_row(collection, mats, row_index, y, width_weights, labels, height=0.13):
    gap = 0.018
    row_width = 2.38
    scale = (row_width - gap * (len(width_weights) - 1)) / sum(width_weights)
    key_widths = [weight * scale for weight in width_weights]
    cursor = -row_width / 2
    keys, legends = [], []
    for column, key_width in enumerate(key_widths):
        center_x = cursor + key_width / 2
        label = labels[column] if column < len(labels) else ""
        if row_index == 0 and column == len(key_widths) - 1:
            keys.append(cylinder(
                "touch_id_sensor", (center_x, y, 0.11285), 0.034, 0.0004,
                mats["sensor"], collection, vertices=24, bevel=0,
            ))
        if label == "↑↓":
            half_height = (height - gap * 0.55) / 2
            for direction, y_offset in (("↑", half_height / 2 + gap * 0.14), ("↓", -half_height / 2 - gap * 0.14)):
                keys.append(keycap(
                    f"key_{row_index:02d}_{column:02d}_{direction}",
                    (center_x, y + y_offset, 0.1075),
                    (key_width, half_height, 0.013), 0.010,
                    mats["key"], collection, segments=4,
                ))
                legends.append(text_mesh(
                    f"legend_{row_index:02d}_{column:02d}_{direction}", direction,
                    (center_x, y + y_offset, 0.11286), 0.036, mats["legend"], collection,
                ))
        else:
            keys.append(keycap(
                f"key_{row_index:02d}_{column:02d}",
                (center_x, y, 0.1075),
                (key_width, height, 0.013),
                0.012,
                mats["key"],
                collection,
                segments=6,
            ))
        if label and label != "↑↓":
            legend_size = 0.030 if len(label) > 2 else 0.040
            legends.append(text_mesh(
                f"legend_{row_index:02d}_{column:02d}", label,
                (center_x, y, 0.11286), legend_size, mats["legend"], collection,
            ))
        cursor += key_width + gap
    return keys, legends


def build_keyboard(collection, mats):
    black_parts = [rounded_prism(
        "keyboard_well_floor", (0, -0.70, 0.1030), (2.465, 1.105, 0.001),
        0.0475, 0, mats["black"], collection,
    )]
    row_specs = (
        (-0.22, [1] * 14, ("esc", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", ""), 0.13),
        (-0.40, [1.25] + [1] * 12 + [1.25], ("`", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "=", "delete"), 0.13),
        (-0.59, [1.45] + [1] * 12 + [1.45], ("tab", "Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P", "[", "]", "\\"), 0.13),
        (-0.78, [1.72] + [1] * 11 + [1.72], ("caps", "A", "S", "D", "F", "G", "H", "J", "K", "L", ";", "'", "return"), 0.13),
        (-0.97, [2.20] + [1] * 10 + [2.20], ("shift", "Z", "X", "C", "V", "B", "N", "M", ",", ".", "/", "shift"), 0.13),
        (-1.16, [1.10, 1, 1.20, 1.35, 5.1, 1.35, 1.20, 1, 1, 1], ("fn", "⌃", "⌥", "⌘", "", "⌘", "⌥", "◀", "↑↓", "▶"), 0.13),
    )
    keys, legends = [], []
    for row_index, (y, widths, labels, height) in enumerate(row_specs):
        row_keys, row_legends = add_key_row(collection, mats, row_index, y, widths, labels, height)
        keys.extend(row_keys)
        legends.extend(row_legends)
    grilles = []
    for side in (-1, 1):
        for column in range(5):
            for row in range(18):
                hole = cylinder(
                    f"speaker_{side}_{column}_{row}",
                    (side * (1.315 + column * 0.040), -0.20 - row * 0.057, 0.1115),
                    0.0052,
                    0.002,
                    mats["black"],
                    collection,
                    vertices=8,
                    bevel=0,
                )
                grilles.append(hole)
    return (
        join_meshes(black_parts, "laptop_insets"),
        join_meshes(keys, "keyboard_keys"),
        join_meshes(legends, "keyboard_legends"),
        join_meshes(grilles, "speaker_grilles"),
    )


def build_laptop_ports(collection, mats):
    ports = []
    # Left edge: MagSafe, two Thunderbolt ports, headphone jack.
    for name, y, length, height in (
        ("magsafe", -0.28, 0.24, 0.040),
        ("thunderbolt_1", -0.63, 0.16, 0.034),
        ("thunderbolt_2", -0.88, 0.16, 0.034),
    ):
        ports.append(rounded_box(name, (-1.563, y, 0.060), (0.002, length, height), mats["port"], 0.012, collection))
    ports.append(cylinder(
        "headphone_jack", (-1.563, -1.20, 0.060), 0.025, 0.003, mats["port"], collection,
        rotation=(0, math.radians(90), 0), vertices=20, bevel=0.004,
    ))
    # Right edge: HDMI, Thunderbolt, and SDXC slot.
    ports.extend((
        rounded_box("hdmi", (1.563, -0.38, 0.060), (0.002, 0.23, 0.043), mats["port"], 0.010, collection),
        rounded_box("thunderbolt_3", (1.563, -0.69, 0.060), (0.002, 0.16, 0.034), mats["port"], 0.012, collection),
        rounded_box("sdxc", (1.563, -1.03, 0.068), (0.002, 0.26, 0.014), mats["port"], 0.004, collection),
    ))
    return join_meshes(ports, "laptop_ports")


def build_laptop(mats):
    c = add_collection("Laptop")
    lower_roll = tuple(
        (
            0.040 * (1 - math.cos(math.pi * step / 16)),
            0.018 * (1 - math.sin(math.pi * step / 16)),
        )
        for step in range(9)
    )
    upper_shoulder = tuple(
        (
            0.082 + 0.028 * math.sin(math.pi * step / 12),
            0.008 * (1 - math.cos(math.pi * step / 12)),
        )
        for step in range(1, 7)
    )
    base = profiled_rounded_shell(
        "laptop_base", (0, -1.106, 0.055), (3.126, 2.212, 0.110),
        0.095,
        (
            *lower_roll,
            (0.082, 0.0000),
            *upper_shoulder,
        ),
        mats["graphite"], c,
    )
    cut_recess(
        base,
        "keyboard_well_cutter",
        (0, -0.70, 0.110),
        (2.48, 1.12, 0.0155),
        0.055,
        c,
    )
    hardware = [
        base,
        cylinder("hinge_left", (-1.03, -0.004, 0.096), 0.0175, 0.42, mats["graphite"], c, rotation=(0, math.radians(90), 0), vertices=24),
        cylinder("hinge_right", (1.03, -0.004, 0.096), 0.0175, 0.42, mats["graphite"], c, rotation=(0, math.radians(90), 0), vertices=24),
    ]
    join_meshes(hardware, "laptop_hardware")
    lid_shell = rounded_prism("laptop_lid_shell", (0, 0.00875, 1.160), (3.126, 2.110, 0.0385), 0.060, 0.006, mats["lid"], c, plane="XZ")
    front_glass = rounded_prism("laptop_front_glass", (0, -0.0112, 1.160), (3.086, 2.070, 0.0015), 0.052, 0, mats["glass"], c, plane="XZ")
    screen = rounded_screen("laptop_screen", (0, LAPTOP_SCREEN_Y, 1.160), 3.024, 1.964, 0.045, mats["desktop_screen"], c)
    screen["replaceable_texture"] = True
    screen["runtime_forward"] = "+Z"
    build_keyboard(c, mats)
    rounded_ring(
        "trackpad_seam", (0, -1.695, 0.11012), (1.2745, 0.7045), (1.270, 0.700),
        0.05725, mats["black"], c,
    )
    rounded_prism("trackpad", (0, -1.695, 0.11008), (1.27, 0.70, 0.00012), 0.055, 0, mats["trackpad"], c)
    notch = rounded_prism("camera_notch", (0, -0.0125, 2.126), (0.315, 0.042, 0.002), 0.020, 0, mats["black"], c, plane="XZ")
    notch_bridge = rounded_prism("camera_notch_bridge", (0, -0.0125, 2.171), (0.315, 0.052, 0.002), 0.002, 0, mats["black"], c, plane="XZ", segments=2)
    camera = cylinder(
        "facetime_camera", (0, -0.0138, 2.142), 0.010, 0.0025, mats["sensor"], c,
        rotation=(math.radians(90), 0, 0), vertices=20, bevel=0,
    )
    build_laptop_ports(c, mats)
    # A centered front scoop and fine seam keep the lower enclosure from reading as a slab.
    rounded_box("front_lip_scoop", (0, -2.213, 0.102), (0.62, 0.005, 0.022), mats["port"], 0.010, c)
    parent_rotating_parts(
        "laptop_lid",
        (lid_shell, front_glass, screen, notch, notch_bridge, camera),
        LAPTOP_HINGE,
        (math.radians(LAPTOP_LID_ANGLE_DEG), 0, 0),
        c,
        rest_pivot=LAPTOP_OPEN_GEOMETRY_PIVOT,
    )
    return c


def build_tablet(mats):
    c = add_collection("Tablet")
    tablet_body = rounded_prism("tablet_body", (0, 0, 0), (2.497, 1.775, 0.053), 0.1505, 0.0055, mats["graphite"], c, plane="XZ")
    assign_front_band(tablet_body, mats["front_band"])
    controls = [
        rounded_box("tablet_power", (1.247, 0, 0.55), (0.014, 0.043, 0.22), mats["edge"], 0.006, c),
        rounded_box("tablet_volume", (0.72, 0, 0.886), (0.28, 0.043, 0.014), mats["edge"], 0.006, c),
    ]
    join_meshes(controls, "tablet_controls")
    rounded_prism("tablet_front_glass", (0, -0.0271, 0), (2.467, 1.745, 0.001), 0.1355, 0, mats["glass"], c, plane="XZ")
    screen = rounded_screen(
        "tablet_screen", (0, TABLET_SCREEN_Y, 0),
        TABLET_SCREEN_SIZE_M[0] / SCALE, TABLET_SCREEN_SIZE_M[1] / SCALE,
        0.066, mats["tablet_screen"], c,
    )
    screen["replaceable_texture"] = True
    screen["runtime_forward"] = "+Z"
    cylinder("tablet_landscape_camera", (0, -0.0280, 0.824), 0.012, 0.003, mats["sensor"], c, rotation=(math.radians(90), 0, 0), vertices=20, bevel=0)
    # Rear camera, microphone, Smart Connector, and centered Thunderbolt/USB-C port.
    cylinder("tablet_rear_camera_ring", (-1.035, 0.0335, 0.675), 0.055, 0.018, mats["edge"], c, rotation=(math.radians(90), 0, 0), vertices=28, bevel=0.006)
    cylinder("tablet_rear_camera", (-1.035, 0.044, 0.675), 0.041, 0.010, mats["lens"], c, rotation=(math.radians(90), 0, 0), vertices=28, bevel=0.003)
    cylinder("tablet_rear_mic", (-0.955, 0.031, 0.675), 0.010, 0.007, mats["port"], c, rotation=(math.radians(90), 0, 0), vertices=12, bevel=0)
    usb = rounded_box("tablet_usb_c", (1.249, 0, 0), (0.007, 0.036, 0.128), mats["port"], 0.015, c)
    smart = [cylinder(f"tablet_smart_{index}", (-0.055 + index * 0.055, 0.029, -0.660), 0.010, 0.004, mats["connector"], c, rotation=(math.radians(90), 0, 0), vertices=12, bevel=0) for index in range(3)]
    join_meshes([usb, *smart], "tablet_connectors")
    return c


def build_phone(mats):
    c = add_collection("Phone")
    phone_body = rounded_prism("phone_body", (0, 0, 0), (0.780, 1.634, 0.0875), 0.154, 0.0080, mats["phone_aluminum"], c, plane="XZ")
    assign_front_band(phone_body, mats["front_band"])
    controls = [
        rounded_box("phone_action", (-0.388, 0, 0.42), (0.013, 0.069, 0.13), mats["phone_edge"], 0.005, c),
        rounded_box("phone_volume_up", (-0.388, 0, 0.17), (0.013, 0.069, 0.18), mats["phone_edge"], 0.005, c),
        rounded_box("phone_volume_down", (-0.388, 0, -0.07), (0.013, 0.069, 0.18), mats["phone_edge"], 0.005, c),
        rounded_box("phone_side", (0.388, 0, 0.28), (0.013, 0.069, 0.30), mats["phone_edge"], 0.005, c),
        rounded_box("phone_camera_control", (0.388, 0, -0.27), (0.013, 0.069, 0.24), mats["phone_edge"], 0.005, c),
    ]
    join_meshes(controls, "phone_controls")
    rounded_prism("phone_front_glass", (0, -0.0444, 0), (0.764, 1.618, 0.001), 0.146, 0, mats["glass"], c, plane="XZ")
    screen = rounded_screen(
        "phone_screen", (0, PHONE_SCREEN_Y, 0),
        PHONE_SCREEN_SIZE_M[0] / SCALE, PHONE_SCREEN_SIZE_M[1] / SCALE,
        0.140, mats["phone_screen"], c,
    )
    screen["replaceable_texture"] = True
    screen["runtime_forward"] = "+Z"
    rounded_prism("phone_island", (0, -0.0452, 0.745), (0.198, 0.056, 0.002), 0.028, 0, mats["black"], c, plane="XZ")
    # The 17 Pro generation uses a forged full-width aluminum camera plateau and
    # a contrasting Ceramic Shield back inset rather than the old square island.
    rounded_prism("phone_back_glass", (0, 0.0445, -0.205), (0.668, 0.775, 0.0015), 0.105, 0, mats["phone_back"], c, plane="XZ")
    rounded_prism("phone_camera_plateau", (0, 0.052, 0.575), (0.760, 0.420, 0.026), 0.115, 0.007, mats["phone_aluminum"], c, plane="XZ")
    lens_positions = ((-0.205, 0.680), (-0.205, 0.438), (0.055, 0.560))
    camera_parts = []
    for index, (x, z) in enumerate(lens_positions, start=1):
        camera_parts.extend((
            cylinder(f"phone_camera_ring_{index}", (x, 0.071, z), 0.090, 0.033, mats["camera_ring"], c, rotation=(math.radians(90), 0, 0), vertices=32, bevel=0.006),
            cylinder(f"phone_camera_lens_{index}", (x, 0.090, z), 0.068, 0.013, mats["lens"], c, rotation=(math.radians(90), 0, 0), vertices=32, bevel=0.003),
        ))
    cylinder("phone_flash", (0.270, 0.071, 0.675), 0.031, 0.010, mats["flash"], c, rotation=(math.radians(90), 0, 0), vertices=24, bevel=0.002)
    cylinder("phone_lidar", (0.270, 0.071, 0.455), 0.026, 0.010, mats["port"], c, rotation=(math.radians(90), 0, 0), vertices=24, bevel=0.002)
    rounded_box("phone_usb_c", (0, 0, -0.817), (0.155, 0.040, 0.007), mats["port"], 0.016, c)
    speaker_slots = [
        rounded_box(f"phone_speaker_{side}_{index}", (side * (0.17 + index * 0.045), 0, -0.817), (0.021, 0.035, 0.007), mats["port"], 0.006, c)
        for side in (-1, 1) for index in range(4)
    ]
    join_meshes([*camera_parts, *speaker_slots], "phone_camera_and_speakers")
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


def add_area(name, location, energy, size, color, target, shape="DISK", size_y=None):
    data = bpy.data.lights.new(name, "AREA")
    # Preserve the authored exposure when the whole physical scene is scaled to meters.
    data.energy = energy * SCALE * SCALE
    data.shape = shape
    data.size = size * SCALE
    if shape == "RECTANGLE" and size_y is not None:
        data.size_y = size_y * SCALE
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
    background.inputs["Color"].default_value = (0.0056, 0.0065, 0.0080, 1.0)
    background.inputs["Strength"].default_value = 0.12


def add_floor(size=14, z=-0.075, black=True):
    bpy.ops.mesh.primitive_plane_add(size=size * SCALE, location=(0, 0, z * SCALE))
    floor = bpy.context.object
    floor.name = "render_floor"
    floor.data.materials.append(bpy.data.materials["RenderFloorBlack" if black else "RenderFloorClear"])
    return floor


def fit_device_framing(camera, margin=0.05):
    """Keep the complete hardware inside each responsive poster's safe frame."""
    bpy.context.view_layer.update()
    points = []
    for name in ("Laptop", "Tablet", "Phone"):
        collection = bpy.data.collections.get(name)
        if not collection or collection.hide_render:
            continue
        for obj in collection_objects(collection):
            if obj.type == "MESH":
                points.extend(obj.matrix_world @ Vector(corner) for corner in obj.bound_box)
    projected = [world_to_camera_view(bpy.context.scene, camera, point) for point in points]
    extent = max((max(abs(point.x - 0.5), abs(point.y - 0.5)) * 2 for point in projected), default=0)
    factor = max(1.0, extent / (1 - 2 * margin))
    if camera.data.type == "ORTHO":
        camera.data.ortho_scale *= factor
    else:
        camera.data.lens /= factor


def linear_rgb(hex_color):
    """Decode the runtime's sRGB light colours for Blender's linear inputs."""
    values = [(hex_color >> shift & 255) / 255 for shift in (16, 8, 0)]
    return tuple(value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4 for value in values)


def satin_environment():
    """Bake the film's eight emissive cards into a linear environment image."""
    image = bpy.data.images.get("render_satin_environment")
    if image:
        return image
    cards = (
        ((3.5, 5), (-4, 3, 5), 0xfffcf6, 4.0),
        ((0.85, 5), (4, 1, 3), 0xf1f4fa, 1.7),
        ((2.2, 1), (-1.8, 5, 1), 0xffffff, 2.2),
        ((6, 2), (-1, 0.1, 5), 0xf0f3f7, 0.9),
        ((1.6, 3), (-3.2, 1.2, -5), 0xffffff, 1.65),
        ((0.6, 3), (-0.8, 1.2, -5), 0xffffff, 1.05),
        ((2.4, 3), (2.2, 1.2, -5), 0xffffff, 0.75),
        ((0.4, 3), (4.6, 1.2, -5), 0xffffff, 2.25),
    )
    width, height = 2048, 1024
    longitude = ((np.arange(width) + 0.5) / width - 0.5) * 2 * math.pi
    latitude = ((np.arange(height) + 0.5) / height - 0.5) * math.pi
    horizontal = np.cos(latitude)[:, None]
    # Equirectangular Blender rays converted from Z-up to the film's Y-up.
    x = horizontal * np.cos(longitude)[None, :]
    y = -horizontal * np.sin(longitude)[None, :]
    z = np.broadcast_to(np.sin(latitude)[:, None], (height, width))
    rays = np.stack((x, z, -y), axis=-1)
    pixels = np.empty((height, width, 4), dtype=np.float32)
    pixels[:, :, :3] = linear_rgb(0x0b0c0e)
    pixels[:, :, 3] = 1
    nearest = np.full((height, width), np.inf)
    for (card_width, card_height), position, color, intensity in cards:
        center = np.array(position, dtype=float)
        normal = -center / np.linalg.norm(center)
        right = np.cross((0, 1, 0), normal)
        right /= np.linalg.norm(right)
        up = np.cross(normal, right)
        denominator = rays @ normal
        distance = np.full_like(nearest, np.inf)
        np.divide(center @ normal, denominator, out=distance, where=np.abs(denominator) > 1e-9)
        points = rays * distance[:, :, None] - center
        inside = (
            (distance > 0) & (distance < nearest)
            & (np.abs(points @ right) <= card_width / 2)
            & (np.abs(points @ up) <= card_height / 2)
        )
        pixels[inside, :3] = np.array(linear_rgb(color)) * intensity
        nearest[inside] = distance[inside]
    image = bpy.data.images.new("render_satin_environment", width=width, height=height, float_buffer=True)
    image.pixels.foreach_set(pixels.ravel())
    image.update()
    return image


def add_satin_studio():
    """Use the film's reflected studio and direct lights with Blender Z-up."""
    world = bpy.context.scene.world
    nodes, links = world.node_tree.nodes, world.node_tree.links
    nodes.clear()
    environment = nodes.new("ShaderNodeTexEnvironment")
    environment.image = satin_environment()
    reflected = nodes.new("ShaderNodeBackground")
    links.new(environment.outputs["Color"], reflected.inputs["Color"])
    background = nodes.new("ShaderNodeBackground")
    background.name = "Background"
    background.inputs["Color"].default_value = (0.0056, 0.0065, 0.0080, 1)
    background.inputs["Strength"].default_value = 0.12
    path = nodes.new("ShaderNodeLightPath")
    mix = nodes.new("ShaderNodeMixShader")
    links.new(path.outputs["Is Camera Ray"], mix.inputs[0])
    links.new(reflected.outputs[0], mix.inputs[1])
    links.new(background.outputs[0], mix.inputs[2])
    output = nodes.new("ShaderNodeOutputWorld")
    links.new(mix.outputs[0], output.inputs["Surface"])
    for name, (x, y, z), color, energy in (
        ("key", (-4, 5, 6), 0xfffcf6, 0.9),
        ("fill", (5, 1, 4), 0xe8edf5, 0.35),
        ("edge", (2, 4, -5), 0xf2f4f3, 0.5),
    ):
        data = bpy.data.lights.new(f"render_{name}", "SUN")
        data.energy = energy
        data.use_shadow = name == "key"
        data.shadow_maximum_resolution = 0.0001
        data.color = linear_rgb(color)
        obj = bpy.data.objects.new(data.name, data)
        bpy.context.scene.collection.objects.link(obj)
        obj.location = (x, -z, y)
        aim_camera(obj, (0, 0, 0))


def render(path, width, height, camera_location, target, lens, transparent, floor=False, ortho_scale=None):
    clear_render_rig()
    setup_render(width, height, transparent)
    bpy.context.scene.render.image_settings.file_format = "PNG" if path.suffix.lower() == ".png" else "WEBP"
    camera = add_camera("render_camera", camera_location, target, lens, ortho_scale=ortho_scale)
    fit_device_framing(camera)
    add_satin_studio()
    if floor:
        add_floor(14, -0.075, black=True)
    bpy.context.scene.render.filepath = str(path)
    bpy.ops.render.render(write_still=True)


def render_social(collections, mats):
    laptop = collections["laptop"]
    set_screen_texture(mats["desktop_screen"], SCREENS_DIR / "ui10-editor-macbook.webp")
    laptop_state = transform_collection(laptop, (0, -1.0, 1.12), offset=(1.55, 0, -0.05), scale=0.72)
    set_visible_collections(laptop)
    bpy.ops.object.text_add(location=(-0.24, -0.08, 0.075), rotation=(math.radians(90), 0, 0))
    headline = bpy.context.object
    headline.name = "social_headline"
    headline.data.body = "Your agents.\nYour machine.\nYour call."
    headline.data.align_x = "LEFT"
    headline.data.align_y = "CENTER"
    headline.data.size = 0.038
    headline.data.space_line = 0.92
    headline.data.extrude = 0.0
    social_font = SOURCE_DIR / "fonts" / "Inter-Bold.ttf"
    if not social_font.exists():
        raise FileNotFoundError(f"Social render requires the bundled Inter font: {social_font}")
    headline.data.font = bpy.data.fonts.load(str(social_font), check_existing=True)
    headline_material = bpy.data.materials.new("SocialHeadline")
    headline_material.diffuse_color = (0.87, 1.0, 0.94, 1)
    headline_material.use_nodes = True
    headline_bsdf = headline_material.node_tree.nodes.get("Principled BSDF")
    headline_bsdf.inputs["Base Color"].default_value = (0.87, 1.0, 0.94, 1)
    headline_bsdf.inputs["Roughness"].default_value = 0.55
    headline_bsdf.inputs["Emission Color"].default_value = (0.87, 1.0, 0.94, 1)
    headline_bsdf.inputs["Emission Strength"].default_value = 1.0
    headline.data.materials.append(headline_material)
    paths = [OUTPUT_DIR / "social-preview.webp", LANDING_ASSETS / "social-preview.png"]
    for path in paths:
        render(path, 1200, 630, (0.2, -7.4, 1.6), (0.0, -0.5, 0.75), 58, False, ortho_scale=5.6)
    bpy.data.objects.remove(headline, do_unlink=True)
    restore_matrices(laptop, laptop_state)
    return paths


def render_cutouts(collections, mats):
    jobs = (
        ("laptop", "desktop_screen", "ui10-editor-macbook.webp", "hero-laptop.webp", 1600, 1180, LAPTOP_RENDER_CAMERA, LAPTOP_RENDER_TARGET, LAPTOP_RENDER_LENS),
        ("laptop", "desktop_screen", "ui10-editor-macbook.webp", "laptop.webp", 1200, 900, LAPTOP_RENDER_CAMERA, LAPTOP_RENDER_TARGET, LAPTOP_RENDER_LENS),
        ("tablet", "tablet_screen", "ui05-merged-ipad.webp", "tablet.webp", 1000, 760, (2.45, -5.0, 1.92), (0, 0, 0), 67),
        ("phone", "phone_screen", "ui03-answer-iphone.webp", "phone.webp", 640, 1040, (1.45, -3.95, 1.10), (0, 0, 0), 72),
    )
    paths = []
    for device, material_name, fixture, filename, width, height, camera, target, lens in jobs:
        set_screen_texture(mats[material_name], SCREENS_DIR / fixture)
        set_visible_collections(collections[device])
        path = OUTPUT_DIR / filename
        render(path, width, height, camera, target, lens, True)
        paths.append(path)
    tablet, phone = collections["tablet"], collections["phone"]
    tablet_state, phone_state = add_mobile_composition(tablet, phone)
    set_visible_collections(tablet, phone)
    path = OUTPUT_DIR / "mobile-hero.webp"
    render(path, 1200, 1350, (2.85, -6.6, 2.20), (-0.15, 0, 0.02), 70, True)
    restore_matrices(tablet, tablet_state)
    restore_matrices(phone, phone_state)
    paths.append(path)
    return paths


def update_render_metadata(paths, fixture_map):
    metadata_path = OUTPUT_DIR / "metadata.json"
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    renders = metadata.setdefault("renders", {})
    for path in paths:
        metadata_key = "social-preview-og" if path.parent == LANDING_ASSETS else path.stem
        info = image_info(path)
        if path.parent != OUTPUT_DIR:
            info["file"] = f"../{path.name}"
        fixtures = fixture_map.get(metadata_key, {})
        info["provenance"] = {
            "blend": BLEND_PATH.name,
            "generator": Path(__file__).name,
            "blender": bpy.app.version_string,
            "screen_fixtures": {
                device: {
                    "file": str(fixture.relative_to(LANDING_ASSETS)),
                    "sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
                }
                for device, fixture in fixtures.items()
            },
        }
        renders[metadata_key] = info
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")


def render_only(group):
    if Path(bpy.data.filepath).resolve() != BLEND_PATH.resolve():
        raise RuntimeError(f"--render-only requires opening {BLEND_PATH} before --python")
    collections = {name.lower(): bpy.data.collections[name] for name in ("Laptop", "Tablet", "Phone")}
    mats = {
        "desktop_screen": bpy.data.materials["ScreenDesktop"],
        "tablet_screen": bpy.data.materials["ScreenTablet"],
        "phone_screen": bpy.data.materials["ScreenPhone"],
    }
    defaults = {
        "laptop": SCREENS_DIR / "ui10-editor-macbook.webp",
        "tablet": SCREENS_DIR / "ui05-merged-ipad.webp",
        "phone": SCREENS_DIR / "ui03-answer-iphone.webp",
    }
    if group == "closing":
        paths = render_closing_posters(collections, mats)
        fixtures = {
            "laptop": SCREENS_DIR / "ui05-merged-macbook.webp",
            "tablet": SCREENS_DIR / "ui05-merged-ipad.webp",
            "phone": SCREENS_DIR / "ui05-merged-iphone.webp",
        }
        update_render_metadata(paths, {path.stem: fixtures for path in paths})
        print(f"Rendered closing assets from {BLEND_PATH}")
        return
    groups = ("posters", "cutouts", "social") if group == "all" else (group,)
    paths = []
    fixture_map = {}
    if "posters" in groups:
        paths.extend(render_scene_posters(collections, mats, defaults))
        scene_specs = {
            1: {"laptop": "ui10-editor-macbook.webp"}, 2: {"laptop": "ui10-editor-macbook.webp"},
            3: {"laptop": "ui12-tasks-macbook.webp"},
            4: {"laptop": "ui13-team-macbook.webp", "phone": "ui03-answer-iphone.webp"},
            5: {"laptop": "ui16-builder-macbook.webp"}, 6: {"laptop": "ui14-git-macbook.webp"},
            7: {"laptop": "ui05-merged-macbook.webp", "tablet": "ui05-merged-ipad.webp"},
            8: {"laptop": "ui05-merged-macbook.webp", "tablet": "ui05-merged-ipad.webp", "phone": "ui05-merged-iphone.webp"},
        }
        for index, specs in scene_specs.items():
            fixtures = {device: SCREENS_DIR / file for device, file in specs.items()}
            fixture_map[f"scene-{index:02d}-desktop"] = fixtures
            fixture_map[f"scene-{index:02d}-mobile"] = fixtures
        fixture_map["desktop-poster"] = fixture_map["scene-08-desktop"]
        fixture_map["mobile-poster"] = fixture_map["scene-08-mobile"]
    if "cutouts" in groups:
        cutouts = render_cutouts(collections, mats)
        paths.extend(cutouts)
        for name, fixture in defaults.items():
            fixture_map[name] = {name: fixture}
        fixture_map["hero-laptop"] = {"laptop": defaults["laptop"]}
        fixture_map["mobile-hero"] = {name: defaults[name] for name in ("tablet", "phone")}
    if "social" in groups:
        social = render_social(collections, mats)
        paths.extend(social)
        fixture_map["social-preview"] = {"laptop": defaults["laptop"]}
        fixture_map["social-preview-og"] = {"laptop": defaults["laptop"]}
    update_render_metadata(paths, fixture_map)
    print(f"Rendered {group} assets from {BLEND_PATH}")


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
    objects = collection_objects(collection)
    object_set = set(objects)
    roots = [obj for obj in objects if obj.parent not in object_set]
    snapshots = {obj.name: obj.matrix_world.copy() for obj in roots}
    pivot = Vector(tuple(value * SCALE for value in pivot))
    offset = Vector(tuple(value * SCALE for value in offset))
    rotation_matrix = Matrix.Rotation(rotation[2], 4, "Z") @ Matrix.Rotation(rotation[1], 4, "Y") @ Matrix.Rotation(rotation[0], 4, "X")
    transform = Matrix.Translation(offset) @ Matrix.Translation(pivot) @ rotation_matrix @ Matrix.Scale(scale, 4) @ Matrix.Translation(-pivot)
    for obj in roots:
        obj.matrix_world = transform @ obj.matrix_world
    return snapshots


def restore_matrices(collection, snapshots):
    for obj in collection_objects(collection):
        if obj.name in snapshots:
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


def collection_bounds_size(collection):
    """Return evaluated outer bounds in exported glTF axis order (X, Y-up, Z)."""
    bpy.context.view_layer.update()
    points = [
        obj.matrix_world @ Vector(corner)
        for obj in collection_objects(collection)
        if obj.type == "MESH"
        for corner in obj.bound_box
    ]
    blender_size = [
        max(getattr(point, axis) for point in points) - min(getattr(point, axis) for point in points)
        for axis in "xyz"
    ]
    return [blender_size[0], blender_size[2], blender_size[1]]


def image_info(path):
    image = bpy.data.images.load(str(path), check_existing=False)
    dimensions = [image.size[0], image.size[1]]
    content_bounds = None
    if image.channels >= 3:
        pixels = np.empty(image.size[0] * image.size[1] * image.channels, dtype=np.float32)
        image.pixels.foreach_get(pixels)
        pixels = pixels.reshape((image.size[1], image.size[0], image.channels))
        if image.channels == 4 and np.any(pixels[:, :, 3] < 0.99):
            visible = pixels[:, :, 3] > 0.01
        else:
            # Posters have a flat near-black world. Use the corner colour as the
            # authored background so bounds describe lit hardware, not the canvas.
            background = np.median(np.concatenate((pixels[:4, :4, :3], pixels[-4:, -4:, :3])), axis=(0, 1))
            visible = np.max(np.abs(pixels[:, :, :3] - background), axis=2) > 0.012
        visible_y, visible_x = np.where(visible)
        if visible_x.size:
            min_x, max_x = int(visible_x.min()), int(visible_x.max())
            min_y, max_y = int(visible_y.min()), int(visible_y.max())
            content_bounds = [min_x, min_y, max_x - min_x + 1, max_y - min_y + 1]
    bpy.data.images.remove(image)
    info = {
        "file": path.name, "width": dimensions[0], "height": dimensions[1],
        "bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }
    if content_bounds:
        info["content_bounds_px_bottom_left"] = content_bounds
    return info


def render_closing_posters(collections, mats):
    """Render the physically scaled Act 8 lineup for both aspect ratios."""
    laptop, tablet, phone = (collections[name] for name in ("laptop", "tablet", "phone"))
    for material, fixture in (
        ("desktop_screen", "ui05-merged-macbook.webp"),
        ("tablet_screen", "ui05-merged-ipad.webp"),
        ("phone_screen", "ui05-merged-iphone.webp"),
    ):
        set_screen_texture(mats[material], SCREENS_DIR / fixture)
    states = (
        (laptop, transform_collection(laptop, (0, -1.0, 1.12), offset=(-1.25, 0, -0.20), scale=0.62,
                                      rotation=(math.radians(8), 0, math.radians(4)))),
        (tablet, transform_collection(tablet, (0, 0, 0), offset=(0.73, 0.05, 0.50), scale=0.62,
                                      rotation=(math.radians(3), 0, math.radians(-8)))),
        (phone, transform_collection(phone, (0, 0, 0), offset=(2.00, -0.14, 0.50), scale=0.62,
                                    rotation=(math.radians(2), 0, math.radians(-10)))),
    )
    set_visible_collections(laptop, tablet, phone)
    desktop = OUTPUT_DIR / "scene-08-desktop.webp"
    mobile = OUTPUT_DIR / "scene-08-mobile.webp"
    try:
        render(desktop, 1440, 900, (0.70, -7.0, 1.65), (0, -0.45, 0.76), 60, False, ortho_scale=5.0)
        render(mobile, 720, 960, (1.0, -7.2, 2.12), (0.1, -0.45, 0.87), 60, False, ortho_scale=5.1)
    finally:
        for collection, state in states:
            restore_matrices(collection, state)
    desktop_alias = OUTPUT_DIR / "desktop-poster.webp"
    mobile_alias = OUTPUT_DIR / "mobile-poster.webp"
    shutil.copyfile(desktop, desktop_alias)
    shutil.copyfile(mobile, mobile_alias)
    return [desktop, mobile, desktop_alias, mobile_alias]


def render_scene_posters(collections, mats, default_screens):
    laptop, tablet, phone = collections["laptop"], collections["tablet"], collections["phone"]
    scenes = [
        ("ui10-editor", None, None),
        ("ui10-editor", None, None),
        ("ui12-tasks", None, None),
        ("ui13-team", None, "ui03-answer"),
        ("ui16-builder", None, None),
        ("ui14-git", None, None),
        ("ui05-merged", "ui05-merged", None),
        ("ui05-merged", "ui05-merged", "ui05-merged"),
    ]
    render_paths = []
    for index, (laptop_stem, tablet_stem, phone_stem) in enumerate(scenes, start=1):
        desktop_fixture = SCREENS_DIR / f"{laptop_stem}-macbook.webp"
        set_screen_texture(mats["desktop_screen"], desktop_fixture)
        if tablet_stem:
            set_screen_texture(mats["tablet_screen"], SCREENS_DIR / f"{tablet_stem}-ipad.webp")
        if phone_stem:
            set_screen_texture(mats["phone_screen"], SCREENS_DIR / f"{phone_stem}-iphone.webp")
        desktop_path = OUTPUT_DIR / f"scene-{index:02d}-desktop.webp"
        mobile_path = OUTPUT_DIR / f"scene-{index:02d}-mobile.webp"

        if index == 1:
            set_visible_collections(laptop)
            render(desktop_path, 1440, 900, LAPTOP_RENDER_CAMERA, LAPTOP_RENDER_TARGET, LAPTOP_RENDER_LENS, False)
            render(mobile_path, 720, 960, LAPTOP_RENDER_CAMERA, LAPTOP_RENDER_TARGET, LAPTOP_RENDER_LENS, False)
        elif index == 2:
            laptop_state = transform_collection(laptop, (0, -1.0, 1.12), scale=0.78, rotation=(0, 0, math.radians(12)))
            set_visible_collections(laptop)
            render(desktop_path, 1440, 900, (0.9, -7.2, 1.55), (0.15, -0.55, 1.15), 58, False, ortho_scale=4.2)
            render(mobile_path, 720, 960, (0.9, -7.2, 1.55), (0.15, -0.55, 1.15), 58, False, ortho_scale=4.2)
            restore_matrices(laptop, laptop_state)
        elif index == 3:
            set_visible_collections(laptop)
            render(desktop_path, 1440, 900, (-2.6, -6.2, 2.0), (0, -0.7, 0.8), 62, False)
            render(mobile_path, 720, 960, (-2.6, -6.2, 2.0), (0, -0.7, 0.8), 62, False)
        elif index == 4:
            laptop_state = transform_collection(laptop, (0, -1.0, 1.12), offset=(-0.82, 0, 0), scale=0.72)
            phone_state = transform_collection(phone, (0, 0, 0), offset=(1.33, -0.10, -0.18), scale=1.05)
            set_visible_collections(laptop, phone)
            render(desktop_path, 1440, 900, (0.8, -7.0, 1.7), (0.1, -0.45, 0.72), 58, False, ortho_scale=4.5)
            render(mobile_path, 720, 960, (0.8, -7.0, 1.7), (0.1, -0.45, 0.72), 58, False, ortho_scale=4.5)
            restore_matrices(laptop, laptop_state)
            restore_matrices(phone, phone_state)
        elif index in {5, 6}:
            set_visible_collections(laptop)
            camera = LAPTOP_RENDER_CAMERA if index == 5 else (0.35, -6.0, 1.30)
            target = LAPTOP_RENDER_TARGET if index == 5 else (0, -0.8, 1.05)
            render(desktop_path, 1440, 900, camera, target, 64 if index == 5 else 72, False)
            render(mobile_path, 720, 960, camera, target, 64 if index == 5 else 72, False)
        elif index == 7:
            laptop_state = transform_collection(laptop, (0, -1.0, 1.12), offset=(-1.05, 0, 0), scale=0.60)
            # Face the tablet back into the offset camera so its review UI stays frontal.
            tablet_state = transform_collection(
                tablet, (0, 0, 0), offset=(0.92, 0.02, 0.31), scale=0.68,
                rotation=(math.radians(-8), 0, math.radians(6)),
            )
            set_visible_collections(laptop, tablet)
            render(desktop_path, 1440, 900, (0.9, -7.0, 1.6), (0.2, -0.4, 0.72), 58, False, ortho_scale=4.8)
            render(mobile_path, 720, 960, (0.9, -7.0, 1.6), (0.2, -0.4, 0.72), 58, False, ortho_scale=4.8)
            restore_matrices(laptop, laptop_state)
            restore_matrices(tablet, tablet_state)
        else:
            render_paths.extend(render_closing_posters(collections, mats))
            continue

        render_paths.extend([desktop_path, mobile_path])

    set_screen_texture(mats["desktop_screen"], default_screens["laptop"])
    set_screen_texture(mats["tablet_screen"], default_screens["tablet"])
    set_screen_texture(mats["phone_screen"], default_screens["phone"])
    return render_paths


def render_previews(preview_dir, collections):
    preview_dir.mkdir(parents=True, exist_ok=True)
    laptop, tablet, phone = collections["laptop"], collections["tablet"], collections["phone"]
    for name, collection in collections.items():
        export_glb(collection, preview_dir / f"{name}.glb")
    export_glb(laptop, preview_dir / "laptop-low.glb", include_decorative=False)
    set_collection_visibility(laptop)
    render(preview_dir / "laptop-candidate.webp", 1200, 900, LAPTOP_RENDER_CAMERA, LAPTOP_RENDER_TARGET, LAPTOP_RENDER_LENS, True)
    set_collection_visibility(tablet)
    render(preview_dir / "tablet-candidate.webp", 1000, 760, (2.45, -5.0, 1.92), (0, 0, 0), 67, True)
    set_collection_visibility(phone)
    render(preview_dir / "phone-candidate.webp", 640, 1040, (1.45, -3.95, 1.10), (0, 0, 0), 72, True)
    render(preview_dir / "phone-back-candidate.webp", 760, 1040, (-1.55, 4.15, 1.18), (0, 0, 0.05), 72, True)


def device_specs(collections=None):
    screen_center_blender = laptop_lid_point((0, LAPTOP_SCREEN_Y, 1.160)) * SCALE
    screen_corners_blender = [
        laptop_lid_point((x, LAPTOP_SCREEN_Y, z)) * SCALE
        for x, z in (
            (-1.512, 0.178), (1.512, 0.178),
            (1.512, 2.142), (-1.512, 2.142),
        )
    ]
    to_gltf = lambda point: [round(point.x, 7), round(point.z, 7), round(-point.y, 7)]
    lid_top = laptop_lid_point((0, 0.010, 2.215)) * SCALE
    lid_rear = max(point.y for point in screen_corners_blender) + 0.0021
    tablet_half_screen = (TABLET_SCREEN_SIZE_M[0] / 2, TABLET_SCREEN_SIZE_M[1] / 2)
    phone_half_screen = (PHONE_SCREEN_SIZE_M[0] / 2, PHONE_SCREEN_SIZE_M[1] / 2)
    specs = {
        "laptop": {
            "model": "MacBook Pro 14-inch (M5, 2025)",
            "dimensions_m": [0.3126, round(lid_top.z, 7), round(0.2212 + lid_rear, 7)],
            "body_size_m": [0.3126, 0.0110, 0.2212],
            "body_corner_radius_m": 0.0095,
            "closed_height_m": 0.0155,
            "lid_size_m": [0.3126, 0.2110, 0.0041],
            "lid_corner_radius_m": 0.0060,
            "lid_open_angle_deg": 105.0,
            "lid_hinge_node": "laptop_lid",
            "lid_hinge_axis": "+X",
            "lid_hinge_closed_rotation_deg": 90.0,
            "lid_hinge_default_rotation_deg": LAPTOP_LID_ANGLE_DEG,
            "screen_size_m": list(LAPTOP_SCREEN_SIZE_M),
            "screen_center_m": to_gltf(screen_center_blender),
            "screen_corner_radius_m": 0.0045,
            "screen_surface_clearance_m": 0.000105,
            "screen_corners_m": [to_gltf(point) for point in screen_corners_blender],
            "hinge_pivot_m": [
                round(LAPTOP_HINGE.x * SCALE, 7),
                round(LAPTOP_HINGE.z * SCALE, 7),
                round(-LAPTOP_HINGE.y * SCALE, 7),
            ],
        },
        "tablet": {
            "model": "iPad Pro 11-inch (M5, 2025)",
            "dimensions_m": [0.2497, 0.1775, 0.0082],
            "body_size_m": [0.2497, 0.1775, 0.0053],
            "body_corner_radius_m": 0.01505,
            "screen_size_m": list(TABLET_SCREEN_SIZE_M),
            "screen_center_m": [0, 0, round(-TABLET_SCREEN_Y * SCALE, 7)],
            "screen_corner_radius_m": 0.0066,
            "screen_surface_clearance_m": 0.000090,
            "screen_corners_m": [
                [-tablet_half_screen[0], -tablet_half_screen[1], round(-TABLET_SCREEN_Y * SCALE, 7)],
                [tablet_half_screen[0], -tablet_half_screen[1], round(-TABLET_SCREEN_Y * SCALE, 7)],
                [tablet_half_screen[0], tablet_half_screen[1], round(-TABLET_SCREEN_Y * SCALE, 7)],
                [-tablet_half_screen[0], tablet_half_screen[1], round(-TABLET_SCREEN_Y * SCALE, 7)],
            ],
        },
        "phone": {
            "model": "iPhone 17 Pro Max (2025)",
            "dimensions_m": [0.0780, 0.1634, 0.01403],
            "body_size_m": [0.0780, 0.1634, 0.00875],
            "body_corner_radius_m": 0.0154,
            "screen_size_m": list(PHONE_SCREEN_SIZE_M),
            "screen_center_m": [0, 0, round(-PHONE_SCREEN_Y * SCALE, 7)],
            "screen_corner_radius_m": 0.0140,
            "screen_surface_clearance_m": 0.000100,
            "screen_corners_m": [
                [-phone_half_screen[0], -phone_half_screen[1], round(-PHONE_SCREEN_Y * SCALE, 7)],
                [phone_half_screen[0], -phone_half_screen[1], round(-PHONE_SCREEN_Y * SCALE, 7)],
                [phone_half_screen[0], phone_half_screen[1], round(-PHONE_SCREEN_Y * SCALE, 7)],
                [-phone_half_screen[0], phone_half_screen[1], round(-PHONE_SCREEN_Y * SCALE, 7)],
            ],
        },
    }
    if collections:
        for name, collection in collections.items():
            specs[name]["dimensions_m"] = collection_bounds_size(collection)
    return specs


def write_device_contract(specs):
    contract = {
        "version": 1,
        "units": "meters",
        "axes": {"up": "+Y", "forward": "+Z", "right": "+X"},
        "origin": "device center; laptop origin is rear-center hinge projection on base underside",
        "devices": {},
    }
    for name, spec in specs.items():
        device = {
            "model": spec["model"],
            "bounds_size_m": spec["dimensions_m"],
            "body_size_m": spec["body_size_m"],
            "body_corner_radius_m": spec["body_corner_radius_m"],
            "screen": {
                "node": "screen",
                "size_m": spec["screen_size_m"],
                "center_m": spec["screen_center_m"],
                "corners_m": spec["screen_corners_m"],
                "corner_radius_m": spec["screen_corner_radius_m"],
                "surface_clearance_m": spec["screen_surface_clearance_m"],
            },
        }
        for key in (
            "closed_height_m", "lid_size_m", "lid_corner_radius_m", "lid_open_angle_deg",
            "lid_hinge_node", "lid_hinge_axis", "lid_hinge_closed_rotation_deg",
            "lid_hinge_default_rotation_deg", "hinge_pivot_m",
        ):
            if key in spec:
                device[key] = spec[key]
        contract["devices"][name] = device
    payload = json.dumps(contract, indent=2)
    source = "const deepFreeze = value => {\n  Object.values(value).forEach(child => {\n    if (child && typeof child === 'object') deepFreeze(child);\n  });\n  return Object.freeze(value);\n};\n\n"
    source += f"export const DEVICE_CONTRACT = deepFreeze({payload});\n"
    (OUTPUT_DIR / "device-contract.js").write_text(source, encoding="utf-8")


def build_metadata(collections, render_paths, default_screens):
    specs = device_specs(collections)
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
        "version": 2,
        "units": "meters",
        "coordinate_system": {"up": "+Y", "forward": "+Z", "right": "+X"},
        "origin": "device center; laptop origin is rear-center hinge projection on base underside",
        "devices": devices,
        "renders": {path.stem: image_info(path) for path in render_paths},
        "source": {"blend": BLEND_PATH.name, "generator": Path(__file__).name, "blender": bpy.app.version_string},
        "screen_fixtures": {
            name: {
                "file": str(path.relative_to(LANDING_ASSETS)),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None,
                "packed_in_blend": path.exists(),
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
    write_device_contract(specs)


def main():
    args = parse_args()
    SOURCE_DIR.mkdir(parents=True, exist_ok=True)
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    if args.render_only:
        render_only(args.render_only)
        return
    default_screens = {
        "laptop": args.laptop_screen.resolve(),
        "tablet": args.tablet_screen.resolve(),
        "phone": args.phone_screen.resolve(),
    }
    reset_scene()
    mats = {
        "graphite": material("SpaceBlackAluminum", SILVER, metallic=1.0, roughness=0.45, coat=0.0, coat_roughness=0.165, anisotropic=0.34),
        "lid": material("SatinLidAluminum", SATIN_DARK, metallic=1.0, roughness=0.60, coat=0.0, coat_roughness=0.165, anisotropic=0.34),
        "front_band": material("SatinFrontBand", SATIN_DARK, metallic=1.0, roughness=0.60, coat=0.0, coat_roughness=0.165, anisotropic=0.34),
        "edge": material("MachinedSpaceBlackEdge", SILVER_EDGE, metallic=1.0, roughness=0.20, coat=0.0, coat_roughness=0.121, anisotropic=0.42),
        "black": material("BlackInset", (0.001, 0.001, 0.0015, 1.0), metallic=0.0, roughness=0.92, specular=0.0),
        "glass": material(
            "FrontGlass", (0.0045, 0.0050, 0.0060, 1.0), metallic=0.0,
            roughness=0.30, coat=0.10, coat_roughness=0.30, specular=0.18,
        ),
        "key": material("KeyGraphite", KEY_COLOR, metallic=0.0, roughness=0.38, coat=0.0, specular=0.18),
        "legend": material("KeyLegend", (0.42, 0.45, 0.49, 1.0), metallic=0.0, roughness=0.42),
        "trackpad": material("TrackpadSpaceBlack", (0.50, 0.51, 0.53, 1.0), metallic=1.0, roughness=0.32, coat=0.0, coat_roughness=0.11),
        "port": material("PortInterior", (0.002, 0.003, 0.004, 1.0), metallic=0.12, roughness=0.37),
        "connector": material("ConnectorMetal", (0.28, 0.22, 0.10, 1.0), metallic=0.84, roughness=0.20),
        "lens": material("OpticalGlass", (0.002, 0.007, 0.014, 1.0), metallic=0.02, roughness=0.055, coat=1.0),
        "sensor": material("SensorBlack", (0.0007, 0.0010, 0.0014, 1.0), metallic=0.0, roughness=0.55, specular=0.05),
        "camera_ring": material("CameraRing", (0.70, 0.71, 0.73, 1.0), metallic=1.0, roughness=0.16, coat=0.16),
        "flash": material("FlashGlass", (0.78, 0.74, 0.58, 1.0), metallic=0.0, roughness=0.17, coat=0.75),
        "phone_aluminum": material("DeepBlueAluminum", SILVER, metallic=1.0, roughness=0.45, coat=0.0, coat_roughness=0.165, anisotropic=0.38),
        "phone_edge": material("DeepBlueMachinedEdge", SILVER_EDGE, metallic=1.0, roughness=0.20, coat=0.0, coat_roughness=0.121, anisotropic=0.42),
        "phone_back": material("DeepBlueCeramicShield", (0.70, 0.71, 0.72, 1.0), metallic=0.15, roughness=0.38, coat=0.62, coat_roughness=0.1375),
        "desktop_screen": screen_material("ScreenDesktop", default_screens["laptop"]),
        "tablet_screen": screen_material("ScreenTablet", default_screens["tablet"]),
        "phone_screen": screen_material("ScreenPhone", default_screens["phone"]),
    }
    material("RenderFloorBlack", (0.0015, 0.002, 0.004, 1), metallic=0.05, roughness=0.34)
    render_floor_clear = material("RenderFloorClear", (0.02, 0.02, 0.02, 0), metallic=0, roughness=0.5)
    render_floor_clear.diffuse_color = (0.02, 0.02, 0.02, 0)

    laptop = build_laptop(mats)
    tablet = build_tablet(mats)
    phone = build_phone(mats)
    collections = {"laptop": laptop, "tablet": tablet, "phone": phone}

    if args.preview_dir:
        render_previews(args.preview_dir.resolve(), collections)
        print(f"Built device previews in {args.preview_dir.resolve()}")
        return

    for name, collection in collections.items():
        export_glb(collection, OUTPUT_DIR / f"{name}.glb")
    export_glb(laptop, OUTPUT_DIR / "laptop-low.glb", include_decorative=False)

    # Save editable source before temporary composition transforms are applied.
    bpy.ops.wm.save_as_mainfile(filepath=str(BLEND_PATH))

    if args.models_only:
        existing_renders = sorted(OUTPUT_DIR.glob("*.webp"))
        build_metadata(collections, existing_renders, default_screens)
        bpy.ops.wm.save_as_mainfile(filepath=str(BLEND_PATH))
        BLEND_PATH.with_suffix(".blend1").unlink(missing_ok=True)
        print(f"Built device models in {OUTPUT_DIR}")
        return

    render_paths = []
    set_collection_visibility(laptop)
    render(OUTPUT_DIR / "hero-laptop.webp", 1600, 1180, LAPTOP_RENDER_CAMERA, LAPTOP_RENDER_TARGET, LAPTOP_RENDER_LENS, True)
    render_paths.append(OUTPUT_DIR / "hero-laptop.webp")
    render(OUTPUT_DIR / "laptop.webp", 1200, 900, LAPTOP_RENDER_CAMERA, LAPTOP_RENDER_TARGET, LAPTOP_RENDER_LENS, True)
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
