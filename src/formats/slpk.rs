//! I3S PointCloud packages: full-detail leaf geometry and LEPCC XYZ/RGB.
//! Mesh scene layers use a different geometry schema and are rejected.
#[path = "lepcc.rs"]
mod lepcc;

use crate::scene::{Point, Scene, Vec3};
use crate::{Error, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::Path;

/// Maximum I3S node-page count visited during leaf traversal.
const MAX_NODES: usize = 1_000_000;
/// Maximum declared point count accepted before geometry allocation.
const MAX_POINTS: usize = 5_000_000;

/// Convert an importer diagnostic into the shared error type.
fn error(message: impl Into<String>) -> Error {
    message.into().into()
}
/// Require a bounded nonnegative integer in the package JSON.
fn integer(value: &Value, label: &str) -> Result<usize> {
    let n = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| error(format!("invalid SLPK {label}")))?;
    usize::try_from(n).map_err(Into::into)
}
/// Require a package-local resource name without unsafe path components.
fn name(value: &Value, label: &str) -> Result<String> {
    let name = if let Some(s) = value.as_str() {
        s.to_owned()
    } else if let Some(n) = value.as_u64() {
        n.to_string()
    } else {
        return Err(error(format!("invalid SLPK {label}")));
    };
    if name.is_empty() || name.contains(['/', '\\', ':', '\0']) || name == "." || name == ".." {
        return Err(error(format!("invalid SLPK {label} path component")));
    }
    Ok(name)
}

/// Indexed I3S ZIP package with aggregate decompression accounting.
struct Package {
    /// Original in-memory ZIP archive.
    archive: zip::ZipArchive<Cursor<Vec<u8>>>,
    /// Normalized member names mapped to archive indices.
    files: HashMap<String, usize>,
    /// Aggregate member bytes expanded during traversal.
    expanded: u64,
}
impl Package {
    /// Index archive members while rejecting duplicate names and unsafe paths.
    fn new(bytes: Vec<u8>) -> Result<Self> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        if archive.len() > MAX_NODES * 16 {
            return Err(error("SLPK archive has too many entries"));
        }
        let mut files = HashMap::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            if file.is_dir() {
                continue;
            }
            let path = file.name().to_ascii_lowercase();
            if path.starts_with('/')
                || path.contains(['\\', ':', '\0'])
                || path
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(error("invalid SLPK archive member path"));
            }
            if file.size() > super::MAX_INPUT_BYTES {
                return Err(error("SLPK archive member exceeds 512 MiB"));
            }
            if files.insert(path, i).is_some() {
                return Err(error("duplicate SLPK archive member path"));
            }
        }
        Ok(Self {
            archive,
            files,
            expanded: 0,
        })
    }
    /// Read a package member, optionally expand gzip and account for aggregate decoded bytes.
    fn read(&mut self, path: &str) -> Result<Vec<u8>> {
        let i = *self
            .files
            .get(&path.to_ascii_lowercase())
            .ok_or_else(|| error(format!("missing SLPK member {path}")))?;
        let mut data = Vec::new();
        self.archive
            .by_index(i)?
            .take(super::MAX_INPUT_BYTES + 1)
            .read_to_end(&mut data)?;
        if data.len() as u64 > super::MAX_INPUT_BYTES {
            return Err(error("SLPK member exceeds 512 MiB"));
        }
        if data.starts_with(&[0x1f, 0x8b]) {
            let mut decoded = Vec::new();
            flate2::read::MultiGzDecoder::new(data.as_slice())
                .take(super::MAX_INPUT_BYTES + 1)
                .read_to_end(&mut decoded)?;
            data = decoded;
        }
        self.expanded = self
            .expanded
            .checked_add(data.len() as u64)
            .ok_or_else(|| error("SLPK expanded-size overflow"))?;
        if self.expanded > super::MAX_INPUT_BYTES {
            return Err(error("SLPK expanded resources exceed 512 MiB"));
        }
        Ok(data)
    }
    /// Parse a package member as JSON after bounded decompression.
    fn json(&mut self, path: &str) -> Result<Value> {
        let plain = format!("{path}.json");
        let compressed = format!("{plain}.gz");
        let a = self.files.contains_key(&plain.to_ascii_lowercase());
        let b = self.files.contains_key(&compressed.to_ascii_lowercase());
        if a && b {
            return Err(error(format!("ambiguous SLPK JSON resource {path}")));
        }
        let data = self.read(if a { &plain } else { &compressed })?;
        serde_json::from_slice(&data).map_err(Into::into)
    }
}

