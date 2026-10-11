//! VRML97 line sets, terrain and spine-aligned extrusions.
use super::*;

fn colors(node: &Node) -> Result<Option<Vec<[u8; 3]>>> {
    node.single("color")?
        .map(|n| {
            if n.kind != "Color" {
                return Err(error("VRML color must be a Color node"));
            }
            n.check(&["color"])?;
            let values = vectors(&n, "color")?;
            if values
                .iter()
                .any(|v| [v.x, v.y, v.z].iter().any(|c| !(0. ..=1.).contains(c)))
            {
                return Err(error("VRML color components must be between zero and one"));
            }
            Ok(values.into_iter().map(color).collect())
        })
        .transpose()
}
fn average(values: &[[u8; 3]]) -> [u8; 3] {
    std::array::from_fn(|i| {
        (values.iter().map(|c| c[i] as u64).sum::<u64>() / values.len() as u64) as u8
    })
}
fn count(node: &Node, name: &str) -> Result<usize> {
    let n = node.scalar(name, 0.)?;
    if n < 0. || n.fract() != 0. || n > MAX_ITEMS as f64 {
        return Err(error("invalid VRML dimension"));
    }
    Ok(n as usize)
}

impl Importer {
    pub(super) fn indexed_lines(
        &mut self,
        node: &Node,
        world: Transform,
        material: Option<[u8; 3]>,
    ) -> Result<()> {
        node.check(&[
            "coord",
            "coordIndex",
            "color",
            "colorIndex",
            "colorPerVertex",
        ])?;
        let Some(coordinates) = node.single("coord")? else {
            return Ok(());
        };
        if coordinates.kind != "Coordinate" {
            return Err(error("VRML coord must be a Coordinate node"));
        }
        coordinates.check(&["point"])?;
        let points = vectors(&coordinates, "point")?;
        let lines = index_lists(&node.numbers("coordIndex")?)?;
        let colors = colors(node)?;
        let per_vertex = node.boolean("colorPerVertex", true)?;
        let raw_colors = node.numbers("colorIndex")?;
        let color_lines = if per_vertex {
            index_lists(&raw_colors)?
        } else {
            Vec::new()
        };
        if per_vertex && !color_lines.is_empty() && color_lines.len() != lines.len() {
            return Err(error(
                "VRML line color indices disagree with coordinate lists",
            ));
        }
        if !per_vertex && !raw_colors.is_empty() && raw_colors.len() < lines.len() {
            return Err(error(
                "VRML IndexedLineSet has fewer line color indices than polylines",
            ));
        }
        for (i, indices) in lines.iter().enumerate() {
            if indices.len() < 2 {
                return Err(error("VRML polyline needs at least two vertices"));
            }
            let positions: Vec<_> = indices
                .iter()
                .map(|&n| {
                    points
                        .get(n)
                        .copied()
                        .ok_or_else(|| error("VRML line coordinate index out of bounds"))
                })
                .collect::<Result<_>>()?;
            let indexed_colors = if let Some(colors) = &colors {
                let ids = if per_vertex {
                    color_lines.get(i).unwrap_or(indices).clone()
                } else {
                    let n = raw_colors.get(i).copied().unwrap_or(i as f64);
                    if n < 0. || n.fract() != 0. || n > MAX_ITEMS as f64 {
                        return Err(error("invalid VRML line color index"));
                    }
                    vec![n as usize; indices.len()]
                };
                if ids.len() != indices.len() {
                    return Err(error("VRML line color and coordinate lengths differ"));
                }
                Some(
                    ids.into_iter()
                        .map(|n| {
                            colors
                                .get(n)
                                .copied()
                                .ok_or_else(|| error("VRML line color index out of bounds"))
                        })
                        .collect::<Result<Vec<_>>>()?,
                )
            } else {
                None
            };
            for (j, pair) in positions.windows(2).enumerate() {
                let vertices = [pair[0], pair[1]].map(|p| {
                    let p = world.apply(p);
                    Vec3::new(p.x, -p.z, p.y)
                });
                if vertices.iter().any(|p| !p.is_finite()) {
                    return Err(error("VRML transformed coordinate is non-finite"));
                }
                if vertices[0] == vertices[1] {
                    continue;
                }
                let color = indexed_colors
                    .as_ref()
                    .map(|c| average(&c[j..j + 2]))
                    .or(material);
                self.scene.lines.push(Line { vertices, color });
                self.limit()?;
            }
        }
        Ok(())
    }
    pub(super) fn elevation_grid(
        &mut self,
        node: &Node,
        world: Transform,
        material: Option<[u8; 3]>,
    ) -> Result<()> {
        node.check(&[
            "height",
            "xDimension",
            "zDimension",
            "xSpacing",
            "zSpacing",
            "ccw",
            "solid",
            "creaseAngle",
            "color",
            "colorPerVertex",
            "normal",
            "normalPerVertex",
            "texCoord",
        ])?;
        let x = count(node, "xDimension")?;
        let z = count(node, "zDimension")?;
        let total = x
            .checked_mul(z)
            .filter(|&n| n <= MAX_ITEMS)
            .ok_or_else(|| error("VRML ElevationGrid exceeds geometry limit"))?;
        let heights = node.numbers("height")?;
        if heights.len() < total {
            return Err(error(
                "VRML ElevationGrid has fewer heights than grid points",
            ));
        }
        let dx = node.scalar("xSpacing", 1.)?;
        let dz = node.scalar("zSpacing", 1.)?;
        if dx <= 0. || dz <= 0. {
            return Err(error("VRML grid spacing must be positive"));
        }
        if x < 2 || z < 2 {
            return Ok(());
        }
        let colors = colors(node)?;
        let per_vertex = node.boolean("colorPerVertex", true)?;
        let cells = (x - 1) * (z - 1);
        if colors
            .as_ref()
            .is_some_and(|c| c.len() < if per_vertex { total } else { cells })
        {
            return Err(error(
                "VRML ElevationGrid has fewer colors than vertices or cells",
            ));
        }
        let ccw = node.boolean("ccw", true)?;
        for j in 0..z - 1 {
            for i in 0..x - 1 {
                // Positive Y normals, matching the specification's default ccw field.
                let indices = [
                    i + j * x,
                    i + (j + 1) * x,
                    i + 1 + (j + 1) * x,
                    i + 1 + j * x,
                ];
                let points = indices
                    .map(|n| Vec3::new((n % x) as f64 * dx, heights[n], (n / x) as f64 * dz));
                let color = colors
                    .as_ref()
                    .map(|c| {
                        if per_vertex {
                            average(&indices.map(|n| c[n]))
                        } else {
                            c[i + j * (x - 1)]
                        }
                    })
                    .or(material);
                // A terrain cell can be nonplanar; retain both actual grid triangles
                // rather than ear-clipping a projected polygon.
                self.face(&[points[0], points[1], points[2]], world, color, ccw)?;
                self.face(&[points[0], points[2], points[3]], world, color, ccw)?;
            }
        }
        Ok(())
    }
    pub(super) fn extrusion(
        &mut self,
        node: &Node,
        world: Transform,
        material: Option<[u8; 3]>,
    ) -> Result<()> {
        node.check(&[
            "crossSection",
            "spine",
            "orientation",
            "scale",
            "beginCap",
            "endCap",
            "ccw",
            "convex",
            "solid",
            "creaseAngle",
        ])?;
        let cross = if node.fields.contains_key("crossSection") {
            node.numbers("crossSection")?
        } else {
            vec![1., 1., 1., -1., -1., -1., -1., 1., 1., 1.]
        };
        if cross.len() < 4 || cross.len() % 2 != 0 {
            return Err(error(
                "VRML extrusion crossSection needs complete coordinate pairs",
            ));
        }
        let cross: Vec<_> = cross
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| Vec3::new(v[0], 0., v[1]))
            .collect();
        let spine = if node.fields.contains_key("spine") {
            vectors(node, "spine")?
        } else {
            vec![Vec3::default(), Vec3::new(0., 1., 0.)]
        };
        if spine.len() < 2 {
            return Err(error("VRML extrusion spine needs at least two points"));
        }
        if cross
            .len()
            .checked_mul(spine.len())
            .is_none_or(|n| n > MAX_ITEMS / 2)
        {
            return Err(error("VRML extrusion exceeds geometry limit"));
        }
        let scales = if node.fields.contains_key("scale") {
            node.numbers("scale")?
        } else {
            vec![1., 1.]
        };
        if scales.is_empty() || scales.len() % 2 != 0 || scales.iter().any(|&n| n <= 0.) {
            return Err(error(
                "VRML extrusion scales must be positive coordinate pairs",
            ));
        }
        let scale_count = scales.len() / 2;
        if scale_count != 1 && scale_count < spine.len() {
            return Err(error("VRML extrusion has too few scale values"));
        }
        let orientations = if node.fields.contains_key("orientation") {
            node.numbers("orientation")?
        } else {
            vec![0., 0., 1., 0.]
        };
        if orientations.is_empty() || orientations.len() % 4 != 0 {
            return Err(error("VRML extrusion orientation needs complete rotations"));
        }
        let rotation_count = orientations.len() / 4;
        if rotation_count != 1 && rotation_count < spine.len() {
            return Err(error("VRML extrusion has too few orientation values"));
        }
        let rotations: Vec<_> = orientations
            .as_chunks::<4>()
            .0
            .iter()
            .take(if rotation_count == 1 { 1 } else { spine.len() })
            .map(|r| axis_rotation(Vec3::new(r[0], r[1], r[2]), r[3]))
            .collect::<Result<_>>()?;
        let frames = spine_frames(&spine)?;
        let mut rings = Vec::new();
        for (i, &point) in spine.iter().enumerate() {
            let scale = if scale_count == 1 {
                &scales[..2]
            } else {
                &scales[2 * i..2 * i + 2]
            };
            let rotation = rotations[if rotation_count == 1 { 0 } else { i }];
            let points: Vec<_> = cross
                .iter()
                .map(|v| {
                    point
                        + frames[i].apply(rotation.apply(Vec3::new(
                            v.x * scale[0],
                            0.,
                            v.z * scale[1],
                        )))
                })
                .collect();
            if points.iter().any(|p| !p.is_finite()) {
                return Err(error("VRML extrusion coordinate overflow"));
            }
            rings.push(points);
        }
        let ccw = node.boolean("ccw", true)?;
        for pair in rings.windows(2) {
            for i in 0..cross.len() - 1 {
                // Twisted sections can make a nonplanar quad whose planar projection
                // crosses itself. Tessellate its actual two surface triangles directly.
                self.face(
                    &[pair[0][i], pair[0][i + 1], pair[1][i + 1]],
                    world,
                    material,
                    ccw,
                )?;
                self.face(
                    &[pair[0][i], pair[1][i + 1], pair[1][i]],
                    world,
                    material,
                    ccw,
                )?;
            }
        }
        let closed = cross.first() == cross.last();
        let size = cross.len() - usize::from(closed);
        for (enabled, i, reverse) in [
            (node.boolean("beginCap", true)?, 0, true),
            (node.boolean("endCap", true)?, rings.len() - 1, false),
        ] {
            if enabled {
                if size < 3 {
                    return Err(error(
                        "VRML extrusion cap needs at least three distinct cross-section points",
                    ));
                }
                let mut cap = rings[i][..size].to_vec();
                if reverse {
                    cap.reverse();
                }
                self.face(&cap, world, material, ccw)?;
            }
        }
        Ok(())
    }
}

