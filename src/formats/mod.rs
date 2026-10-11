//! Explicit importer dispatch. Unsupported readers fail rather than shelling out.
/// First-sample Alembic geometry and object hierarchies.
mod alembic;
/// IGES and native ASCII BREP topology conversion and tessellation.
mod cad;
/// Static CityGML surfaces and diffuse appearances.
mod citygml;
/// HDF5/NetCDF scientific geometry and scalar datasets.
mod containers;
/// Native, deflated, JPEG and RLE DICOM image frames.
pub(crate) mod dicom;
/// Text, binary and compressed DirectX mesh scenes.
mod directx;
/// Pure Rust Draco geometry and attribute decoding.
mod draco;
/// DXF drawing entities, sampled curves and block instances.
mod dxf;
/// Static and first-pose FBX geometry with supported deformations.
mod fbx;
/// glTF/GLB scenes, appearances, deformations and supported extensions.
mod gltf;
/// IFC placements, representation geometry and bounded solid operations.
mod ifc;
/// COLLADA indexed geometry, materials and node transforms.
mod legacy_scene;
/// LAS and Rust-decompressed LAZ survey points.
mod lidar;
/// Polygon, triangle, point and wire interchange readers.
mod mesh;
/// First-frame Quake IDPO model geometry.
mod quakemdl;
/// VTK mesh, grid and multiblock geometry.
mod scientific;
/// I3S point-cloud packages and bounded hierarchy traversal.
mod slpk;
/// Compressed Gaussian-center positions and colors.
mod spz;
/// STEP B-rep shells, length contexts and assembly placements.
mod step;
/// Survey point and trajectory binary layouts.
mod survey;
/// Static 3DS triangle geometry and earliest-key object hierarchy.
mod threeds;
/// Core and production multipart 3MF model packages.
mod threemf;
/// Column-major affine transforms shared by scene importers.
mod transform;
/// Local composed USD stages and supported static geometry.
mod usd;
/// Sparse OpenVDB scalar trees and density surfaces.
mod vdb;
/// Scientific scalar volumes and exposed voxel surfaces.
mod volume;
/// VRML97 shapes, instances, transforms and colors.
mod vrml;
/// Binary XCAF document geometry, placement and color tables.
mod xbf;

use crate::{Result, scene::Scene};
use std::path::Path;

/// Advertised extension aliases with a Rust geometry reader.
pub const MODEL_EXTENSIONS: &[&str] = &[
    "stl", "obj", "ply", "off", "pts", "ptx", "pcd", "splat", "gltf", "glb", "3mf", "vtk", "vtp",
    "vtu", "vtr", "vts", "vtm", "dae", "las", "laz", "step", "stp", "stpnc", "p21", "210", "mdl",
    "3ds", "wrl", "vrml", "gml", "usd", "usda", "usdc", "usdz", "spz", "vti", "nrrd", "nhdr",
    "mhd", "mha", "dxf", "bpf", "bin", "qi", "sbet", "csd", "x", "drc", "fbx", "vtkhdf", "e",
    "ex2", "exo", "g", "nc", "cdf", "ncdf", "slpk", "abc", "ifc", "vdb", "iges", "igs", "brep",
    "xbf",
];
/// Advertised extension aliases decoded as fitted two-dimensional images.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "bmp", "tga", "hdr", "webp", "exr", "tif", "tiff", "dcm",
];

/// Return the path's extension in ASCII lowercase, without its leading dot.
///
/// # Errors
/// Returns an error when the path has no extension or its extension is not UTF-8.
pub fn extension(path: &Path) -> Result<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "input needs a recognized file extension".into())
}

/// Import supported model geometry and appearances into a validated Z-up scene.
///
/// # Errors
/// Returns an error for unsupported extensions, unreadable resources, malformed
/// or unimplemented file variants, exceeded reader limits, or invalid geometry.
pub fn load(path: &Path) -> Result<Scene> {
    let scene = match extension(path)?.as_str() {
        "stl" | "obj" | "ply" | "off" | "pts" | "ptx" | "pcd" | "splat" => mesh::load(path)?,
        "gltf" | "glb" => gltf::load(path)?,
        "dae" => legacy_scene::load(path)?,
        "gml" => citygml::load(path)?,
        "dxf" => dxf::load(path)?,
        "x" => directx::load(path)?,
        "drc" => draco::load(path)?,
        "fbx" => fbx::load(path)?,
        "abc" => alembic::load(path)?,
        "ifc" => ifc::load(path)?,
        "vdb" => vdb::load(path)?,
        "iges" | "igs" | "brep" => cad::load(path)?,
        "xbf" => xbf::load(path)?,
        "slpk" => slpk::load(path)?,
        "vtkhdf" | "e" | "ex2" | "exo" | "g" | "nc" | "cdf" | "ncdf" => containers::load(path)?,
        "usd" | "usda" | "usdc" | "usdz" => usd::load(path)?,
        "las" | "laz" => lidar::load(path)?,
        "step" | "stp" | "stpnc" | "p21" | "210" => step::load(path)?,
        "mdl" => quakemdl::load(path)?,
        "spz" => spz::load(path)?,
        "bpf" | "bin" | "qi" | "sbet" | "csd" => survey::load(path)?,
        "vti" | "nrrd" | "nhdr" | "mhd" | "mha" => volume::load(path)?,
        "3ds" => threeds::load(path)?,
        "wrl" | "vrml" => vrml::load(path)?,
        "3mf" => threemf::load(path)?,
        "vtk" | "vtp" | "vtu" | "vtr" | "vts" | "vtm" => scientific::load(path)?,
        ext => {
            return Err(format!(
                "unsupported model format .{ext}; see --list-formats for implemented readers"
            )
            .into());
        }
    };
    scene.validate()?;
    Ok(scene)
}

/// Shared limit for the primary file and each referenced buffer/model part.
pub(crate) const MAX_INPUT_BYTES: u64 = 512 * 1024 * 1024;

/// Read a primary or referenced input with the shared 512 MiB byte limit.
pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut data = Vec::new();
    file.take(MAX_INPUT_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_INPUT_BYTES {
        return Err("input exceeds the 512 MiB limit".into());
    }
    Ok(data)
}

/// Decode relative resource paths without interpreting them as shell commands or URLs.
pub(crate) fn relative_resource(parent: &Path, uri: &str) -> Result<std::path::PathBuf> {
    let mut decoded = Vec::new();
    let bytes = uri.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("truncated percent escape in resource URI".into());
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])?;
            decoded.push(u8::from_str_radix(hex, 16)?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(decoded)?;
    if text.contains([':', '\0', '\\', '?', '#']) {
        return Err("resource URI must be a relative local path".into());
    }
    let resource = Path::new(&text);
    if resource.is_absolute()
        || resource
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        return Err("resource URI must stay within the model directory".into());
    }
    let root = parent.canonicalize()?;
    let path = root.join(resource).canonicalize()?;
    if !path.starts_with(&root) {
        return Err("resource resolves outside the model directory".into());
    }
    Ok(path)
}