/// Geographic packages store degrees horizontally and meters vertically.
/// Convert to a tangent plane in meters before thumbnail framing.
struct Coordinates {
    /// Whether coordinates are longitude/latitude rather than projected positions.
    geographic: bool,
    /// First geographic point used to define the local tangent-frame origin.
    origin: Option<Vec3>,
    /// Declared vertical-unit conversion into meters.
    height_scale: f64,
    /// Declared vertical origin offset in meters.
    offset: f64,
    /// Ellipsoid eccentricity squared for geographic conversion.
    eccentricity_squared: f64,
}
impl Coordinates {
    /// Resolve supported horizontal and vertical spatial-reference metadata.
    fn new(layer: &Value) -> Result<Self> {
        let sr = &layer["spatialReference"];
        let wkid = sr
            .get("latestWkid")
            .or_else(|| sr.get("wkid"))
            .map(|v| integer(v, "spatial-reference WKID"))
            .transpose()?;
        let geographic = matches!(wkid, Some(4326 | 4490 | 4979));
        if matches!(wkid, Some(4000..=4999)) && !geographic {
            return Err(error("SLPK geographic CRS requires WGS84 or CGCS2000"));
        }
        if wkid.is_none() && sr.get("wkt").is_some() {
            return Err(error("SLPK custom WKT spatial references are unsupported"));
        }
        if let Some(mode) = layer["elevationInfo"]["mode"].as_str()
            && mode != "absoluteHeight"
        {
            return Err(error(
                "SLPK terrain-relative geometry requires a terrain surface",
            ));
        }
        let offset = layer["elevationInfo"]
            .get("offset")
            .map(|v| {
                v.as_f64()
                    .filter(|n| n.is_finite())
                    .ok_or_else(|| error("invalid SLPK elevation offset"))
            })
            .transpose()?
            .unwrap_or(0.);
        let height_scale = match layer["heightModelInfo"]["heightUnit"].as_str() {
            None | Some("meter" | "meters" | "metre" | "metres") => 1.,
            Some("foot" | "feet") => 0.3048,
            Some("us-foot" | "us-survey-foot") => 1200. / 3937.,
            Some(_) => return Err(error("unsupported SLPK height unit")),
        };
        Ok(Self {
            geographic,
            origin: None,
            height_scale,
            offset,
            eccentricity_squared: if wkid == Some(4490) {
                6.694_380_022_900_787e-3
            } else {
                6.694_379_990_141_316_5e-3
            },
        })
    }
    /// Apply stored height units and convert geographic points to local Cartesian coordinates.
    fn apply(&mut self, mut p: Vec3) -> Result<Vec3> {
        p.z = if self.geographic {
            p.z * self.height_scale + self.offset
        } else {
            p.z + self.offset
        };
        if !p.is_finite() {
            return Err(error("non-finite SLPK coordinate"));
        }
        if !self.geographic {
            return Ok(p);
        }
        if p.x.abs() > 180. || p.y.abs() > 90. {
            return Err(error("SLPK longitude/latitude out of range"));
        }
        let origin = *self.origin.get_or_insert(p);
        let lon = origin.x.to_radians();
        let lat = origin.y.to_radians();
        let d = ecef(p, self.eccentricity_squared) - ecef(origin, self.eccentricity_squared);
        let result = Vec3::new(
            -lon.sin() * d.x + lon.cos() * d.y,
            -lat.sin() * lon.cos() * d.x - lat.sin() * lon.sin() * d.y + lat.cos() * d.z,
            lat.cos() * lon.cos() * d.x + lat.cos() * lon.sin() * d.y + lat.sin() * d.z,
        );
        if !result.is_finite() {
            return Err(error("SLPK tangent-plane coordinate overflow"));
        }
        Ok(result)
    }
}
/// Convert longitude, latitude and ellipsoidal height to Earth-centered Cartesian coordinates.
fn ecef(p: Vec3, e2: f64) -> Vec3 {
    let lon = p.x.to_radians();
    let lat = p.y.to_radians();
    let radius = 6_378_137. / (1. - e2 * lat.sin().powi(2)).sqrt();
    Vec3::new(
        (radius + p.z) * lat.cos() * lon.cos(),
        (radius + p.z) * lat.cos() * lon.sin(),
        (radius * (1. - e2) + p.z) * lat.sin(),
    )
}

