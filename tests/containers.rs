use hdf5_writer::{AttributeBuilder, DatasetBuilder, Hdf5Builder, WriteOptions};
use netcdf_writer::{NcAttrValue, NcFileBuilder, NcWriteFormat, NcWriteOptions};
use std::{path::Path, process::Command};

fn exodus_cube() -> Vec<u8> {
    let mut builder = NcFileBuilder::new();
    let dim = builder.add_dimension("num_dim", 3).unwrap();
    let nodes = builder.add_dimension("num_nodes", 8).unwrap();
    builder.add_dimension("num_elem", 1).unwrap();
    builder.add_dimension("num_el_blk", 1).unwrap();
    let elems = builder.add_dimension("num_el_in_blk1", 1).unwrap();
    let corners = builder.add_dimension("num_nod_per_el1", 8).unwrap();
    let coord = builder.add_variable::<f64>("coord", &[dim, nodes]).unwrap();
    builder
        .write_variable(
            coord,
            &[
                0., 1., 1., 0., 0., 1., 1., 0., 0., 0., 1., 1., 0., 0., 1., 1., 0., 0., 0., 0., 1.,
                1., 1., 1.,
            ],
        )
        .unwrap();
    let conn = builder
        .add_variable::<i32>("connect1", &[elems, corners])
        .unwrap();
    builder
        .add_variable_attribute(conn, "elem_type", NcAttrValue::Chars("HEX8".into()))
        .unwrap();
    builder
        .write_variable(conn, &[1, 2, 3, 4, 5, 6, 7, 8])
        .unwrap();
    builder
        .to_vec(NcWriteOptions {
            format: NcWriteFormat::Classic,
        })
        .unwrap()
        .1
}

fn vtkhdf_cube() -> Vec<u8> {
    let mut builder = Hdf5Builder::new()
        .group_attribute(
            "VTKHDF",
            AttributeBuilder::fixed_string("Type", "UnstructuredGrid"),
        )
        .group_attribute(
            "VTKHDF",
            AttributeBuilder::vector("Version", &[2i32, 0]).unwrap(),
        )
        .dataset(
            DatasetBuilder::typed_data(
                "VTKHDF/Points",
                vec![8, 3],
                &[
                    0f64, 0., 0., 1., 0., 0., 1., 1., 0., 0., 1., 0., 0., 0., 1., 1., 0., 1., 1.,
                    1., 1., 0., 1., 1.,
                ],
            )
            .unwrap(),
        );
    for (name, values) in [
        ("NumberOfPoints", vec![8i64]),
        ("NumberOfCells", vec![1]),
        ("NumberOfConnectivityIds", vec![8]),
        ("Offsets", vec![0, 8]),
        ("Connectivity", vec![0, 1, 2, 3, 4, 5, 6, 7]),
        ("Types", vec![12]),
    ] {
        builder = builder.dataset(
            DatasetBuilder::typed_data(
                format!("VTKHDF/{name}"),
                vec![values.len() as u64],
                &values,
            )
            .unwrap(),
        );
    }
    builder
        .into_plan()
        .unwrap()
        .encode(WriteOptions::default())
        .unwrap()
}

fn netcdf_shell() -> Vec<u8> {
    let mut builder = NcFileBuilder::new();
    let z = builder.add_dimension("z", 16).unwrap();
    let y = builder.add_dimension("y", 16).unwrap();
    let x = builder.add_dimension("x", 16).unwrap();
    let scalar = builder.add_variable::<u8>("density", &[z, y, x]).unwrap();
    let mut density = Vec::new();
    for z in 0..16i32 {
        for y in 0..16i32 {
            for x in 0..16i32 {
                let radius = (x - 8).pow(2) + (y - 8).pow(2) + (z - 8).pow(2);
                density.push(if (9..=25).contains(&radius) { 100u8 } else { 0 });
            }
        }
    }
    builder.write_variable(scalar, &density).unwrap();
    builder
        .set_variable_chunking(scalar, vec![8, 8, 8])
        .unwrap();
    builder.set_variable_deflate(scalar, Some(6), true).unwrap();
    builder
        .to_vec(NcWriteOptions {
            format: NcWriteFormat::Nc4,
        })
        .unwrap()
        .1
}

fn thumbnail(directory: &Path, name: &str, bytes: &[u8]) -> Vec<u8> {
    let input = directory.join(name);
    let output = directory.join(format!("{name}.png"));
    std::fs::write(&input, bytes).unwrap();
    let scene = catwalk::formats::load(&input).unwrap();
    assert!(scene.triangles.len() >= 12);
    let result = Command::new(env!("CARGO_BIN_EXE_catwalk"))
        .env_remove("DISPLAY")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .arg("--size")
        .arg("128")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let image = image::ImageReader::open(&output)
        .unwrap()
        .decode()
        .unwrap()
        .to_rgba8();
    assert_eq!(image.dimensions(), (128, 128));
    assert!(image.pixels().filter(|p| p[3] > 0).count() > 500);
    std::fs::read(output).unwrap()
}

#[test]
fn scientific_container_aliases_render_actual_mesh_and_density_headlessly() {
    let directory =
        std::env::temp_dir().join(format!("catwalk-container-cli-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let exodus = exodus_cube();
    let reference = thumbnail(&directory, "cube.vtkhdf", &vtkhdf_cube());
    for extension in ["e", "ex2", "exo", "g"] {
        assert_eq!(
            thumbnail(&directory, &format!("cube.{extension}"), &exodus),
            reference
        );
    }
    let netcdf = netcdf_shell();
    let reference = thumbnail(&directory, "shell.nc", &netcdf);
    for extension in ["cdf", "ncdf"] {
        assert_eq!(
            thumbnail(&directory, &format!("shell.{extension}"), &netcdf),
            reference
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
