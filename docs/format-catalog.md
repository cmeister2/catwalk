# Machine-readable format discovery

Run `catwalk --list-formats --json` to discover the readers available in that executable. `--list-readers --json` is equivalent. The command needs no model or output image and writes one JSON document followed by a newline to stdout. Diagnostics go to stderr; failures, including output write failures, return a nonzero exit code. `--json` requires `--list-formats` or its alias. The existing human-readable output remains the default.

The [JSON Schema](format-catalog.schema.json) defines schema version 1:

| Field | Meaning |
| --- | --- |
| `schema_version` | Integer `1`. Check this before consuming the catalog. |
| `catwalk_version` | Version of the executable, matching `catwalk --version`. |
| `extensions` | Complete sorted, deduplicated array of lowercase extensions without leading dots. Includes every alias from the model, image, saved-preview and native-document geometry registries. |
| `formats` | One record per extension, ordered to match `extensions`. Each record has `extension` and a human-readable English `description`. |
| `categories.geometry` | Union of model and native-document geometry readers. |
| `categories.image` | Raster and DICOM image readers. |
| `categories.saved_preview` | Readers that extract saved document images. |
| `categories.native_geometry` | Native-document geometry readers, also included in `geometry`. |

All category arrays are sorted, deduplicated and use the same extension normalization. Categories overlap: TIFF belongs to both `image` and `geometry`, and documents can support both `saved_preview` and `geometry`. An extension's inclusion advertises a reader for its documented subset; it does not promise support for every file variant. See [format coverage](format-coverage.md).

Descriptions are display text, not identifiers; wording may change without increasing the schema version. Aliases remain separate records and may share a description. Consumers should match on `extension`, reject unsupported schema versions, and ignore additional fields or category keys so the schema can gain compatible metadata.

For a downstream CI coverage check, compare the `extensions` array with the integration's supported and deliberately excluded extensions. This avoids parsing prose or depending on reader-group headings.

For Manyfold's Thumbnail Settings table, `formats` supplies the extension and format description. The integration can apply its own MIME grouping, image exclusions and provider settings. For example:

```sh
catwalk --list-formats --json | jq -r '.formats[] | "\(.extension)\t\(.description)"'
```

An individual record can be selected without parsing display text:

```sh
catwalk --list-formats --json | jq '.formats[] | select(.extension == "sldprt")'
```

```json
{
  "extension": "sldprt",
  "description": "SolidWorks part"
}
```
