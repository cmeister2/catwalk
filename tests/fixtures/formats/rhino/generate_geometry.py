"""Author small native fixtures with McNeel rhino3dm 8.17.0 public APIs.

This optional development script creates new native archives, UUIDs and stamps.
The committed archives are immutable test inputs. Running it intentionally
reauthors those fixtures and therefore requires rerunning qualify_geometry.py
and reviewing all source hashes and measurements.
"""
from pathlib import Path
import rhino3dm as rh

ROOT = Path(__file__).resolve().parent
assert rh.__version__ == "8.17.0"

for version in [5, 6, 7, 8]:
    model = rh.File3dm()
    model.Settings.ModelUnitSystem = rh.UnitSystem.Centimeters
    attributes = rh.ObjectAttributes()
    attributes.Name = "authored visible geometry"
    attributes.ObjectColor = (220, 70, 30, 255)
    attributes.ColorSource = rh.ObjectColorSource.ColorFromObject
    bounds = rh.BoundingBox(rh.Point3d(0, 0, 0), rh.Point3d(2, 3, 5))
    model.Objects.AddBrep(rh.Brep.CreateFromBox(rh.Box(bounds)), attributes)
    model.Objects.AddLine(rh.Point3d(4, 1, 1), rh.Point3d(6, 1, 1), attributes)
    model.Objects.AddCircle(rh.Circle(rh.Point3d(5, 3, 1), 1), attributes)
    poles = [rh.Point3d(4, 0, 0), rh.Point3d(4, 1, 3), rh.Point3d(6, 2, 3), rh.Point3d(6, 3, 0)]
    model.Objects.AddCurve(rh.NurbsCurve.Create(False, 3, poles), attributes)
    model.Objects.AddPoint(rh.Point3d(-1, -2, -3), attributes)
    hidden = rh.ObjectAttributes()
    hidden.Visible = False
    hidden.ObjectColor = (0, 255, 0, 255)
    hidden.ColorSource = rh.ObjectColorSource.ColorFromObject
    model.Objects.AddPoint(rh.Point3d(1000, 1000, 1000), hidden)
    mesh = rh.Mesh()
    for vertex in [(8, 0, 0), (10, 0, 0), (10, 2, 0), (8, 2, 0)]:
        mesh.Vertices.Add(*vertex)
    mesh.Faces.AddFace(0, 1, 2, 3)
    mesh.Normals.ComputeNormals()
    model.Objects.AddMesh(mesh, attributes)
    assert model.Write(str(ROOT / f"producer_geometry_v{version}.3dm"), version)

model = rh.File3dm()
layer = rh.Layer()
layer.Name = "Block"
model.Layers.Add(layer)
model.Settings.ModelUnitSystem = rh.UnitSystem.Millimeters
mesh = rh.Mesh()
for vertex in [(0, 0, 0), (2, 0, 0), (0, 3, 0)]:
    mesh.Vertices.Add(*vertex)
mesh.Faces.AddFace(0, 1, 2)
mesh.Normals.ComputeNormals()
attributes = rh.ObjectAttributes()
attributes.LayerIndex = 0
attributes.ColorSource = rh.ObjectColorSource.ColorFromObject
attributes.ObjectColor = (20, 100, 200, 255)
index = model.InstanceDefinitions.Add("triangle", "authored block", "", "", rh.Point3d(0, 0, 0), (mesh,), (attributes,))
definition = model.InstanceDefinitions.FindIndex(index).Id
for translation, color in [(rh.Vector3d(10, 20, 30), (10, 150, 240, 255)), (rh.Vector3d(-10, -20, -30), (240, 20, 80, 255))]:
    attributes = rh.ObjectAttributes()
    attributes.LayerIndex = 0
    attributes.ColorSource = rh.ObjectColorSource.ColorFromObject
    attributes.ObjectColor = color
    model.Objects.AddInstanceObject(rh.InstanceReference(definition, rh.Transform.Translation(translation)), attributes)
assert model.Write(str(ROOT / "producer_instances.3dm"), 8)

model = rh.File3dm()
layer = rh.Layer()
layer.Name = "points"
index = model.Layers.Add(layer)
attributes = rh.ObjectAttributes()
attributes.LayerIndex = index
model.Objects.AddPoint(rh.Point3d(1.25, -2.5, 7.75), attributes)
assert model.Write(str(ROOT / "producer_point.3dm"), 8)

model = rh.File3dm()
layer = rh.Layer()
layer.Name = "native NURBS"
model.Layers.Add(layer)
attributes = rh.ObjectAttributes()
attributes.LayerIndex = 0
attributes.ColorSource = rh.ObjectColorSource.ColorFromObject
attributes.ObjectColor = (30, 160, 90, 255)
first = rh.NurbsCurve.Create(False, 2, [rh.Point3d(0, 0, 0), rh.Point3d(1, 0, 1), rh.Point3d(2, 0, 0)])
second = rh.NurbsCurve.Create(False, 2, [rh.Point3d(0, 3, 0), rh.Point3d(1, 3, 2), rh.Point3d(2, 3, 0)])
model.Objects.AddSurface(rh.NurbsSurface.CreateRuledSurface(first, second), attributes)
assert model.Write(str(ROOT / "producer_surface.3dm"), 8)
