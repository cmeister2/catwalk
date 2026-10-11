These independent fixtures test the Rust I3S/LEPCC importer against Esri's
reference implementation, licensed under Apache-2.0 (LICENSE.txt and NOTICE.txt).

Source: https://github.com/Esri/lepcc

`autzen.slpk` and `autzen-reference.bin` are the unmodified upstream files
`testData/SMALL_AUTZEN_LAS_All.slpk` and `testData/SMALL_AUTZEN_LAS_All.bin`.
`autzen.xyz` is its unchanged `nodes/0/geometries/0.bin.pccxyz` member.
The reference binary contains a 32-bit point count followed by original XYZ
double-precision coordinates, before the LEPCC quantization and sorting.

The RGB streams were generated with Esri's ClusterRGB reference encoder, with
`TryHuffmanOnColor` enabled, and independently decoded using the same reference
library. Inputs (zero-based i):

- `rgb-raw.bin`: 17 colors `[13*i, 7*i, 9*i]`.
- `rgb-constant.bin`: 1024 copies of `[30, 100, 200]`.
- `rgb-huffman.bin`: 1024 colors, red when i is divisible by 17, otherwise green
  when divisible by 29, otherwise blue. Three palette entries with Huffman indices.

Runtime decoding and the checked-in tests use Rust only; the C++ reference
encoder was used solely to prepare these independent compressed fixtures.
