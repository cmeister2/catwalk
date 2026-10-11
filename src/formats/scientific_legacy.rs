//! Legacy VTK geometry, with big-endian binary words and version 5 split connectivity.
use super::arrays::Scalar;
use super::{MAX_ITEMS, append_grid, append_line, append_polydata, append_structured};
use crate::Result;
use crate::scene::{Scene, Vec3};

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    binary: bool,
    split_cells: bool,
}
impl Reader<'_> {
    fn line(&mut self) -> Result<&str> {
        let end = self.data[self.at..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|n| n + self.at)
            .unwrap_or(self.data.len());
        let start = self.at;
        self.at = (end + 1).min(self.data.len());
        Ok(std::str::from_utf8(&self.data[start..end])?.trim_end_matches('\r'))
    }
    fn token(&mut self) -> Result<&str> {
        while self.data.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
        let start = self.at;
        while self
            .data
            .get(self.at)
            .is_some_and(|b| !b.is_ascii_whitespace())
        {
            self.at += 1;
        }
        if start == self.at {
            return Err("truncated VTK geometry".into());
        }
        Ok(std::str::from_utf8(&self.data[start..self.at])?)
    }
    fn end(&self) -> bool {
        self.data[self.at..].iter().all(u8::is_ascii_whitespace)
    }
    fn expect(&mut self, expected: &str) -> Result<()> {
        if !self.token()?.eq_ignore_ascii_case(expected) {
            return Err(format!("expected VTK {expected}").into());
        }
        Ok(())
    }
    fn count(&mut self) -> Result<usize> {
        let n: usize = self.token()?.parse()?;
        if n > MAX_ITEMS {
            return Err("VTK count exceeds geometry limit".into());
        }
        Ok(n)
    }
    fn coordinate(&mut self) -> Result<f64> {
        let n: f64 = self.token()?.parse()?;
        if !n.is_finite() {
            return Err("VTK coordinate is not finite".into());
        }
        Ok(n)
    }
    fn raw(&mut self, count: usize, ty: Scalar) -> Result<&[u8]> {
        while self
            .data
            .get(self.at)
            .is_some_and(|&b| matches!(b, b' ' | b'\t' | b'\r'))
        {
            self.at += 1;
        }
        if self.data.get(self.at) != Some(&b'\n') {
            return Err("VTK binary data must follow a newline".into());
        }
        self.at += 1;
        let size = count
            .checked_mul(ty.width())
            .ok_or("VTK binary array size overflow")?;
        let end = self
            .at
            .checked_add(size)
            .ok_or("VTK binary array offset overflow")?;
        let bytes = self
            .data
            .get(self.at..end)
            .ok_or("truncated VTK binary array")?;
        self.at = end;
        Ok(bytes)
    }
    fn floats(&mut self, count: usize, ty: Scalar) -> Result<Vec<f64>> {
        if self.binary {
            self.raw(count, ty)?
                .chunks_exact(ty.width())
                .map(|w| ty.float(w, true))
                .collect()
        } else {
            (0..count).map(|_| self.coordinate()).collect()
        }
    }
    fn indices(&mut self, count: usize, ty: Scalar) -> Result<Vec<usize>> {
        if count > MAX_ITEMS {
            return Err("VTK connectivity array exceeds geometry limit".into());
        }
        if self.binary {
            self.raw(count, ty)?
                .chunks_exact(ty.width())
                .map(|w| {
                    let n = usize::try_from(ty.integer(w, true)?)?;
                    if n > MAX_ITEMS {
                        return Err("VTK connectivity index exceeds geometry limit".into());
                    }
                    Ok(n)
                })
                .collect()
        } else {
            (0..count).map(|_| self.count()).collect()
        }
    }
    fn cells(&mut self) -> Result<Vec<Vec<usize>>> {
        let count = self.count()?;
        let total = self.count()?;
        if self.split_cells {
            if count == 0 {
                return Err("VTK split cell offsets must contain an initial zero".into());
            }
            self.expect("OFFSETS")?;
            let ty = Scalar::parse(&self.token()?.to_ascii_lowercase())?;
            let offsets = self.indices(count, ty)?;
            self.expect("CONNECTIVITY")?;
            let ty = Scalar::parse(&self.token()?.to_ascii_lowercase())?;
            let indices = self.indices(total, ty)?;
            if offsets.first() != Some(&0) || offsets.last() != Some(&total) {
                return Err("VTK split connectivity offsets disagree with header".into());
            }
            offsets
                .windows(2)
                .map(|pair| {
                    Ok(indices
                        .get(pair[0]..pair[1])
                        .ok_or("invalid VTK split connectivity offset")?
                        .to_vec())
                })
                .collect()
        } else {
            let values = self.indices(total, Scalar::I32)?;
            let mut at = 0usize;
            let mut cells = Vec::new();
            for _ in 0..count {
                let n = *values.get(at).ok_or("truncated VTK cell list")?;
                at += 1;
                let end = at.checked_add(n).ok_or("VTK cell size overflow")?;
                cells.push(
                    values
                        .get(at..end)
                        .ok_or("truncated VTK cell indices")?
                        .to_vec(),
                );
                at = end;
            }
            if at != total {
                return Err("VTK cell list size disagrees with header".into());
            }
            Ok(cells)
        }
    }
    fn metadata(&mut self) -> Result<()> {
        // vtkDataWriter terminates array metadata with an empty line.
        self.line()?;
        let start = self.at;
        for _ in 0..4096 {
            if self.at - start > 1_048_576 {
                break;
            }
            if self.line()?.trim().is_empty() {
                return Ok(());
            }
        }
        Err("VTK metadata exceeds limit or is not terminated".into())
    }
    fn field(&mut self) -> Result<()> {
        self.token()?;
        let count = self.count()?;
        for _ in 0..count {
            self.token()?;
            let components = self.count()?;
            let tuples = self.count()?;
            let ty = Scalar::parse(&self.token()?.to_ascii_lowercase())?;
            let n = components
                .checked_mul(tuples)
                .ok_or("VTK field size overflow")?;
            if n > MAX_ITEMS * 3 {
                return Err("VTK field exceeds array limit".into());
            }
            if self.binary {
                self.raw(n, ty)?;
            } else {
                for _ in 0..n {
                    self.token()?;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<Scene> {
    let mut reader = Reader {
        data: bytes,
        at: 0,
        binary: false,
        split_cells: false,
    };
    let header = reader.line()?;
    let version = header
        .strip_prefix("# vtk DataFile Version ")
        .ok_or("missing VTK legacy header")?
        .trim()
        .parse::<f64>()?;
    if !version.is_finite() || version <= 0. || version >= 6. {
        return Err("unsupported VTK legacy version".into());
    }
    reader.split_cells = version >= 5.;
    reader.line()?;
    reader.binary = match reader.line()?.trim() {
        "ASCII" => false,
        "BINARY" => true,
        _ => return Err("invalid VTK legacy encoding".into()),
    };
    reader.expect("DATASET")?;
    let dataset = reader.token()?.to_ascii_uppercase();
    if !matches!(
        dataset.as_str(),
        "POLYDATA"
            | "UNSTRUCTURED_GRID"
            | "STRUCTURED_GRID"
            | "RECTILINEAR_GRID"
            | "STRUCTURED_POINTS"
    ) {
        return Err(format!("VTK legacy dataset {dataset} is not implemented").into());
    }
    let mut positions = None;
    let mut polygons = Vec::new();
    let mut strips = Vec::new();
    let mut vertices = Vec::new();
    let mut lines = Vec::new();
    let mut cells = None;
    let mut types = None;
    let mut dimensions = None;
    let mut axes: [Option<Vec<f64>>; 3] = [None, None, None];
    let mut origin = Vec3::default();
    let mut spacing = Vec3::new(1., 1., 1.);
    while !reader.end() {
        let keyword = reader.token()?.to_ascii_uppercase();
        match keyword.as_str() {
            "POINTS" => {
                if positions.is_some() {
                    return Err("duplicate VTK POINTS section".into());
                }
                let count = reader.count()?;
                let ty = Scalar::parse(&reader.token()?.to_ascii_lowercase())?;
                let values = reader.floats(count * 3, ty)?;
                positions = Some(
                    values
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|p| Vec3::new(p[0], p[1], p[2]))
                        .collect::<Vec<_>>(),
                );
            }
            "POLYGONS" if dataset == "POLYDATA" => polygons.extend(reader.cells()?),
            "TRIANGLE_STRIPS" if dataset == "POLYDATA" => strips.extend(reader.cells()?),
            "VERTICES" if dataset == "POLYDATA" => vertices.extend(reader.cells()?),
            "LINES" if dataset == "POLYDATA" => lines.extend(reader.cells()?),
            "CELLS" if dataset == "UNSTRUCTURED_GRID" => {
                if cells.is_some() {
                    return Err("duplicate VTK CELLS section".into());
                }
                cells = Some(reader.cells()?);
            }
            "CELL_TYPES" if dataset == "UNSTRUCTURED_GRID" => {
                if types.is_some() {
                    return Err("duplicate VTK CELL_TYPES section".into());
                }
                let count = reader.count()?;
                types = Some(reader.indices(count, Scalar::I32)?);
            }
            "DIMENSIONS" if dataset.ends_with("GRID") || dataset == "STRUCTURED_POINTS" => {
                if dimensions.is_some() {
                    return Err("duplicate VTK DIMENSIONS section".into());
                }
                let dims = [reader.count()?, reader.count()?, reader.count()?];
                if dims.contains(&0)
                    || dims
                        .iter()
                        .try_fold(1usize, |n, &d| n.checked_mul(d))
                        .is_none_or(|n| n > MAX_ITEMS)
                {
                    return Err("invalid or oversized VTK structured dimensions".into());
                }
                dimensions = Some(dims);
            }
            "X_COORDINATES" | "Y_COORDINATES" | "Z_COORDINATES"
                if dataset == "RECTILINEAR_GRID" =>
            {
                let axis = match keyword.as_str() {
                    "X_COORDINATES" => 0,
                    "Y_COORDINATES" => 1,
                    _ => 2,
                };
                if axes[axis].is_some() {
                    return Err("duplicate VTK coordinate axis".into());
                }
                let count = reader.count()?;
                let ty = Scalar::parse(&reader.token()?.to_ascii_lowercase())?;
                axes[axis] = Some(reader.floats(count, ty)?);
            }
            "ORIGIN" if dataset == "STRUCTURED_POINTS" => {
                origin = Vec3::new(
                    reader.coordinate()?,
                    reader.coordinate()?,
                    reader.coordinate()?,
                )
            }
            "SPACING" | "ASPECT_RATIO" if dataset == "STRUCTURED_POINTS" => {
                spacing = Vec3::new(
                    reader.coordinate()?,
                    reader.coordinate()?,
                    reader.coordinate()?,
                );
                if spacing.x <= 0. || spacing.y <= 0. || spacing.z <= 0. {
                    return Err("VTK structured spacing must be positive".into());
                }
            }
            "METADATA" => reader.metadata()?,
            "FIELD" => reader.field()?,
            "POINT_DATA" | "CELL_DATA" => break,
            _ => return Err(format!("VTK section {keyword} is not implemented").into()),
        }
    }
    let mut scene = Scene::default();
    match dataset.as_str() {
        "POLYDATA" => {
            let positions = positions.ok_or("VTK model has no POINTS")?;
            append_polydata(&positions, polygons, strips, vertices, &mut scene)?;
            for line in lines {
                append_line(&positions, &line, &mut scene)?;
            }
        }
        "UNSTRUCTURED_GRID" => append_grid(
            &positions.ok_or("VTK grid has no POINTS")?,
            &cells.ok_or("VTK grid has no CELLS")?,
            &types.ok_or("VTK grid has no CELL_TYPES")?,
            &mut scene,
        )?,
        _ => {
            let dims = dimensions.ok_or("VTK structured dataset has no DIMENSIONS")?;
            let positions = if dataset == "STRUCTURED_GRID" {
                positions.ok_or("VTK structured grid has no POINTS")?
            } else {
                for axis in 0..3 {
                    if dataset == "STRUCTURED_POINTS" {
                        let (o, s) = [
                            (origin.x, spacing.x),
                            (origin.y, spacing.y),
                            (origin.z, spacing.z),
                        ][axis];
                        axes[axis] = Some((0..dims[axis]).map(|i| o + i as f64 * s).collect());
                    }
                    let values = axes[axis]
                        .as_ref()
                        .ok_or("VTK rectilinear coordinate axis is missing")?;
                    if values.len() != dims[axis] {
                        return Err("VTK coordinate count disagrees with dimensions".into());
                    }
                    if values
                        .windows(2)
                        .any(|w| w[0] == w[1] || (w[1] > w[0]) != (values[1] > values[0]))
                    {
                        return Err("VTK rectilinear coordinates must be strictly monotonic".into());
                    }
                }
                let mut positions = Vec::new();
                for &z in axes[2].as_ref().unwrap() {
                    for &y in axes[1].as_ref().unwrap() {
                        for &x in axes[0].as_ref().unwrap() {
                            positions.push(Vec3::new(x, y, z));
                        }
                    }
                }
                positions
            };
            if positions.len() != dims.iter().product::<usize>() {
                return Err("VTK structured point count disagrees with dimensions".into());
            }
            append_structured(&positions, dims, &mut scene)?;
        }
    }
    scene.validate()?;
    Ok(scene)
}
