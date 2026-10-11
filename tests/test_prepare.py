"""Exercise release version preparation without publishing or changing the checkout."""

import importlib.util
import os
import tempfile
import tomllib
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "prepare.py"
SPEC = importlib.util.spec_from_file_location("release_prepare", SCRIPT)
PREPARE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PREPARE)


class PrepareTests(unittest.TestCase):
    """Verify local version updates, dependency preservation and invalid input failures."""

    def setUp(self):
        """Create an independent package and lockfile with misleading dependency versions."""
        self.directory = tempfile.TemporaryDirectory()
        self.previous = Path.cwd()
        os.chdir(self.directory.name)
        self.manifest = '[package]\nname = "catwalk3d"\nversion = "0.1.0"\n\n[dependencies]\nother = "0.1.0"\n'
        self.lock = 'version = 4\n\n[[package]]\nname = "before"\nversion = "0.1.0"\nsource = "registry+https://example.invalid/index"\n\n[[package]]\nname = "catwalk3d"\nversion = "0.1.0"\ndependencies = ["before"]\n\n[[package]]\nname = "other"\nversion = "0.1.0"\n'
        Path("Cargo.toml").write_text(self.manifest)
        Path("Cargo.lock").write_text(self.lock)

    def tearDown(self):
        """Restore the process directory and remove only the temporary package."""
        os.chdir(self.previous)
        self.directory.cleanup()

    def test_updates_local_package_and_lock_entry_only(self):
        """A real semver updates the local package without touching registry versions."""
        for version in ("1.2.3", "2.0.0-rc.1+build.7"):
            PREPARE.prepare(version)
            manifest = tomllib.loads(Path("Cargo.toml").read_text())
            locked = tomllib.loads(Path("Cargo.lock").read_text())
            self.assertEqual(manifest["package"]["version"], version)
            self.assertEqual(manifest["dependencies"]["other"], "0.1.0")
            self.assertEqual([p["version"] for p in locked["package"]], ["0.1.0", version, "0.1.0"])
            self.assertEqual(locked["package"][1]["dependencies"], ["before"])

    def test_rejects_invalid_versions_without_writing(self):
        """Malformed or shell-shaped versions never enter either TOML file."""
        for version in ("", "1.2", "01.2.3", "1.2.3-01", "1.2.3;echo", "1.2.3\n", "1.2.3-", '1.2.3"'):
            with self.subTest(version=version), self.assertRaises(ValueError):
                PREPARE.prepare(version)
            self.assertEqual(Path("Cargo.toml").read_text(), self.manifest)
            self.assertEqual(Path("Cargo.lock").read_text(), self.lock)

    def test_invalid_lockfile_leaves_manifest_untouched(self):
        """Both package entries must be valid before the manifest is changed."""
        for lock in ("version = 4\n", self.lock.replace('name = "catwalk3d"', 'name = "wrong"')):
            Path("Cargo.lock").write_text(lock)
            with self.assertRaises(ValueError):
                PREPARE.prepare("1.0.0")
            self.assertEqual(Path("Cargo.toml").read_text(), self.manifest)
            self.assertEqual(Path("Cargo.lock").read_text(), lock)


if __name__ == "__main__":
    unittest.main()
