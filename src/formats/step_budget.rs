//! Shared selected STEP geometry budgets across analytic, polygonal and AP242 loaders.

use crate::Result;
use crate::scene::{Triangle, TriangleAppearance, Vec3};

/// Maximum coordinates and normal vectors decoded from selected geometry.
pub(super) const MAX_VECTORS: usize = 5_000_000;
/// Maximum selected point/normal and triangle index entries visited.
pub(super) const MAX_INDICES: usize = 15_000_000;
/// Maximum triangles expanded from selected faces and surface sets.
pub(super) const MAX_TRIANGLES: usize = 5_000_000;
/// Maximum cumulative vector, index, reference and corner-processing operations.
pub(super) const MAX_WORK: usize = 50_000_000;
/// Maximum coordinate, index and estimated scene storage allocated by this decoder.
pub(super) const MAX_BYTES: usize = 512 * 1024 * 1024;
/// Maximum cumulative selected faces and surface sets, matching the STEP face ceiling.
pub(super) const MAX_FACES: usize = 100_000;

/// Cumulative budgets shared across every lazily selected item decoded from one file.
#[derive(Default)]
pub(super) struct Budget {
    /// Surface items decoded across all selected representations and containers.
    faces: usize,
    /// Coordinates and normals parsed, counting shared coordinate lists once.
    pub(super) vectors: usize,
    /// Point/normal mapping and primitive-index entries parsed.
    indices: usize,
    /// Output triangle count, including authored zero-area triangles.
    triangles: usize,
    /// Estimated aggregate decoded storage, including corner appearances.
    bytes: usize,
    /// Aggregate decoding and corner expansion operations.
    work: usize,
}

/// Charge a budget before allocation or bounded repeated processing.
fn charge(counter: &mut usize, amount: usize, maximum: usize, description: &str) -> Result<()> {
    *counter = counter
        .checked_add(amount)
        .filter(|&total| total <= maximum)
        .ok_or_else(|| format!("STEP owned geometry exceeds {description} limit"))?;
    Ok(())
}

impl Budget {
    /// Account selected faces across analytic, polygonal and tessellated geometry.
    ///
    /// # Errors
    ///
    /// The cumulative selected face count exceeds the importer ceiling.
    pub(super) fn faces(&mut self, count: usize) -> Result<()> {
        charge(&mut self.faces, count, MAX_FACES, "selected face")
    }

    /// Account finite vector storage and its three component validations.
    ///
    /// # Errors
    ///
    /// A cumulative geometry limit is exceeded or checked accounting overflows.
    pub(super) fn vectors(&mut self, count: usize) -> Result<()> {
        charge(&mut self.vectors, count, MAX_VECTORS, "vector")?;
        self.storage(count, std::mem::size_of::<Vec3>())?;
        self.work(count.checked_mul(3).ok_or("STEP vector work overflows")?)
    }

    /// Account index entries and their validation work before decoding a list.
    ///
    /// # Errors
    ///
    /// A cumulative geometry limit is exceeded or checked accounting overflows.
    pub(super) fn indices(&mut self, count: usize) -> Result<()> {
        charge(&mut self.indices, count, MAX_INDICES, "index")?;
        self.work(count)
    }

    /// Account work before traversing repeated references or triangle corners.
    ///
    /// # Errors
    ///
    /// A cumulative geometry limit is exceeded or checked accounting overflows.
    pub(super) fn work(&mut self, count: usize) -> Result<()> {
        charge(&mut self.work, count, MAX_WORK, "processing work")
    }

    /// Account decoded allocation bytes using checked multiplication.
    ///
    /// # Errors
    ///
    /// A cumulative geometry limit is exceeded or checked accounting overflows.
    pub(super) fn storage(&mut self, count: usize, size: usize) -> Result<()> {
        let bytes = count.checked_mul(size).ok_or("STEP storage overflows")?;
        charge(&mut self.bytes, bytes, MAX_BYTES, "512 MiB decoded storage")
    }

    /// Account triangles, retained corner appearances and three corner lookups before expansion.
    ///
    /// # Errors
    ///
    /// A cumulative geometry limit is exceeded or checked accounting overflows.
    pub(super) fn triangles(&mut self, count: usize, normals: bool) -> Result<()> {
        charge(&mut self.triangles, count, MAX_TRIANGLES, "triangle")?;
        let appearance = if normals {
            std::mem::size_of::<TriangleAppearance>() + 4 * std::mem::size_of::<usize>()
        } else {
            0
        };
        self.storage(count, std::mem::size_of::<Triangle>() + appearance)?;
        self.work(count.checked_mul(3).ok_or("STEP triangle work overflows")?)
    }
}

#[cfg(test)]
mod tests {
    //! Cumulative accounting regressions with no large allocations.
    use super::*;

    #[test]
    fn cumulative_limits_fail_before_large_allocation_or_expansion() {
        let mut budget = Budget::default();
        assert!(budget.vectors(MAX_VECTORS + 1).is_err());
        assert!(Budget::default().indices(MAX_INDICES + 1).is_err());
        assert!(Budget::default().work(MAX_WORK + 1).is_err());
        assert!(Budget::default().storage(usize::MAX, 2).is_err());
        assert!(Budget::default().storage(MAX_BYTES + 1, 1).is_err());
        assert!(
            Budget::default()
                .triangles(MAX_TRIANGLES + 1, false)
                .is_err()
        );
        let mut budget = Budget {
            triangles: MAX_TRIANGLES,
            ..Default::default()
        };
        assert!(budget.triangles(1, true).is_err());
        assert!(budget.faces(MAX_FACES + 1).is_err());
    }
}
