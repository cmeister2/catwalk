# Local patch to ogeom-io 0.9.16

The exact crates.io source is preserved under its MIT OR Apache-2.0 license,
with both original license files included.

In `src/brep.rs`, `Cursor::seat` now retains the authored Y direction and
uses `Frame::from_axes(origin, x, y, normal, tol)` instead of reconstructing
a right-handed frame from the normal and X direction. OCCT BREP stores all
three directions; valid left-handed analytic surface charts must preserve
their parameterization. Discarding Y changes the points described by their
pcurves and can make correctly placed curve/surface pairs disagree by the
model's entire diameter.

Primary format specification:
https://occt3d.com/dev/doc/overview/html/specification__brep_format.html

Portable regression in Catwalk's `src/formats/cad.rs` constructs a genuine
left-handed analytic cylinder BREP, checks native chart evaluation and
compares the resulting surface area and placed bounds. This patch changes
no format aliases, native dependencies, mesh tolerance or omission policy.

The separate `read_iges_unsewn` preview entry point preserves every genuine
independent trimmed face in a compound, without sewing, repairing geometry,
or dropping faces. The existing `read_iges` entry point retains its original
sewing behavior. Preview expansion is limited to 100,000 entities and 4096
independent trimmed faces, with the original recursive/count bounds retained.

Successfully consumed type-142 boundaries may contain both an authored 3D
trim and an advisory parameter-space curve. The preview variant validates
those actual alternate curve definitions through the existing bounded curve
reader and accounts for them in its visited entities. It does not render
redundant parameter-space definitions or consume independent/unowned curves.
All original geometry warning/error reporting is retained. Catwalk rejects
compromised geometry reports before rendering.

Portable regressions verify source/imported/rendered face cardinality, actual
surface area/bounds, accounted owned alternatives, and explicit failures for
unowned or malformed alternate curves. This preserves all 213 authored faces
of the independent original OCCT `bearing.iges` fixture before tessellation;
its imported faces build without geometry warnings, whereas ordinary sewing
returned zero shells.
