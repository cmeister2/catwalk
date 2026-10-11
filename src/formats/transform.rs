//! Shared column-major affine placement math for model scene importers.
use crate::{Result, scene::Vec3};

/// Column-major affine matrix, matching glTF and 3MF transform conventions.
#[derive(Clone, Copy)]
pub(super) struct Transform(
    /// Four matrix columns; the last column carries spatial translation.
    pub [[f64; 4]; 4],
);

impl Transform {
    /// Affine identity matrix retaining positions and directions unchanged.
    pub const IDENTITY: Self = Self([
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ]);

    /// Convert a glTF column-major matrix to finite double precision.
    ///
    /// # Errors
    ///
    /// Any matrix component is non-finite.
    pub fn from_gltf(matrix: [[f32; 4]; 4]) -> Result<Self> {
        let matrix = matrix.map(|column| column.map(f64::from));
        if matrix.iter().flatten().any(|v| !v.is_finite()) {
            return Err("non-finite scene transform".into());
        }
        Ok(Self(matrix))
    }

    /// Decode a 3MF twelve-number affine transform or use identity when absent.
    ///
    /// # Errors
    ///
    /// The text does not contain exactly twelve finite numbers.
    pub fn from_3mf(value: Option<&str>) -> Result<Self> {
        let Some(value) = value else {
            return Ok(Self::IDENTITY);
        };
        let values = value
            .split_whitespace()
            .map(str::parse::<f64>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if values.len() != 12 || values.iter().any(|v| !v.is_finite()) {
            return Err("3MF transform must contain 12 finite numbers".into());
        }
        let mut matrix = Self::IDENTITY.0;
        for column in 0..4 {
            for row in 0..3 {
                matrix[column][row] = values[column * 3 + row];
            }
        }
        Ok(Self(matrix))
    }

    /// Compose a child transform beneath this parent, applying the child first.
    pub fn then(self, child: Self) -> Self {
        let mut product = [[0.; 4]; 4];
        for (column, values) in product.iter_mut().enumerate() {
            for (row, value) in values.iter_mut().enumerate() {
                *value = (0..4).map(|k| self.0[k][row] * child.0[column][k]).sum();
            }
        }
        Self(product)
    }

    /// Apply the affine matrix to a spatial point with homogeneous coordinate one.
    pub fn apply(self, v: Vec3) -> Vec3 {
        let values = [v.x, v.y, v.z, 1.];
        let result: [f64; 3] = std::array::from_fn(|row| {
            (0..4)
                .map(|column| self.0[column][row] * values[column])
                .sum()
        });
        Vec3::new(result[0], result[1], result[2])
    }
}