fn spine_frames(spine: &[Vec3]) -> Result<Vec<Transform>> {
    // Coincident spine points share the SCP while their authored orientations remain separate.
    let mut distinct = Vec::new();
    let mut mapping = Vec::new();
    for &p in spine {
        if distinct.last() != Some(&p) {
            distinct.push(p);
        }
        mapping.push(distinct.len() - 1);
    }
    if distinct.len() == 1 {
        return Ok(vec![Transform::IDENTITY; spine.len()]);
    }
    let closed = distinct.len() > 2 && distinct.first() == distinct.last();
    let n = distinct.len() - usize::from(closed);
    let mut y = Vec::new();
    let mut z = Vec::new();
    for i in 0..n {
        let previous = if i > 0 {
            i - 1
        } else if closed {
            n - 1
        } else {
            0
        };
        let next = if i + 1 < n {
            i + 1
        } else if closed {
            0
        } else {
            n - 1
        };
        let tangent = (distinct[next] - distinct[previous]).normalized();
        if tangent.length() == 0. {
            return Err(error("VRML spine has an undefined tangent at a reversal"));
        }
        y.push(tangent);
        z.push(
            (distinct[next] - distinct[i])
                .cross(distinct[previous] - distinct[i])
                .normalized(),
        );
    }
    if !closed && n > 2 {
        z[0] = z[1];
        z[n - 1] = z[n - 2];
    }
    let mut frames = Vec::new();
    if let Some(first) = z.iter().copied().find(|v| v.length() != 0.) {
        let mut previous = first;
        for i in 0..n {
            let mut normal = if z[i].length() == 0. { previous } else { z[i] };
            if previous.dot(normal) < 0. {
                normal = -normal;
            }
            let x = y[i].cross(normal).normalized();
            if x.length() == 0. {
                return Err(error("VRML spine has an undefined cross-section frame"));
            }
            // Reorthogonalize after collinear runs to keep the SCP a pure rotation.
            normal = x.cross(y[i]).normalized();
            frames.push(Transform([
                [x.x, x.y, x.z, 0.],
                [y[i].x, y[i].y, y[i].z, 0.],
                [normal.x, normal.y, normal.z, 0.],
                [0., 0., 0., 1.],
            ]));
            previous = normal;
        }
    } else {
        for tangent in y {
            let axis = Vec3::new(tangent.z, 0., -tangent.x);
            let rotation = if axis.length() == 0. {
                if tangent.y >= 0. {
                    Transform::IDENTITY
                } else {
                    axis_rotation(Vec3::new(1., 0., 0.), std::f64::consts::PI)?
                }
            } else {
                axis_rotation(axis, tangent.y.clamp(-1., 1.).acos())?
            };
            frames.push(rotation);
        }
    }
    if closed {
        frames.push(frames[0]);
    }
    Ok(mapping.into_iter().map(|i| frames[i]).collect())
}
