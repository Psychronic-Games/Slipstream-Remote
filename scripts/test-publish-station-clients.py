#!/usr/bin/env python3
"""Exercise publication ordering and recovery against temporary station files."""
import importlib.util
import hashlib
import io
import json
from email.message import Message
from pathlib import Path
import struct
import sys
import tempfile
import unittest
import urllib.parse
from unittest.mock import patch

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('publisher', Path(__file__).with_name('publish-station-clients.py'))
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='slipstream-publication-test-')
        self.addCleanup(self.temp.cleanup)
        base = Path(self.temp.name)
        self.source = base / 'candidate'
        self.root = base / 'station/public_html'
        self.destination = self.root / 'clients'
        self.destination.mkdir(parents=True)
        (self.root / 'lib').mkdir()
        (self.root / 'index.php').write_text('fixture')
        (self.root / 'lib/slipstream.php').write_text('fixture')
        (self.root.parent / 'tmp').mkdir()
        self.key = Ed25519PrivateKey.generate()
        self.public = self.key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex()
        self.bundle(self.destination, '0.20.0', 98, 'old')
        self.bundle(self.source, '0.21.1', 106, 'new')
        self.before = self.contents()

    def contents(self):
        return {p.name: p.read_bytes() for p in self.destination.iterdir() if p.is_file()}

    def bundle(self, directory, version, build, commit):
        directory.mkdir(exist_ok=True)
        entries = {}
        for platform, name, os_name in [('linux-x86_64', 'slipstream-linux-x86_64', 'linux'),
                                         ('windows-x86_64', 'slipstream-windows-x86_64.exe', 'windows')]:
            blob = f'{platform} {version}'.encode()
            (directory / name).write_bytes(blob)
            checksum = hashlib.sha256(blob).hexdigest()
            message = f'slipstream update v1\n{platform}\n{version}\n{build}\n{len(blob)}\n{checksum}'.encode()
            entries[platform] = {'file': name, 'os': os_name, 'size': len(blob), 'sha256': checksum,
                                 'sig': self.key.sign(message).hex(), 'signed': False}
        (directory / 'slipstream-android.apk').write_bytes(b'APK fixture ' + version.encode())
        (directory / 'manifest.json').write_text(json.dumps({'version': version, 'build': build,
                                                          'commit': commit, 'signer': self.public,
                                                          'platforms': entries}))

    def publish(self):
        return publisher.publish(self.source, self.root, self.public, '0.20.0/98/old')

    def test_manifest_is_last_and_backup_is_exact(self):
        actual_replace = publisher.os.replace
        replacements = []

        def observe(source, destination):
            replacements.append(Path(destination).name)
            if Path(destination).name == 'manifest.json':
                for name in self.before:
                    if name != 'manifest.json':
                        self.assertEqual((self.destination / name).read_bytes(), (self.source / name).read_bytes())
            return actual_replace(source, destination)

        with patch.object(publisher.os, 'replace', side_effect=observe):
            backup = self.publish()
        self.assertEqual(replacements[-1], 'manifest.json')
        self.assertEqual(self.contents(), {p.name: p.read_bytes() for p in self.source.iterdir()})
        for name, blob in self.before.items():
            self.assertEqual((backup / name).read_bytes(), blob)
        self.assertFalse(backup.resolve().is_relative_to(self.root.resolve()))

    def test_failed_publication_restores_prior_release(self):
        actual_replace = publisher.os.replace
        failed = False

        def fail_once(source, destination):
            nonlocal failed
            if Path(destination).suffix == '.exe' and not failed:
                failed = True
                raise OSError('fixture interrupted upload')
            return actual_replace(source, destination)

        with patch.object(publisher.os, 'replace', side_effect=fail_once):
            with self.assertRaisesRegex(OSError, 'interrupted upload'):
                self.publish()
        self.assertEqual(self.contents(), self.before)
        self.assertFalse(any(self.destination.glob('.slipstream-stage-*')))

    def test_failed_manifest_verification_restores_prior_release(self):
        actual_check = publisher.check_files
        failed = False

        def fail_once(directory, hashes):
            nonlocal failed
            if (directory == self.destination and not failed and
                    json.loads((directory / 'manifest.json').read_text())['version'] == '0.21.1'):
                failed = True
                raise OSError('fixture readback failure after manifest replacement')
            return actual_check(directory, hashes)

        with patch.object(publisher, 'check_files', side_effect=fail_once):
            with self.assertRaisesRegex(OSError, 'readback failure'):
                self.publish()
        self.assertEqual(self.contents(), self.before)

    def test_tampering_and_stale_expectation_do_not_change_station(self):
        path = self.source / 'slipstream-linux-x86_64'
        original = path.read_bytes()
        path.write_bytes(original.replace(b'linux', b'LINUX'))
        with self.assertRaisesRegex(ValueError, 'Checksum mismatch'):
            self.publish()
        self.assertEqual(self.contents(), self.before)
        path.write_bytes(original)
        with self.assertRaisesRegex(ValueError, 'Expected'):
            publisher.publish(self.source, self.root, self.public, '0.19.1/95/stale')
        self.assertEqual(self.contents(), self.before)
        self.assertEqual(list((self.root.parent / 'tmp').iterdir()), [])

    def test_bad_signature_and_missing_mount_do_not_change_station(self):
        path = self.source / 'manifest.json'
        manifest = json.loads(path.read_text())
        manifest['platforms']['linux-x86_64']['sig'] = '00' * 64
        path.write_text(json.dumps(manifest))
        with self.assertRaises(InvalidSignature):
            self.publish()
        self.assertEqual(self.contents(), self.before)
        self.bundle(self.source, '0.21.1', 106, 'new')
        missing = self.root.parent / 'not-mounted/public_html'
        with self.assertRaisesRegex(ValueError, 'reconnect the mount'):
            publisher.publish(self.source, missing, self.public, '0.20.0/98/old')
        self.assertFalse(missing.exists())

    def test_live_offers_downloads_and_tampered_provisioning(self):
        manifest = json.loads((self.source / 'manifest.json').read_bytes())
        settings = self.source.parent / 'settings.toml'
        settings.write_text('[server]\nurl = "https://fixture.invalid"\nkey = "fixture-key"\n')
        bad_trailer = False

        class FixtureOpener:
            def open(inner, request, timeout):
                headers = Message()
                if request.full_url.endswith('/api/'):
                    body = json.loads(request.data)
                    self.assertEqual(body['key'], 'fixture-key')
                    entry = manifest['platforms'][body['platform']]
                    offered = {key: manifest[key] for key in ('version', 'build', 'commit')}
                    offered.update({key: entry[key] for key in ('size', 'sha256', 'sig', 'os')})
                    blob = json.dumps({'ok': True, 'update': offered}).encode()
                else:
                    self.assertEqual(request.get_header('X-slipstream-key'), 'fixture-key')
                    os_name = urllib.parse.parse_qs(urllib.parse.urlsplit(request.full_url).query)['os'][0]
                    if os_name == 'android':
                        blob = (self.source / 'slipstream-android.apk').read_bytes()
                        headers['Content-Type'] = 'application/vnd.android.package-archive'
                        headers['Content-Disposition'] = 'attachment; filename="fixture.apk"'
                    else:
                        entry = next(e for e in manifest['platforms'].values() if e['os'] == os_name)
                        config = json.dumps({'server': 'fixture.invalid', 'key': 'wrong' if bad_trailer else 'fixture-key'}).encode()
                        blob = (self.source / entry['file']).read_bytes() + config + struct.pack('<I', len(config)) + b'SLIPCFG1'
                response = io.BytesIO(blob)
                response.headers = headers
                return response

        with patch.object(publisher.urllib.request, 'build_opener', return_value=FixtureOpener()):
            publisher.live_check(self.source, self.public, settings)
            bad_trailer = True
            with self.assertRaisesRegex(ValueError, 'Wrong provisioning key'):
                publisher.live_check(self.source, self.public, settings)


if __name__ == '__main__':
    unittest.main()
