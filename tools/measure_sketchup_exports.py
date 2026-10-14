"""Measure unchanged SketchUp COLLADA exports without using Catwalk or SKP code.

Run from the repository root. The exporter writes both sides of every face;
the witness records half the exported triangle area, in square millimetres.
"""

import json
import math
from pathlib import Path
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[1] / "tests/fixtures/formats/sketchup"
NS = {"c": "http://www.collada.org/2005/11/COLLADASchema"}
IDENTITY = [float(i // 4 == i % 4) for i in range(16)]


def multiply(left, right):
    """Compose row-major matrices under the column-vector convention."""
    return [sum(left[row * 4 + k] * right[k * 4 + col] for k in range(4))
            for row in range(4) for col in range(4)]


def position(matrix, point):
    """Apply an affine node placement to a source position."""
    return [sum(matrix[row * 4 + k] * (point + [1.0])[k] for k in range(4))
            for row in range(3)]


def measure(path):
    """Traverse the producer's selected visual scene and compute world geometry."""
    doc = ET.parse(path).getroot()
    by_id = {node.get("id"): node for node in doc.iter() if node.get("id")}
    unit = float(doc.find("c:asset/c:unit", NS).get("meter")) * 1000.0
    polygons = []
    all_points = []

    def geometry(node, matrix):
        mesh = node.find("c:mesh", NS)
        sources = {}
        for source in mesh.findall("c:source", NS):
            values = source.find("c:float_array", NS)
            if values is not None:
                accessor = source.find("c:technique_common/c:accessor", NS)
                sources[source.get("id")] = (list(map(float, values.text.split())),
                                              int(accessor.get("stride", "1")))
        primitives = [node for node in mesh if node.tag.rsplit("}", 1)[-1] in {"triangles", "polylist", "polygons", "lines"}]
        for primitive in primitives:
            inputs = primitive.findall("c:input", NS)
            width = max(int(value.get("offset", "0")) for value in inputs) + 1
            vertex = next(value for value in inputs if value.get("semantic") == "VERTEX")
            vertices = by_id[vertex.get("source")[1:]]
            point_input = next(value for value in vertices.findall("c:input", NS)
                               if value.get("semantic") == "POSITION")
            points, stride = sources[point_input.get("source")[1:]]
            kind = primitive.tag.rsplit("}", 1)[-1]
            corner_offset = int(vertex.get("offset", "0"))

            def corners_for(text):
                indices = list(map(int, text.split()))
                assert len(indices) % width == 0
                corners = []
                for offset in range(corner_offset, len(indices), width):
                    index = indices[offset] * stride
                    placed = position(matrix, points[index:index + 3])
                    corners.append([coordinate * unit for coordinate in placed])
                all_points.extend(corners)
                return corners

            if kind == "polygons":
                groups = [node for node in primitive if node.tag.rsplit("}", 1)[-1] in {"p", "ph"}]
                assert len(groups) == int(primitive.get("count"))
                for group in groups:
                    rings = [group] if group.tag.endswith("}p") else [group.find("c:p", NS)] + group.findall("c:h", NS)
                    polygons.append([corners_for(ring.text) for ring in rings])
                continue
            indices = list(map(int, primitive.find("c:p", NS).text.split()))
            counts = list(map(int, primitive.find("c:vcount", NS).text.split())) if kind == "polylist" else [3 if kind == "triangles" else 2] * int(primitive.get("count"))
            assert len(counts) == int(primitive.get("count"))
            assert len(indices) == sum(counts) * width
            corners = corners_for(primitive.find("c:p", NS).text)
            if kind != "lines":
                offset = 0
                for count in counts:
                    polygons.append([corners[offset:offset + count]])
                    offset += count

    def visit(node, parent):
        matrix = parent
        transform = node.find("c:matrix", NS)
        if transform is not None:
            matrix = multiply(parent, list(map(float, transform.text.split())))
        for child in node:
            kind = child.tag.rsplit("}", 1)[-1]
            if kind == "node":
                visit(child, matrix)
            elif kind == "instance_node":
                visit(by_id[child.get("url")[1:]], matrix)
            elif kind == "instance_geometry":
                geometry(by_id[child.get("url")[1:]], matrix)

    selected = doc.find("c:scene/c:instance_visual_scene", NS)
    scene = by_id[selected.get("url")[1:]]
    visit(scene, IDENTITY)
    points = all_points
    bounds = [[function(point[axis] for point in points) for axis in range(3)]
              for function in (min, max)]
    area = 0.0
    for rings in polygons:
        for index, polygon in enumerate(rings):
            cross = [0.0, 0.0, 0.0]
            for u, v in zip(polygon, polygon[1:] + polygon[:1]):
                cross[0] += u[1] * v[2] - u[2] * v[1]
                cross[1] += u[2] * v[0] - u[0] * v[2]
                cross[2] += u[0] * v[1] - u[1] * v[0]
            area += math.hypot(*cross) / 2.0 * (1 if index == 0 else -1)
    return {"bounds_mm": bounds, "one_sided_area_mm2": area / 2.0,
            "export_surface_polygons": len(polygons)}


if __name__ == "__main__":
    witnesses = {path.name.removesuffix("-export.dae"): measure(path)
                 for path in sorted(ROOT.glob("*-export.dae"))}
    (ROOT / "geometry-witnesses.json").write_text(json.dumps(witnesses, indent=2) + "\n")
