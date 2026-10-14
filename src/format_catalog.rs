//! Machine-readable discovery built from the readers linked into this executable.

use catwalk3d::{formats, saved_preview};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Normalize, sort and deduplicate every alias from the selected reader registries.
fn extensions(registries: &[&[&str]]) -> Vec<String> {
    registries
        .iter()
        .flat_map(|registry| registry.iter())
        .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Build schema version 1 without inspecting a model or opening an output file.
pub(crate) fn json() -> Value {
    let registered = extensions(&[
        formats::MODEL_EXTENSIONS,
        formats::IMAGE_EXTENSIONS,
        saved_preview::EXTENSIONS,
        formats::NATIVE_GEOMETRY_EXTENSIONS,
    ]);
    let entries: Vec<_> = registered
        .iter()
        .map(|extension| {
            // Discovery must still include a newly registered reader before its
            // dedicated label is added. The coverage test requires named labels.
            let description = description(extension).map_or_else(
                || format!("{} file", extension.to_ascii_uppercase()),
                str::to_owned,
            );
            json!({"extension": extension, "description": description})
        })
        .collect();
    json!({
        "schema_version": 1,
        "catwalk_version": env!("CARGO_PKG_VERSION"),
        "extensions": registered,
        "formats": entries,
        "categories": {
            "geometry": extensions(&[
                formats::MODEL_EXTENSIONS,
                formats::NATIVE_GEOMETRY_EXTENSIONS,
            ]),
            "image": extensions(&[formats::IMAGE_EXTENSIONS]),
            "saved_preview": extensions(&[saved_preview::EXTENSIONS]),
            "native_geometry": extensions(&[formats::NATIVE_GEOMETRY_EXTENSIONS]),
        },
    })
}

/// Human-readable English format labels, with a separate entry for each alias.
fn description(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "stl" => "STL mesh",
        "obj" => "Wavefront OBJ model",
        "ply" => "PLY polygon or point cloud",
        "off" => "Object File Format mesh",
        "pts" => "PTS point cloud",
        "ptx" => "PTX point cloud",
        "pcd" => "PCD point cloud",
        "splat" => "Gaussian splat point cloud",
        "gltf" => "glTF model",
        "glb" => "Binary glTF model",
        "3mf" => "3D Manufacturing Format model",
        "vtk" => "VTK legacy dataset",
        "vtp" => "VTK PolyData XML dataset",
        "vtu" => "VTK UnstructuredGrid XML dataset",
        "vtr" => "VTK RectilinearGrid XML dataset",
        "vts" => "VTK StructuredGrid XML dataset",
        "vtm" => "VTK MultiBlock XML dataset",
        "vti" => "VTK ImageData XML volume",
        "dae" => "COLLADA model",
        "las" => "LAS point cloud",
        "laz" => "Compressed LAS point cloud",
        "step" | "stp" | "stpnc" | "p21" | "210" => "STEP model",
        "mdl" => "Quake MDL model",
        "3ds" => "3D Studio model",
        "wrl" | "vrml" => "VRML model",
        "gml" => "CityGML model",
        "usd" => "Universal Scene Description stage",
        "usda" => "Universal Scene Description stage (text)",
        "usdc" => "Universal Scene Description stage (binary)",
        "usdz" => "Universal Scene Description archive",
        "spz" => "Compressed SPZ Gaussian splat point cloud",
        "nrrd" | "nhdr" => "NRRD scalar volume",
        "mhd" | "mha" => "MetaImage scalar volume",
        "dxf" => "AutoCAD DXF drawing",
        "bpf" => "Binary Point File point cloud",
        "bin" => "TerraScan BIN point cloud",
        "qi" => "QFIT point cloud",
        "sbet" => "SBET trajectory",
        "csd" => "Optech CSD point cloud",
        "x" => "DirectX model",
        "drc" => "Draco mesh or point cloud",
        "fbx" => "Autodesk FBX model",
        "vtkhdf" => "VTK HDF dataset",
        "e" | "ex2" | "exo" | "g" => "Exodus II dataset",
        "nc" | "cdf" | "ncdf" => "NetCDF dataset",
        "slpk" => "Scene Layer Package point cloud",
        "abc" => "Alembic scene",
        "ifc" => "Industry Foundation Classes model",
        "vdb" => "OpenVDB scalar volume",
        "iges" | "igs" => "IGES model",
        "brep" => "Open CASCADE boundary representation",
        "xbf" => "Open CASCADE XBF document",
        "amf" => "Additive Manufacturing File model",
        "lys" => "Lychee Slicer scene",
        "tif" | "tiff" => "TIFF image or scalar volume",
        "fcstd" => "FreeCAD document",
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "bmp" => "BMP image",
        "tga" => "TGA image",
        "hdr" => "Radiance HDR image",
        "webp" => "WebP image",
        "exr" => "OpenEXR image",
        "dcm" => "DICOM image",
        "blend" => "Blender scene",
        "lbrn" | "lbrn2" => "LightBurn project",
        "dwg" => "AutoCAD DWG drawing",
        "ipt" => "Autodesk Inventor part",
        "iam" => "Autodesk Inventor assembly",
        "f3d" => "Autodesk Fusion design",
        "skp" => "SketchUp model",
        "max" => "Autodesk 3ds Max scene",
        "3dm" => "Rhino model",
        "sldprt" => "SolidWorks part",
        "sldasm" => "SolidWorks assembly",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_alias_has_a_dedicated_display_description() {
        for extension in extensions(&[
            formats::MODEL_EXTENSIONS,
            formats::IMAGE_EXTENSIONS,
            saved_preview::EXTENSIONS,
            formats::NATIVE_GEOMETRY_EXTENSIONS,
        ]) {
            assert!(
                description(&extension).is_some_and(|label| !label.trim().is_empty()),
                "add a display description for registered extension .{extension}"
            );
        }
    }
}
