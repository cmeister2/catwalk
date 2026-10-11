//! Authored attribute mappings, malformed normals, storage bounds and rendered color controls.

use super::*;
use crate::render::{RenderOptions, render_xy};
use draco_core::AttributeValueIndex;

/// Construct finite or deliberately malformed double-precision tuples through the public builder.
///
/// # Errors
///
/// The upstream attribute builder cannot allocate its small test buffer.
fn attribute(
    kind: GeometryAttributeType,
    components: u8,
    values: &[f64],
) -> Result<PointAttribute> {
    let mut attribute = PointAttribute::new();
    attribute.try_init(
        kind,
        components,
        DataType::Float64,
        false,
        values.len() / usize::from(components),
    )?;
    let bytes: Vec<_> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    attribute.buffer_mut().update(&bytes, Some(0));
    Ok(attribute)
}

/// Build a unit right triangle and attach the supplied independent normal/color attributes.
///
/// # Errors
///
/// Its position attribute cannot be allocated.
fn geometry(normals: Option<PointAttribute>, color: bool) -> Result<DecodedGeometry> {
    let mut attributes = PointCloud::new();
    attributes.set_num_points(3);
    attributes.add_attribute(attribute(
        GeometryAttributeType::Position,
        3,
        &[0., 0., 0., 1., 0., 0., 0., 1., 0.],
    )?);
    if let Some(normals) = normals {
        attributes.add_attribute(normals);
    }
    if color {
        attributes.add_attribute(attribute(
            GeometryAttributeType::Color,
            3,
            &[1., 0., 0., 0., 1., 0., 0., 0., 1.],
        )?);
    }
    Ok(DecodedGeometry {
        point_count: 3,
        faces: Some(vec![[0, 1, 2]]),
        attributes,
    })
}

/// Compare independently calculated normal directions within double-precision tolerance.
fn close(actual: Vec3, expected: Vec3) -> bool {
    (actual - expected).length() < 1e-12
}

#[test]
fn mapped_normal_values_follow_point_to_value_indices() -> Result<()> {
    let mut normals = attribute(
        GeometryAttributeType::Normal,
        3,
        &[0., 0., 8., 0., 6., 0., 4., 0., 0.],
    )?;
    normals.set_explicit_mapping_from(&[
        AttributeValueIndex(2),
        AttributeValueIndex(0),
        AttributeValueIndex(1),
    ]);
    let scene = decoded_scene(geometry(Some(normals), false)?)?;
    let actual = scene.appearances[&0]
        .normals
        .ok_or("missing mapped normals")?;
    for (actual, expected) in actual.into_iter().zip([
        Vec3::new(1., 0., 0.),
        Vec3::new(0., 0., 1.),
        Vec3::new(0., 1., 0.),
    ]) {
        assert!(close(actual, expected));
    }
    assert_eq!(scene.triangles[0].vertices[1], Vec3::new(1., 0., 0.));
    Ok(())
}

#[test]
fn extreme_finite_directions_normalize_without_overflow_or_underflow() -> Result<()> {
    let normals = attribute(
        GeometryAttributeType::Normal,
        3,
        &[
            f64::MAX,
            f64::MAX,
            0.,
            f64::MIN_POSITIVE,
            0.,
            f64::MIN_POSITIVE,
            0.,
            f64::from_bits(1),
            0.,
        ],
    )?;
    let scene = decoded_scene(geometry(Some(normals), false)?)?;
    let values = scene.appearances[&0]
        .normals
        .ok_or("missing finite normals")?;
    let diagonal = std::f64::consts::FRAC_1_SQRT_2;
    for (actual, expected) in values.into_iter().zip([
        Vec3::new(diagonal, diagonal, 0.),
        Vec3::new(diagonal, 0., diagonal),
        Vec3::new(0., 1., 0.),
    ]) {
        assert!(close(actual, expected));
    }
    scene.validate()?;
    Ok(())
}

#[test]
fn signed_normalized_minimum_is_saturated_before_direction_normalization() -> Result<()> {
    let mut normals = PointAttribute::new();
    normals.try_init(GeometryAttributeType::Normal, 3, DataType::Int8, true, 3)?;
    normals
        .buffer_mut()
        .update(&[128, 127, 0, 128, 127, 0, 128, 127, 0], Some(0));
    let scene = decoded_scene(geometry(Some(normals), false)?)?;
    let expected = Vec3::new(
        -std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
        0.,
    );
    assert!(
        scene.appearances[&0]
            .normals
            .ok_or("missing integer normals")?
            .into_iter()
            .all(|normal| close(normal, expected))
    );
    Ok(())
}

#[test]
fn zero_corner_normals_use_face_direction_and_all_zero_retains_legacy_path() -> Result<()> {
    let normals = attribute(
        GeometryAttributeType::Normal,
        3,
        &[0., 0., 0., 2., 0., 0., 0., 3., 0.],
    )?;
    let scene = decoded_scene(geometry(Some(normals), false)?)?;
    assert_eq!(
        scene.appearances[&0].normals,
        Some([
            Vec3::new(0., 0., 1.),
            Vec3::new(1., 0., 0.),
            Vec3::new(0., 1., 0.),
        ])
    );
    let absent = decoded_scene(geometry(None, true)?)?;
    let zeros = decoded_scene(geometry(
        Some(attribute(GeometryAttributeType::Normal, 3, &[0.; 9])?),
        true,
    )?)?;
    assert!(zeros.appearances.is_empty());
    let options = RenderOptions {
        width: 64,
        height: 64,
        ..RenderOptions::default()
    };
    assert_eq!(render_xy(&zeros, &options)?, render_xy(&absent, &options)?);
    assert!(
        corner_normals(
            [0, 1, 2],
            [Vec3::default(); 3],
            &[None, Some(Vec3::new(1., 0., 0.)), None]
        )
        .is_none()
    );
    Ok(())
}

