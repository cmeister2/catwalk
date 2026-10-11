#!/usr/bin/env python3
"""Create a research fixture by removing only Fusion's named Previews members."""
import hashlib
import json
import pathlib
import sys
import zipfile

source, destination = map(pathlib.Path, sys.argv[1:])
if source.resolve() == destination.resolve():
    raise SystemExit("source must be preserved")
with zipfile.ZipFile(source) as original:
    removed = [m.filename for m in original.infolist() if "/Previews/" in m.filename]
    if not removed:
        raise SystemExit("no qualified preview members found")
    with zipfile.ZipFile(destination, "x", compression=zipfile.ZIP_DEFLATED) as output:
        for member in original.infolist():
            if member.filename not in removed:
                output.writestr(member, original.read(member))
    with zipfile.ZipFile(destination) as output:
        identical = all(original.read(m) == output.read(m.filename)
                        for m in original.infolist() if m.filename not in removed)
        assert identical
print(json.dumps({"source": str(source), "derived": str(destination),
                  "removed_members": removed,
                  "nonpreview_members_byte_identical": identical,
                  "sha256": hashlib.sha256(destination.read_bytes()).hexdigest()}, indent=2))
