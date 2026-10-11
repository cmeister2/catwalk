#!/usr/bin/env python3
"""Set the crate and lockfile root version for a semantic-release publication."""

import re
import sys
import tomllib
from pathlib import Path


def set_version(text: str, header: str, version: str) -> str:
    """Update only the matching catwalk3d package section, preserving other bytes."""
    sections = re.split(r"(?m)(?=^\[)", text)
    matches = 0
    for index, section in enumerate(sections):
        if not section.startswith(header + "\n"):
            continue
        parsed = tomllib.loads(section)["package"]
        package = parsed[0] if isinstance(parsed, list) else parsed
        if package.get("name") != "catwalk3d" or "source" in package:
            continue
        updated, count = re.subn(
            r'(?m)^(version\s*=\s*)"[^"\n]+"',
            lambda match: f'{match.group(1)}"{version}"',
            section,
        )
        if count != 1:
            raise ValueError("crate section must contain one version")
        sections[index] = updated
        matches += 1
    if matches != 1:
        raise ValueError("expected exactly one local catwalk3d package")
    return "".join(sections)


def prepare(version: str) -> None:
    """Validate both inputs before updating the crate version and its lock entry."""
    identifier = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    semver = rf"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-{identifier}(?:\.{identifier})*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    if re.fullmatch(semver, version) is None:
        raise ValueError("release version must be a valid semantic version")
    changes = [
        (path, set_version(path.read_text(), header, version))
        for path, header in (
            (Path("Cargo.toml"), "[package]"),
            (Path("Cargo.lock"), "[[package]]"),
        )
    ]
    for path, text in changes:
        path.write_text(text)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: python3 prepare.py VERSION")
    prepare(sys.argv[1])