#[test]
fn malformed_or_ambiguous_mesh_normals_fail_but_point_cloud_behavior_is_unchanged() -> Result<()> {
    for components in [2, 4] {
        let normals = attribute(
            GeometryAttributeType::Normal,
            components,
            &vec![1.; usize::from(components) * 3],
        )?;
        assert!(
            decoded_scene(geometry(Some(normals), false)?)
                .unwrap_err()
                .to_string()
                .contains("three components")
        );
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let normals = attribute(
            GeometryAttributeType::Normal,
            3,
            &[value, 0., 1., 0., 0., 1., 0., 0., 1.],
        )?;
        assert!(
            decoded_scene(geometry(Some(normals), false)?)
                .unwrap_err()
                .to_string()
                .contains("non-finite")
        );
    }
    let mut ambiguous = geometry(
        Some(attribute(GeometryAttributeType::Normal, 3, &[1.; 9])?),
        false,
    )?;
    ambiguous
        .attributes
        .add_attribute(attribute(GeometryAttributeType::Normal, 3, &[2.; 9])?);
    assert!(
        decoded_scene(ambiguous)
            .unwrap_err()
            .to_string()
            .contains("multiple attributes")
    );
    let mut cloud = geometry(
        Some(attribute(GeometryAttributeType::Normal, 2, &[f64::NAN; 6])?),
        true,
    )?;
    cloud.faces = None;
    let scene = decoded_scene(cloud)?;
    assert_eq!(scene.points.len(), 3);
    assert!(scene.triangles.is_empty() && scene.appearances.is_empty());
    assert_eq!(scene.points[0].color, Some([255, 0, 0]));
    Ok(())
}

#[test]
fn normal_storage_preflights_tree_slack_attributes_and_checked_overflow() {
    assert!(normal_storage(24, 12, 864, 3, 0).is_ok());
    for counts in [
        (3, MAX_FACES, 0, 2, 0),
        (MAX_POINTS, 1, 0, 2, 0),
        (3, 1, MAX_NORMAL_BYTES, 2, 0),
        (3, usize::MAX, 0, 2, 0),
        (usize::MAX, 1, 0, 2, 0),
        (3, 1, 0, usize::MAX, 0),
        (3, 1, 0, 2, usize::MAX),
    ] {
        assert!(normal_storage(counts.0, counts.1, counts.2, counts.3, counts.4).is_err());
    }
}

#[test]
fn authored_normal_shading_preserves_rgb_and_custom_fallback_colors() -> Result<()> {
    for supplied_color in [false, true] {
        let scene = decoded_scene(geometry(
            Some(attribute(
                GeometryAttributeType::Normal,
                3,
                &[1., 0., 0., 1., 0., 0., 1., 0., 0.],
            )?),
            supplied_color,
        )?)?;
        let mut images = Vec::new();
        for fallback in [[255, 0, 0], [0, 255, 0]] {
            let options = RenderOptions {
                width: 64,
                height: 64,
                transparent: true,
                supersampling: 1,
                color: fallback,
                ..RenderOptions::default()
            };
            let image = render_xy(&scene, &options)?;
            // +X normals have no positive diffuse contribution from the fixed
            // light (-0.35,-0.6,+1): intensity is exactly the 0.35 ambient term.
            let expected = if supplied_color {
                [50, 50, 50, 255]
            } else if fallback[0] == 255 {
                [160, 0, 0, 255]
            } else {
                [0, 160, 0, 255]
            };
            assert!(image.pixels().any(|pixel| pixel.0 == expected));
            assert!(
                image
                    .pixels()
                    .filter(|pixel| pixel[3] == 255)
                    .all(|pixel| pixel.0 == expected)
            );
            images.push(image);
        }
        assert_eq!(images[0] == images[1], supplied_color);
        assert_eq!(scene.triangles[0].color, supplied_color.then_some([85; 3]));
    }
    Ok(())
}

#[test]
fn mapped_varying_normals_change_interior_lighting_without_changing_coverage() -> Result<()> {
    let scene = decoded_scene(geometry(
        Some(attribute(
            GeometryAttributeType::Normal,
            3,
            &[0., 0., 1., 1., 0., 0., 0., 1., 0.],
        )?),
        false,
    )?)?;
    let options = RenderOptions {
        width: 96,
        height: 96,
        transparent: true,
        supersampling: 1,
        color: [255; 3],
        ..RenderOptions::default()
    };
    let varied = render_xy(&scene, &options)?;
    let mut flat = scene.clone();
    flat.appearances
        .get_mut(&0)
        .ok_or("missing authored appearance")?
        .normals = Some([Vec3::new(0., 0., 1.); 3]);
    let flat = render_xy(&flat, &options)?;
    assert_eq!(
        varied.pixels().map(|pixel| pixel[3]).collect::<Vec<_>>(),
        flat.pixels().map(|pixel| pixel[3]).collect::<Vec<_>>()
    );
    let intensities: Vec<_> = varied
        .pixels()
        .filter(|pixel| pixel[3] == 255)
        .map(|pixel| pixel[0])
        .collect();
    assert!(
        intensities
            .iter()
            .max()
            .zip(intensities.iter().min())
            .is_some_and(|(maximum, minimum)| maximum - minimum > 50)
    );
    assert_ne!(varied, flat);
    Ok(())
}