/// Read a bounded I3S point-cloud scene package and decode its preview geometry.
///
/// # Errors
/// Returns an error for unsafe/duplicate archive paths, invalid node/resource metadata, cyclic
/// traversal, malformed attributes or unsupported spatial references.
pub(super) fn load(path: &Path) -> Result<Scene> {
    decode(super::read_file(path)?)
}
/// Walk leaf node pages, decode LEPCC positions/colors and combine placed point resources.
#[allow(
    clippy::too_many_lines,
    reason = "Keep I3S page/resource traversal together with its archive, cycle and decoded-byte budgets."
)]
fn decode(bytes: Vec<u8>) -> Result<Scene> {
    let mut package = Package::new(bytes)?;
    let layer = package.json("3dSceneLayer")?;
    if layer["layerType"].as_str() != Some("PointCloud") {
        return Err(error("SLPK importer supports I3S PointCloud layers"));
    }
    let store = &layer["store"];
    let version = store["version"]
        .as_str()
        .ok_or_else(|| error("missing SLPK store version"))?;
    let modern = match version {
        "1.6" | "1.7" | "1.8" | "1.9" => false,
        "2.0" => true,
        _ => {
            return Err(error(format!(
                "unsupported I3S PointCloud version {version}"
            )));
        }
    };
    if store["defaultGeometrySchema"]["encoding"].as_str() != Some("lepcc-xyz") {
        return Err(error(
            "SLPK PointCloud geometry must use LEPCC XYZ encoding",
        ));
    }
    let cap = store["index"]
        .get("nodesPerPage")
        .or_else(|| store["index"].get("nodePerIndexBlock"))
        .map(|v| integer(v, "nodes per page"))
        .transpose()?
        .unwrap_or(64);
    if cap == 0 || cap > MAX_NODES {
        return Err(error("invalid SLPK node-page capacity"));
    }
    let mut color_key = None;
    if let Some(attributes) = layer.get("attributeStorageInfo") {
        let attributes = attributes
            .as_array()
            .ok_or_else(|| error("invalid SLPK attributeStorageInfo"))?;
        for attribute in attributes {
            if attribute["name"].as_str() == Some("RGB") {
                if color_key.is_some() {
                    return Err(error("duplicate SLPK RGB attribute"));
                }
                if attribute["encoding"].as_str() != Some("lepcc-rgb") {
                    return Err(error("SLPK RGB attribute must use LEPCC RGB encoding"));
                }
                color_key = Some(name(&attribute["key"], "RGB resource key")?);
            }
        }
    }
    let mut coordinates = Coordinates::new(&layer)?;
    let mut pages = HashMap::<usize, Value>::new();
    let mut seen = HashSet::new();
    let mut resources = HashSet::new();
    let mut todo = vec![0usize];
    let mut scene = Scene::default();
    while let Some(index) = todo.pop() {
        if index >= MAX_NODES || !seen.insert(index) || seen.len() > MAX_NODES {
            return Err(error(
                "SLPK node hierarchy is cyclic, repeated or too large",
            ));
        }
        let ordinal = index / cap;
        if let std::collections::hash_map::Entry::Vacant(entry) = pages.entry(ordinal) {
            let page = if modern { ordinal } else { ordinal * cap };
            let value = package.json(&format!("nodepages/{page}"))?;
            let nodes = value["nodes"]
                .as_array()
                .ok_or_else(|| error("SLPK node page has no nodes array"))?;
            if nodes.is_empty() || nodes.len() > cap {
                return Err(error("invalid SLPK node-page count"));
            }
            entry.insert(value);
        }
        let node = pages[&ordinal]["nodes"]
            .get(index % cap)
            .ok_or_else(|| error("SLPK child index is outside its node page"))?;
        let child_count = integer(&node["childCount"], "child count")?;
        if child_count > 0 {
            let first = integer(&node["firstChild"], "first child")?;
            let end = first
                .checked_add(child_count)
                .filter(|&n| n <= MAX_NODES)
                .ok_or_else(|| error("SLPK child range is too large"))?;
            if todo
                .len()
                .checked_add(child_count)
                .is_none_or(|n| n > MAX_NODES)
            {
                return Err(error("SLPK pending node count exceeds limit"));
            }
            todo.extend((first..end).rev());
            continue;
        }
        let count = integer(
            &node[if modern { "vertexCount" } else { "pointCount" }],
            "point count",
        )?;
        if scene
            .points
            .len()
            .checked_add(count)
            .is_none_or(|n| n > MAX_POINTS)
        {
            return Err(error("SLPK preview exceeds five million points"));
        }
        let resource = name(&node["resourceId"], "geometry resource ID")?;
        if !resources.insert(resource.clone()) {
            return Err(error("SLPK leaf nodes repeat the same geometry resource"));
        }
        if count == 0 {
            continue;
        }
        let data = package.read(&format!("nodes/{resource}/geometries/0.bin.pccxyz"))?;
        let points = lepcc::xyz(&data)?;
        if points.len() != count {
            return Err(error("SLPK node point count differs from LEPCC geometry"));
        }
        let colors = color_key
            .as_ref()
            .map(|key| {
                let bytes =
                    package.read(&format!("nodes/{resource}/attributes/{key}.bin.pccrgb"))?;
                lepcc::rgb(&bytes, count)
            })
            .transpose()?;
        for (i, p) in points.into_iter().enumerate() {
            scene.points.push(Point {
                position: coordinates.apply(p)?,
                color: colors.as_ref().map(|c| c[i]),
            });
        }
    }
    scene.validate()?;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    const PACKAGE: &[u8] = include_bytes!("../../tests/fixtures/slpk/autzen.slpk");
    const XYZ: &[u8] = include_bytes!("../../tests/fixtures/slpk/autzen.xyz");

    fn zip(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    fn layer(version: &str) -> Value {
        serde_json::json!({"layerType":"PointCloud", "spatialReference":{"wkid":4326},
            "store":{"version":version,"index":{"nodesPerPage":2},
            "defaultGeometrySchema":{"encoding":"lepcc-xyz"}}})
    }
    fn bytes(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    #[test]
    fn esri_original_package_renders_geographic_points_in_meters() {
        let scene = decode(PACKAGE.to_vec()).unwrap();
        assert_eq!(scene.points.len(), 106);
        let (min, max) = scene.points.iter().fold(
            (
                Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
                Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
            ),
            |(lo, hi), p| {
                (
                    Vec3::new(
                        lo.x.min(p.position.x),
                        lo.y.min(p.position.y),
                        lo.z.min(p.position.z),
                    ),
                    Vec3::new(
                        hi.x.max(p.position.x),
                        hi.y.max(p.position.y),
                        hi.z.max(p.position.z),
                    ),
                )
            },
        );
        assert!(max.x - min.x > 800. && max.y - min.y > 1200.);
        assert!(max.z - min.z > 30. && max.z - min.z < 50.);
        assert!(crate::render::render(&scene, &crate::render::RenderOptions::default()).is_ok());
    }
    #[test]
    fn both_page_naming_schemes_select_only_leaves_and_resource_ids() {
        for version in ["1.6", "2.0"] {
            let count_key = if version == "2.0" {
                "vertexCount"
            } else {
                "pointCount"
            };
            let mut first = serde_json::json!({"nodes":[
                {"resourceId":99,"firstChild":2,"childCount":1},
                {"resourceId":999,"firstChild":0,"childCount":0}]});
            first["nodes"][0][count_key] = 1.into();
            let mut leaf = serde_json::json!({"nodes":[{"resourceId":"17","childCount":0}]});
            leaf["nodes"][0][count_key] = 106.into();
            let filename = if version == "2.0" {
                "nodepages/1.json"
            } else {
                "nodepages/2.json"
            };
            let package = zip(&[
                ("3dSceneLayer.json", bytes(&layer(version))),
                ("nodepages/0.json", bytes(&first)),
                (filename, bytes(&leaf)),
                ("nodes/17/geometries/0.bin.pccxyz", XYZ.to_vec()),
            ]);
            assert_eq!(decode(package).unwrap().points.len(), 106);
        }
    }
    #[test]
    fn malformed_archives_cycles_and_missing_resources_fail() {
        assert!(decode(b"not a zip".to_vec()).is_err());
        assert!(decode(zip(&[("../outside", vec![])])).is_err());
        assert!(decode(zip(&[("a", vec![]), ("A", vec![])])).is_err());
        let cycle = serde_json::json!({"nodes":[{"resourceId":0,"firstChild":0,"childCount":1}]});
        assert!(
            decode(zip(&[
                ("3dSceneLayer.json", bytes(&layer("2.0"))),
                ("nodepages/0.json", bytes(&cycle))
            ]))
            .is_err()
        );
        let missing =
            serde_json::json!({"nodes":[{"resourceId":0,"childCount":0,"vertexCount":106}]});
        assert!(
            decode(zip(&[
                ("3dSceneLayer.json", bytes(&layer("2.0"))),
                ("nodepages/0.json", bytes(&missing))
            ]))
            .is_err()
        );
        let mut wrong = layer("2.0");
        wrong["layerType"] = "3DObject".into();
        assert!(decode(zip(&[("3dSceneLayer.json", bytes(&wrong))])).is_err());
    }
}
