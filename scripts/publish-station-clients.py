#!/usr/bin/env python3
"""Verify, publish, and check an existing station client release.

Requires Python 3.11+ and cryptography. Does not build apps, change PHP, or
restart the relay. See docs/BUILDING.md for examples and rollback instructions.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import tempfile
import tomllib
import urllib.parse
import urllib.request

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def release_id(manifest):
    return f"{manifest['version']}/{manifest['build']}/{manifest['commit']}"


def verify(clients, trusted_key):
    require(not (clients / 'manifest.json').is_symlink(), 'Manifest must be a regular file')
    manifest = json.loads((clients / 'manifest.json').read_bytes())
    require(manifest['signer'] == trusted_key, 'Manifest uses a different release key')
    key = Ed25519PublicKey.from_public_bytes(bytes.fromhex(trusted_key))
    require(bool(manifest['platforms']), 'Manifest has no platforms')
    files = {'manifest.json': digest(clients / 'manifest.json')}
    for platform, entry in manifest['platforms'].items():
        name = entry['file']
        require(name == Path(name).name and name not in ('.', '..', 'manifest.json'),
                'Invalid manifest filename')
        path = clients / name
        require(not path.is_symlink() and path.is_file(), f'Missing regular file: {name}')
        require(path.stat().st_size == entry['size'], f'Size mismatch: {name}')
        require(digest(path) == entry['sha256'], f'Checksum mismatch: {name}')
        message = (f"slipstream update v1\n{platform}\n{manifest['version']}\n"
                   f"{manifest['build']}\n{entry['size']}\n{entry['sha256'].lower()}")
        key.verify(bytes.fromhex(entry['sig']), message.encode())
        files[name] = entry['sha256']
    apk = clients / 'slipstream-android.apk'
    if apk.is_file():
        require(not apk.is_symlink(), 'APK must be a regular file')
        files[apk.name] = digest(apk)
    return manifest, files


def check_files(directory, hashes):
    for name, expected in hashes.items():
        require(digest(directory / name) == expected, f'Checksum mismatch: {name}')


def copy_with_owner(source, target, metadata):
    shutil.copyfile(source, target)
    current = target.stat()
    if (current.st_uid, current.st_gid) != (metadata.st_uid, metadata.st_gid):
        os.chown(target, metadata.st_uid, metadata.st_gid)
    os.chmod(target, metadata.st_mode & 0o777)


def publish(clients, root, trusted_key, expected):
    candidate, hashes = verify(clients, trusted_key)
    require((root / 'index.php').is_file() and (root / 'lib/slipstream.php').is_file(),
            'Station web root is unavailable; reconnect the mount before publishing')
    destination = root / 'clients'
    previous, old_hashes = verify(destination, trusted_key)
    require(release_id(previous) == expected,
            f'Expected {expected}, found {release_id(previous)}; no files changed')
    require((tuple(map(int, candidate['version'].split('.'))), candidate['build']) >
            (tuple(map(int, previous['version'].split('.'))), previous['build']),
            'Candidate must be newer than the deployed release')
    require(set(previous['platforms']) <= set(candidate['platforms']),
            'Candidate omits a deployed platform; review before replacing its manifest')
    backup_parent = root.parent / 'tmp'
    require(backup_parent.is_dir(), f'Private backup directory unavailable: {backup_parent}')
    require(not backup_parent.resolve().is_relative_to(root.resolve()),
            'Backups must be outside the public web root')
    backup = Path(tempfile.mkdtemp(prefix=f"slipstream-rollback-{previous['version']}-to-{candidate['version']}-",
                                   dir=backup_parent))
    metadata = {}
    missing = []
    for name in hashes:
        path = destination / name
        require(not path.is_symlink(), f'Destination is a symlink: {name}')
        if path.exists():
            metadata[name] = path.stat()
            shutil.copy2(path, backup / name)
        else:
            metadata[name] = (destination / 'manifest.json').stat()
            missing.append(name)
    backup_hashes = {p.name: digest(p) for p in backup.iterdir()}
    (backup / 'rollback.json').write_text(json.dumps({'hashes': backup_hashes, 'remove': missing}, indent=2) + '\n')
    check_files(backup, {name: value for name, value in old_hashes.items() if name in hashes})
    print(f'Rollback backup: {backup}', flush=True)
    replaced = []
    with tempfile.TemporaryDirectory(prefix='.slipstream-stage-', dir=destination) as temporary:
        stage = Path(temporary)
        for name in hashes:
            copy_with_owner(clients / name, stage / name, metadata[name])
        check_files(stage, hashes)
        # Refuse stale state before the first replacement; manifest is always last.
        check_files(destination, old_hashes)
        try:
            for name in hashes:
                if name != 'manifest.json':
                    os.replace(stage / name, destination / name)
                    replaced.append(name)
                    check_files(destination, {name: hashes[name]})
            os.replace(stage / 'manifest.json', destination / 'manifest.json')
            replaced.append('manifest.json')
            check_files(destination, hashes)
        except BaseException:
            # Recover ordinary copy/rename failures. Power loss or a disconnected
            # mount may require restoring the printed backup manually.
            for name in replaced:
                if name in missing:
                    (destination / name).unlink(missing_ok=True)
                else:
                    copy_with_owner(backup / name, stage / name, metadata[name])
                    os.replace(stage / name, destination / name)
            check_files(destination, old_hashes)
            raise
    print(f'Published {release_id(candidate)}; client hashes verified, manifest published last')
    return backup


class NoRedirect(urllib.request.HTTPRedirectHandler):
    # Never forward the station key to a redirect destination.
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def live_check(clients, trusted_key, settings_path, address=None):
    manifest, hashes = verify(clients, trusted_key)
    settings = tomllib.loads(settings_path.read_text())['server']
    address = (address or settings['url']).rstrip('/') + '/'
    if '://' not in address:
        address = 'https://' + address
    require(urllib.parse.urlsplit(address).scheme == 'https', 'Live checks require HTTPS')
    key = settings.get('key', '')
    opener = urllib.request.build_opener(NoRedirect)
    for platform, entry in manifest['platforms'].items():
        body = json.dumps({'op': 'update_check', 'platform': platform, 'key': key}).encode()
        request = urllib.request.Request(address + 'api/', data=body,
                                         headers={'Content-Type': 'application/json'})
        with opener.open(request, timeout=30) as response:
            reply = json.load(response)
        require(reply.get('ok') and reply.get('update'), f'No update offered for {platform}')
        offered = reply['update']
        for field in ('version', 'build', 'commit'):
            require(offered[field] == manifest[field], f'Live offer differs: {platform} {field}')
        for field in ('size', 'sha256', 'sig', 'os'):
            require(offered[field] == entry[field], f'Live offer differs: {platform} {field}')
        print(f'{platform}: authenticated update offer matches signed release')
    downloads = {entry['os']: entry for entry in manifest['platforms'].values()}
    if 'slipstream-android.apk' in hashes:
        downloads['android'] = {'file': 'slipstream-android.apk', 'signed': True}
    with tempfile.TemporaryDirectory(prefix='slipstream-live-download-') as temporary:
        for os_name, entry in downloads.items():
            request = urllib.request.Request(address + 'download.php?' + urllib.parse.urlencode({'os': os_name}),
                                             headers={'X-Slipstream-Key': key})
            path = Path(temporary) / entry['file']
            size = (clients / entry['file']).stat().st_size
            with opener.open(request, timeout=60) as response, path.open('wb') as out:
                headers = response.headers
                remaining = size + 65536 + 12
                while remaining:
                    chunk = response.read(min(1024 * 1024, remaining))
                    if not chunk:
                        break
                    out.write(chunk)
                    remaining -= len(chunk)
                require(not response.read(1), f'Oversized {os_name} download')
            with path.open('rb') as stream:
                data = stream.read(size)
                trailer = stream.read()
            require(len(data) == size and hashlib.sha256(data).hexdigest() == hashes[entry['file']],
                    f'Live download checksum mismatch: {os_name}')
            if entry.get('signed') or os_name == 'macos':
                require(not trailer, f'Signed {os_name} download was modified')
            else:
                require(len(trailer) >= 12 and trailer[-8:] == b'SLIPCFG1', 'Missing provisioning trailer')
                require(struct.unpack('<I', trailer[-12:-8])[0] == len(trailer) - 12, 'Invalid trailer length')
                config = json.loads(trailer[:-12])
                server = str(config['server']).removeprefix('https://').rstrip('/')
                require(server == address.removeprefix('https://').rstrip('/'), 'Wrong provisioning server')
                require(config.get('key', '') == key, 'Wrong provisioning key')
            if os_name == 'android':
                require(headers.get_content_type() == 'application/vnd.android.package-archive', 'Wrong APK MIME type')
                require(headers.get_filename() and headers.get_filename().endswith('.apk'), 'Missing APK filename')
            print(f'{os_name}: full HTTPS download verified')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--clients', type=Path, default=Path(__file__).resolve().parent.parent / 'dist/slipstream-station/clients')
    parser.add_argument('--trusted-key', required=True, help='Previously trusted Ed25519 public key (hex)')
    actions = parser.add_subparsers(dest='action', required=True)
    actions.add_parser('verify', help='Check local desktop hashes/signatures and record APK checksum')
    upload = actions.add_parser('publish', help='Back up and publish clients on a mounted station')
    upload.add_argument('--root', required=True, type=Path)
    upload.add_argument('--expect', required=True, help='Expected deployed version/build/commit')
    live = actions.add_parser('check-live', help='Check authenticated offers and full HTTPS downloads')
    live.add_argument('--settings', type=Path, default=Path.home() / '.config/slipstream/settings.toml')
    live.add_argument('--url', help='Station base HTTPS URL; access key is read from --settings')
    args = parser.parse_args()
    if args.action == 'publish':
        publish(args.clients, args.root, args.trusted_key, args.expect)
    elif args.action == 'check-live':
        live_check(args.clients, args.trusted_key, args.settings, args.url)
    else:
        manifest, hashes = verify(args.clients, args.trusted_key)
        print(f'Verified desktop release {release_id(manifest)}')
        for name, checksum in hashes.items():
            print(f'{checksum}  {name}')


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        # Do not include HTTP bodies, URLs with credentials, or station settings.
        raise SystemExit(f'{type(error).__name__}: {error}') from None
