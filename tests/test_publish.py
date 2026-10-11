"""Check release uploads with temporary archives and fully mocked Cargo/network IO."""

import hashlib
import importlib.util
import io
import json
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError, URLError


SCRIPT = Path(__file__).resolve().parents[1] / "publish.py"
SPEC = importlib.util.spec_from_file_location("release_publish", SCRIPT)
PUBLISH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PUBLISH)


class Response(io.BytesIO):
    """A bounded byte response with the status of a successful official index read."""

    status = 200


class PublishTests(unittest.TestCase):
    """Exercise exact package identity, index failures and idempotent publication."""

    def setUp(self):
        """Create a release checkout and prevent every real Cargo or network call."""
        self.directory = tempfile.TemporaryDirectory()
        self.previous = Path.cwd()
        os.chdir(self.directory.name)
        Path("Cargo.toml").write_text('[package]\nname = "catwalk3d"\nversion = "1.0.0"\n')
        Path("Cargo.lock").write_text('version = 4\n[[package]]\nname = "catwalk3d"\nversion = "1.0.0"\n')
        self.commands = []
        self.archive_version = "1.0.0"
        self.archive_override = None
        self.package_error = False
        self.publish_error = False
        self.change_after_upload = False
        self.run_patch = patch.object(PUBLISH.subprocess, "run", side_effect=self.cargo)
        self.run_patch.start()
        self.network_patch = patch.object(
            PUBLISH, "urlopen", side_effect=AssertionError("unexpected live network")
        )
        self.network = self.network_patch.start()
        self.sleep_patch = patch.object(PUBLISH.time, "sleep")
        self.sleep = self.sleep_patch.start()
        self.print_patch = patch("builtins.print")
        self.print_patch.start()

    def tearDown(self):
        """Remove only temporary checkout data and restore all mocked boundaries."""
        self.print_patch.stop()
        self.sleep_patch.stop()
        self.network_patch.stop()
        self.run_patch.stop()
        os.chdir(self.previous)
        self.directory.cleanup()

    def archive(self, extra=False):
        """Write a real gzip/tar crate carrying an explicitly selected package version."""
        path = Path("target/package/catwalk3d-1.0.0.crate")
        path.parent.mkdir(parents=True, exist_ok=True)
        if self.archive_override is not None:
            path.write_bytes(self.archive_override)
            return
        with tarfile.open(path, "w:gz") as archive:
            data = f'[package]\nname = "catwalk3d"\nversion = "{self.archive_version}"\n'.encode()
            member = tarfile.TarInfo("catwalk3d-1.0.0/Cargo.toml")
            member.size = len(data)
            archive.addfile(member, io.BytesIO(data))
            if extra:
                member = tarfile.TarInfo("catwalk3d-1.0.0/README.md")
                member.size = 7
                archive.addfile(member, io.BytesIO(b"changed"))

    def cargo(self, command, *, check):
        """Simulate package output and upload failures without launching a subprocess."""
        self.assertTrue(check)
        self.commands.append(command)
        self.assertEqual(command[2:], ["--locked", "--allow-dirty"])
        if command[:2] == ["cargo", "package"]:
            if self.package_error:
                raise subprocess.CalledProcessError(1, command)
            self.archive()
        elif command[:2] == ["cargo", "publish"]:
            if self.publish_error:
                raise subprocess.CalledProcessError(1, command)
            if self.change_after_upload:
                self.archive(extra=True)
        else:
            self.fail(f"unexpected command: {command}")
        return subprocess.CompletedProcess(command, 0)

    def record(self, checksum="0" * 64, version="1.0.0", yanked=False):
        """Serialize the actual sparse-index fields used to identify a published crate."""
        return {"name": "catwalk3d", "vers": version, "cksum": checksum, "yanked": yanked}

    def registry(self, *states):
        """Supply missing, matching or conflicting official-index responses in order."""
        states = list(states)

        def request(request, *, timeout):
            self.assertEqual(request.full_url, PUBLISH.INDEX_URL)
            self.assertEqual(timeout, PUBLISH.NETWORK_TIMEOUT)
            self.assertEqual(
                dict(request.header_items()),
                {"User-agent": "catwalk3d-release", "Cache-control": "no-cache"},
            )
            state = states.pop(0)
            if state is None:
                raise HTTPError(PUBLISH.INDEX_URL, 404, "missing", None, None)
            checksum = hashlib.sha256(Path("target/package/catwalk3d-1.0.0.crate").read_bytes()).hexdigest()
            record = self.record(checksum if state != "different" else "0" * 64, yanked=state == "yanked")
            return Response(json.dumps(record).encode() + b"\n")

        self.network.side_effect = request

    def assert_staged_current_archive(self):
        """The GitHub asset contains only the exact newly verified package bytes."""
        self.assertEqual(
            Path("target/release-artifacts/catwalk3d.crate").read_bytes(),
            Path("target/package/catwalk3d-1.0.0.crate").read_bytes(),
        )
        self.assertFalse(Path("target/release-artifacts/catwalk3d.crate.tmp").exists())

    def assert_no_staged_archive(self):
        """Failed publication cannot create a GitHub upload asset."""
        self.assertFalse(Path("target/release-artifacts/catwalk3d.crate").exists())

    def test_identical_existing_version_skips_upload(self):
        """Only an exact official checksum skips an otherwise verified Cargo upload."""
        self.registry("match")
        PUBLISH.publish("1.0.0")
        self.assertEqual(self.commands, [["cargo", "package", "--locked", "--allow-dirty"]])
        self.sleep.assert_not_called()
        self.assert_staged_current_archive()

    def test_absent_version_uploads_once_and_confirms_index(self):
        """The first upload follows local verification and waits for exact registry bytes."""
        self.registry(None, None, "match")
        PUBLISH.publish("1.0.0")
        self.assertEqual([command[1] for command in self.commands], ["package", "publish"])
        self.assertEqual(self.network.call_count, 3)
        self.sleep.assert_called_once_with(PUBLISH.RECHECK_DELAY)
        self.assert_staged_current_archive()

    def test_staging_replaces_only_the_fixed_asset_with_current_bytes(self):
        """Old package versions and a stale staged asset cannot enter the new release."""
        stale = Path("target/package/catwalk3d-0.1.0.crate")
        stale.parent.mkdir(parents=True)
        stale.write_bytes(b"old package")
        staged = Path("target/release-artifacts/catwalk3d.crate")
        staged.parent.mkdir(parents=True)
        staged.write_bytes(b"old staged asset")
        self.registry("match")
        PUBLISH.publish("1.0.0")
        self.assert_staged_current_archive()
        self.assertEqual(stale.read_bytes(), b"old package")

    def test_conflicting_or_yanked_version_never_uploads(self):
        """A reused version cannot overwrite different bytes or revive a yanked release."""
        for state in ("different", "yanked"):
            with self.subTest(state=state):
                self.commands.clear()
                self.registry(state)
                with self.assertRaises(RuntimeError):
                    PUBLISH.publish("1.0.0")
                self.assertEqual([command[1] for command in self.commands], ["package"])
                self.assert_no_staged_archive()

    def test_package_failure_stops_before_registry_and_upload(self):
        """A failed Cargo verification cannot reach publication or consume a tag."""
        self.package_error = True
        with self.assertRaises(subprocess.CalledProcessError):
            PUBLISH.publish("1.0.0")
        self.network.assert_not_called()
        self.assert_no_staged_archive()

    def test_upload_failure_propagates_before_tagging(self):
        """The preparation hook fails immediately when the first Cargo upload fails."""
        self.publish_error = True
        self.registry(None)
        with self.assertRaises(subprocess.CalledProcessError):
            PUBLISH.publish("1.0.0")
        self.assertEqual(self.network.call_count, 1)
        self.sleep.assert_not_called()
        self.assert_no_staged_archive()

    def test_confirmation_has_a_bounded_wait(self):
        """Index propagation cannot cause an unbounded release job."""
        self.registry(None, None, None, None)
        with patch.object(PUBLISH, "RECHECK_ATTEMPTS", 3):
            with self.assertRaisesRegex(RuntimeError, "bounded index wait"):
                PUBLISH.publish("1.0.0")
        self.assertEqual(self.network.call_count, 4)
        self.assertEqual(self.sleep.call_count, 2)
        self.assert_no_staged_archive()

    def test_post_upload_checksum_conflicts_fail(self):
        """A successful command alone does not establish publication of the verified bytes."""
        self.registry(None, "different")
        with self.assertRaisesRegex(RuntimeError, "checksum differs"):
            PUBLISH.publish("1.0.0")
        self.assertEqual([command[1] for command in self.commands], ["package", "publish"])
        self.assert_no_staged_archive()

    def test_archive_changed_by_upload_is_rejected(self):
        """Repackaging during upload must retain the initially verified archive bytes."""
        self.change_after_upload = True
        self.registry(None)
        with self.assertRaisesRegex(RuntimeError, "checksum differs"):
            PUBLISH.publish("1.0.0")
        self.assertEqual(self.network.call_count, 1)
        self.assert_no_staged_archive()

    def test_archive_identity_and_size_are_checked_before_network(self):
        """Stale package metadata and archives at the size ceiling cannot be uploaded."""
        self.archive_version = "2.0.0"
        with self.assertRaisesRegex(ValueError, "archive does not match"):
            PUBLISH.publish("1.0.0")
        self.archive_override = b"x" * 100
        with patch.object(PUBLISH, "MAX_CRATE_BYTES", 100):
            with self.assertRaisesRegex(ValueError, "smaller than 10 MB"):
                PUBLISH.publish("1.0.0")
        self.network.assert_not_called()
        self.assert_no_staged_archive()

    def test_workspace_and_version_mismatch_never_run_cargo(self):
        """Both stamped inputs must agree before packaging starts."""
        for version in ("", "1.2", "1.0.0;echo", "01.0.0", "2.0.0"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                PUBLISH.publish(version)
        Path("Cargo.lock").write_text('version = 4\n[[package]]\nname = "catwalk3d"\nversion = "0.1.0"\n')
        with self.assertRaisesRegex(ValueError, "Cargo.lock"):
            PUBLISH.publish("1.0.0")
        self.assertEqual(self.commands, [])
        self.network.assert_not_called()
        self.assert_no_staged_archive()

    def test_only_404_means_missing_crate(self):
        """Permissions, rate limits and server failures cannot be treated as new versions."""
        self.network.side_effect = HTTPError(PUBLISH.INDEX_URL, 404, "missing", None, None)
        self.assertIsNone(PUBLISH.registry_checksum("1.0.0"))
        for code in (401, 403, 429, 500):
            self.network.side_effect = HTTPError(PUBLISH.INDEX_URL, code, "failure", None, None)
            with self.subTest(code=code), self.assertRaises(RuntimeError):
                PUBLISH.registry_checksum("1.0.0")
        for error in (URLError("unreachable"), TimeoutError("timeout")):
            self.network.side_effect = error
            with self.subTest(error=type(error).__name__), self.assertRaises(RuntimeError):
                PUBLISH.registry_checksum("1.0.0")

    def test_index_read_is_bounded(self):
        """Oversized successful responses fail before attempting JSON parsing."""
        self.network.side_effect = None
        self.network.return_value = Response(b"x" * 33)
        with patch.object(PUBLISH, "MAX_INDEX_BYTES", 32):
            with self.assertRaisesRegex(RuntimeError, "read limit"):
                PUBLISH.registry_checksum("1.0.0")

    def test_malformed_index_and_duplicate_versions_fail(self):
        """Incomplete, wrong-identity and nonstandard records cannot authorize retries."""
        valid = self.record()
        invalid = [b"", b"\xff", b"{", b'{"value":NaN}']
        for field, value in (("name", "other"), ("vers", "1.2"), ("cksum", "short"), ("yanked", 0)):
            record = {**valid, field: value}
            invalid.append(json.dumps(record).encode())
        invalid.append(json.dumps(valid).encode() + b"\n" + json.dumps(valid).encode())
        for data in invalid:
            self.network.side_effect = None
            self.network.return_value = Response(data)
            with self.subTest(data=data[:60]), self.assertRaises(RuntimeError):
                PUBLISH.registry_checksum("1.0.0")

    def test_other_valid_versions_do_not_authorize_skipping_upload(self):
        """Only the exact selected version may supply a publication checksum."""
        self.network.side_effect = None
        self.network.return_value = Response(json.dumps(self.record(version="0.9.0")).encode())
        self.assertIsNone(PUBLISH.registry_checksum("1.0.0"))


if __name__ == "__main__":
    unittest.main()
