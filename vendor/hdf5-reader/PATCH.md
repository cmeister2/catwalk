# Local hdf5-reader patch

Vendored from the crates.io `hdf5-reader` 0.9.1 package, upstream
https://github.com/roteiro-gis/netcdf-rust (MIT OR Apache-2.0).

The only source changes are three filtered chunk-size reads in
`src/fixed_array.rs`, `src/extensible_array.rs`, and `src/btree_v2.rs`.
They now use the existing bounds-checked `Cursor::read_uvar`, accepting
1 through 8 encoded bytes, instead of `read_length`, which accepts only
2, 4, or 8 bytes for file-wide length fields.

HDF5 layout version 4 encodes a filtered chunk size with one more byte
than needed for its unfiltered size, capped at eight. For example,
512-byte unfiltered chunks have a three-byte size field. This is specified
in the HDF Group file-format reference:
https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t4.html

`src/formats/containers.rs` tests filtered 512-byte chunks through fixed
array, extensible array, and version-2 B-tree indexes, including deflate
and Fletcher32. `tests/containers.rs` renders a compressed NetCDF4
volume using the previously failing three-byte chunk-size encoding.

Remove this patch when an upstream release includes these fixes, after
running those regressions and the container reference corpus.
