//! Cumulative allocation and work ceilings for selected stored Lychee meshes.

use crate::Result;

/// Maximum retained source, metadata, topology and reserved output accounting.
const MAX_BYTES: usize = 512 * 1024 * 1024;
/// Maximum selected geometry, byte-transform and structural decode work.
const MAX_WORK: usize = 50_000_000;
/// Maximum distinct stored mesh vertices examined across selected mesh members.
const MAX_VERTICES: usize = 5_000_000;
/// Maximum distinct stored connectivity indices examined across selected meshes.
const MAX_INDICES: usize = 15_000_000;
/// Maximum triangles emitted across all visible object occurrences.
const MAX_TRIANGLES: usize = 5_000_000;

/// Charged allocations and traversal counters shared by metadata and all instances.
#[derive(Default)]
pub(super) struct Budget {
    /// Conservative retained and temporary allocation bytes charged before growth.
    bytes: usize,
    /// Cumulative byte-transform, metadata and coordinate/index operations.
    work: usize,
    /// Cumulative source vertex count, counting shared meshes only once.
    vertices: usize,
    /// Cumulative source index count, counting shared meshes only once.
    indices: usize,
    /// Cumulative reserved visible triangle count across all occurrences.
    triangles: usize,
}

impl Budget {
    /// Charge conservative allocation bytes before allocating a bounded buffer.
    ///
    /// # Errors
    /// Returns an error for multiplication/addition overflow or the 512 MiB ceiling.
    pub(super) fn storage(&mut self, count: usize, size: usize) -> Result<()> {
        let bytes = count
            .checked_mul(size)
            .ok_or("Lychee allocation overflow")?;
        add(
            &mut self.bytes,
            bytes,
            MAX_BYTES,
            "Lychee allocation limit exceeded",
        )
    }

    /// Charge bounded structural, byte-transform or geometry work before traversing.
    ///
    /// # Errors
    /// Returns an error for counter overflow or the fifty-million-operation ceiling.
    pub(super) fn work(&mut self, count: usize) -> Result<()> {
        add(
            &mut self.work,
            count,
            MAX_WORK,
            "Lychee work limit exceeded",
        )
    }

    /// Reserve one unique mesh's complete coordinate and connectivity traversal.
    ///
    /// # Errors
    /// Returns an error when vertices, indices or total work exceed their ceilings.
    pub(super) fn mesh(&mut self, vertices: usize, indices: usize) -> Result<()> {
        add(
            &mut self.vertices,
            vertices,
            MAX_VERTICES,
            "Lychee vertex limit exceeded",
        )?;
        add(
            &mut self.indices,
            indices,
            MAX_INDICES,
            "Lychee index limit exceeded",
        )?;
        self.work(
            vertices
                .checked_add(indices)
                .ok_or("Lychee mesh work overflow")?,
        )
    }

    /// Reserve visible triangle count and geometric-vector capacity before output growth.
    ///
    /// # Errors
    /// Returns an error for visible triangle, corner-work or allocation limits.
    pub(super) fn triangles(&mut self, count: usize) -> Result<()> {
        add(
            &mut self.triangles,
            count,
            MAX_TRIANGLES,
            "Lychee triangle limit exceeded",
        )?;
        self.work(count.checked_mul(3).ok_or("Lychee corner work overflow")?)?;
        self.storage(count, std::mem::size_of::<crate::scene::Triangle>())
    }
}

/// Update a cumulative counter only when the requested addition stays within its ceiling.
fn add(counter: &mut usize, count: usize, ceiling: usize, error: &str) -> Result<()> {
    let total = counter.checked_add(count).ok_or(error)?;
    if total > ceiling {
        return Err(error.into());
    }
    *counter = total;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cumulative_geometry_and_allocation_are_checked_before_growth() {
        let mut budget = Budget {
            triangles: MAX_TRIANGLES - 1,
            ..Default::default()
        };
        budget.triangles(1).unwrap();
        assert!(
            budget
                .triangles(1)
                .unwrap_err()
                .to_string()
                .contains("triangle")
        );
        let mut budget = Budget {
            bytes: MAX_BYTES - 15,
            ..Default::default()
        };
        assert!(
            budget
                .storage(2, 8)
                .unwrap_err()
                .to_string()
                .contains("allocation")
        );
        assert_eq!(budget.bytes, MAX_BYTES - 15);
        assert!(budget.storage(usize::MAX, 2).is_err());
        let mut budget = Budget {
            work: MAX_WORK - 2,
            ..Default::default()
        };
        budget.work(2).unwrap();
        assert!(budget.work(1).unwrap_err().to_string().contains("work"));
    }
}
