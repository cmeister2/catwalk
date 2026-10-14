"""Independent Rhino geometry witnesses using McNeel rhino3dm 8.17.0.

Read retained archives only; never rewrite or tessellate with Catwalk.
Run with: python -m pip install rhino3dm==8.17.0
          python qualify_geometry.py
"""
import hashlib
import json
from pathlib import Path
import rhino3dm as rh

ROOT = Path(__file__).resolve().parent

def point(value):
    return [value.X, value.Y, value.Z]

PIN = "eb92af3ba1806b0a34a99aba0d3bda83e3d46083"
witness = {"upstream_repository": "https://github.com/mcneel/opennurbs", "upstream_commit": PIN, "producer": "McNeel rhino3dm", "version": rh.__version__, "files": []}
for name in ["my_curves.3dm", "my_points.3dm", "my_trimmed_surface.3dm", "my_mesh.3dm", "v4_MechPartB.3dm", "v5_extrusions.3dm", "producer_surface.3dm", "producer_point.3dm", "producer_instances.3dm"] + [f"producer_geometry_v{version}.3dm" for version in [5, 6, 7, 8]]:
    path = ROOT / name
    model = rh.File3dm.Read(str(path))
    entry = {"file": name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "unit_system": str(model.Settings.ModelUnitSystem), "objects": []}
    if name.startswith("producer_"):
        entry["authored"] = "Catwalk test models authored through McNeel rhino3dm 8.17.0 public geometry and document APIs"
    else:
        version = "V4" if name == "v4_MechPartB.3dm" else "V5" if name == "v5_extrusions.3dm" else "V6"
        entry["upstream_path"] = f"example_files/{version}/{name}"
    for obj in model.Objects:
        geometry = obj.Geometry
        item = {"id": str(obj.Attributes.Id), "kind": type(geometry).__name__, "visible": obj.Attributes.Visible, "color": list(obj.Attributes.ObjectColor)}
        if isinstance(geometry, rh.Point):
            item["point"] = point(geometry.Location)
        elif isinstance(geometry, rh.PointCloud):
            item["points"] = [point(p) for p in geometry.GetPoints()]
        elif isinstance(geometry, rh.Mesh):
            item["vertices"] = [point(vertex) for vertex in geometry.Vertices]
            item["native_faces"] = [list(face) for face in geometry.Faces]
            item["normals"] = [point(normal) for normal in geometry.Normals]
            geometry.Faces.ConvertQuadsToTriangles()
            item["triangle_indices"] = [list(face[:3]) for face in geometry.Faces]
            item["triangles"] = [[item["vertices"][index] for index in face[:3]] for face in geometry.Faces]
        elif isinstance(geometry, rh.Curve):
            domain = geometry.Domain
            item["samples"] = [point(geometry.PointAt(domain.T0 + (domain.T1-domain.T0)*index/16)) for index in range(17)]
        elif isinstance(geometry, rh.Extrusion):
            item["profile_count"] = geometry.ProfileCount
            item["profile_samples"] = []
            for index in range(geometry.ProfileCount):
                for height in [0.0, 1.0]:
                    curve = geometry.Profile3d(index, height)
                    domain = curve.Domain
                    item["profile_samples"].extend([point(curve.PointAt(domain.T0+(domain.T1-domain.T0)*sample/16)) for sample in range(17)])
        elif isinstance(geometry, rh.NurbsSurface):
            u, v = geometry.Domain(0), geometry.Domain(1)
            item["samples"] = [point(geometry.PointAt(u.T0+(u.T1-u.T0)*index/8, end)) for index in range(9) for end in [v.T0, v.T1]]
        elif isinstance(geometry, rh.Brep):
            item["faces"] = len(geometry.Faces)
            item["vertices"] = [point(vertex.Location) for vertex in geometry.Vertices]
            item["cache_triangles"] = sum(2 if face[2] != face[3] else 1 for source_face in geometry.Faces for face in (source_face.GetMesh(rh.MeshType.Render).Faces if source_face.GetMesh(rh.MeshType.Render) else []))
        elif isinstance(geometry, rh.InstanceReference):
            item["definition"] = str(geometry.ParentIdefId)
            item["transform"] = [[getattr(geometry.Xform, f"M{row}{col}") for col in range(4)] for row in range(4)]
        entry["objects"].append(item)
    witness["files"].append(entry)
(ROOT / "geometry-witnesses.json").write_text(json.dumps(witness, indent=2) + "\n")
