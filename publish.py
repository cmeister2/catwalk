#!/usr/bin/env python3
"""Verify and publish exact crate bytes before semantic-release creates its tag."""

import hashlib
import io
import json
import re
import subprocess
import sys
import tarfile
import time
import tomllib
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


NAME = "catwalk3d"
INDEX_URL = "https://index.crates.io/ca/tw/catwalk3d"
MAX_CRATE_BYTES = 10_000_000
MAX_INDEX_BYTES = 4 * 1024 * 1024
MAX_MANIFEST_BYTES = 1024 * 1024
NETWORK_TIMEOUT = 10
RECHECK_ATTEMPTS = 12
RECHECK_DELAY = 5


def valid_version(version: str) -> bool:
    """Accept semantic versions without allowing shell or path syntax."""
    identifier = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    semver = rf"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-{identifier}(?:\.{identifier})*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    return isinstance(version, str) and re.fullmatch(semver, version) is not None


def verify_workspace(version: str) -> None:
    """Require the selected crate version in the manifest and local lock entry."""
    if not valid_version(version):
        raise ValueError("release version must be a valid semantic version")
    manifest = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))
    package = manifest.get("package", {})
    if package.get("name") != NAME or package.get("version") != version:
        raise ValueError("Cargo.toml does not match the selected crate name/version")
    lock = tomllib.loads(Path("Cargo.lock").read_text(encoding="utf-8"))
    roots = [
        entry
        for entry in lock.get("package", [])
        if entry.get("name") == NAME and "source" not in entry
    ]
    if len(roots) != 1 or roots[0].get("version") != version:
        raise ValueError("Cargo.lock does not match the selected local crate version")


def package_checksum(version: str) -> str:
    """Check the verified archive's size and package identity, then hash its bytes."""
    path = Path("target/package") / f"{NAME}-{version}.crate"
    with path.open("rb") as source:
        contents = source.read(MAX_CRATE_BYTES)
    if not contents or len(contents) >= MAX_CRATE_BYTES:
        raise ValueError("crate archive must be nonempty and smaller than 10 MB")
    with tarfile.open(fileobj=io.BytesIO(contents), mode="r:gz") as archive:
        member = archive.getmember(f"{NAME}-{version}/Cargo.toml")
        if not member.isfile() or member.size > MAX_MANIFEST_BYTES:
            raise ValueError("crate archive has an invalid package manifest")
        with archive.extractfile(member) as source:
            manifest = tomllib.loads(source.read().decode("utf-8"))
        package = manifest.get("package", {})
        if package.get("name") != NAME or package.get("version") != version:
            raise ValueError("crate archive does not match the selected name/version")
    return hashlib.sha256(contents).hexdigest()


def reject_nonfinite(value: str) -> None:
    """Reject nonstandard JSON constants instead of accepting malformed index data."""
    raise ValueError(f"invalid JSON constant {value}")


def registry_checksum(version: str) -> str | None:
    """Read a bounded official sparse index; return only a non-yanked exact version."""
    request = Request(
        INDEX_URL,
        headers={"User-Agent": "catwalk3d-release", "Cache-Control": "no-cache"},
    )
    try:
        with urlopen(request, timeout=NETWORK_TIMEOUT) as response:
            if response.status != 200:
                raise RuntimeError("official crates.io index returned an unexpected status")
            data = response.read(MAX_INDEX_BYTES + 1)
    except HTTPError as error:
        if error.code == 404:
            return None
        raise RuntimeError("official crates.io index request failed") from None
    except (URLError, TimeoutError, OSError):
        raise RuntimeError("official crates.io index could not be read") from None
    if len(data) > MAX_INDEX_BYTES:
        raise RuntimeError("official crates.io index exceeds the release read limit")
    matches = []
    try:
        records = data.decode("utf-8").splitlines()
        if not records:
            raise ValueError("empty index")
        for line in records:
            record = json.loads(line, parse_constant=reject_nonfinite)
            if (
                not isinstance(record, dict)
                or record.get("name") != NAME
                or not valid_version(record.get("vers"))
                or not isinstance(record.get("cksum"), str)
                or re.fullmatch(r"[0-9a-f]{64}", record["cksum"]) is None
                or type(record.get("yanked")) is not bool
            ):
                raise ValueError("invalid index record")
            if record["vers"] == version:
                matches.append(record)
        if len(matches) > 1:
            raise ValueError("duplicate version")
    except (ValueError, UnicodeError, TypeError, RecursionError):
        raise RuntimeError("official crates.io index contains malformed release data") from None
    if not matches:
        return None
    if matches[0]["yanked"]:
        raise RuntimeError("the selected crates.io version is yanked")
    return matches[0]["cksum"]


def require_checksum(actual: str, expected: str) -> None:
    """Prevent an existing version from being mistaken for different crate bytes."""
    if actual != expected:
        raise RuntimeError("the published crate checksum differs from the verified archive")


def stage_archive(version: str, checksum: str) -> None:
    """Stage only registry-confirmed bytes at the fixed GitHub release asset path."""
    source = Path("target/package") / f"{NAME}-{version}.crate"
    with source.open("rb") as archive:
        contents = archive.read(MAX_CRATE_BYTES)
    require_checksum(hashlib.sha256(contents).hexdigest(), checksum)
    destination = Path("target/release-artifacts/catwalk3d.crate")
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(".crate.tmp")
    temporary.write_bytes(contents)
    temporary.replace(destination)


def publish(version: str) -> None:
    """Package and upload before tagging, safely recognizing identical prior uploads."""
    verify_workspace(version)
    subprocess.run(["cargo", "package", "--locked", "--allow-dirty"], check=True)
    checksum = package_checksum(version)
    existing = registry_checksum(version)
    if existing is not None:
        require_checksum(existing, checksum)
        stage_archive(version, checksum)
        print(f"Verified existing {NAME} {version}; identical crate upload retained.")
        return
    subprocess.run(["cargo", "publish", "--locked", "--allow-dirty"], check=True)
    require_checksum(package_checksum(version), checksum)
    for attempt in range(RECHECK_ATTEMPTS):
        existing = registry_checksum(version)
        if existing is not None:
            require_checksum(existing, checksum)
            stage_archive(version, checksum)
            print(f"Verified published {NAME} {version} against the official registry.")
            return
        if attempt + 1 < RECHECK_ATTEMPTS:
            time.sleep(RECHECK_DELAY)
    raise RuntimeError("the uploaded crate was not confirmed within the bounded index wait")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: python3 publish.py VERSION")
    try:
        publish(sys.argv[1])
    except (OSError, ValueError, RuntimeError, tarfile.TarError, KeyError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"Release blocked: {error}") from None
