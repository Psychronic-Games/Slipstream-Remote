#!/usr/bin/env python3
"""Turns an assembled station (scripts/build-station.sh) into GitHub release assets.

    scripts/package-release.py [--station dist/slipstream-station] [--out dist/github-release]

Writes dist/github-release/<version>/ with:

  slipstream-station.zip            the whole station, to upload to a web root by hand
  slipstream-station-web.tar        its PHP site, for stations updating themselves
  slipstream-linux-x86_64, slipstream-windows-x86_64.exe, slipstream-android.apk,
  slipstream-server-linux-x86_64    the apps and relay, also handy as direct downloads
  station-update.json               the signed list stations check before installing
  notes.md                          release notes from CHANGELOG.md

The signature uses the release key on this machine (tools/slipstream-release);
stations trust the keys in their lib/release-keys.txt.
"""
import argparse
import hashlib
import io
import json
import re
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Station files shipped as their own release assets rather than inside the tar.
SEPARATE = {'bin', 'clients'}
KEEP_IN_TAR = {'.htaccess', 'manifest.json'}
EXECUTABLE = {'bin/slipstream-server-linux-x86_64', 'clients/slipstream-linux-x86_64'}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def notes(version: str) -> str:
    text = (ROOT / 'CHANGELOG.md').read_text()
    match = re.search(rf'^## {re.escape(version)} .*?\n(.*?)(?=^## |\Z)', text, re.S | re.M)
    if not match:
        sys.exit(f'CHANGELOG.md has no section for {version}')
    body = match.group(1).strip()
    return (f'{body}\n\n'
            '### Downloads\n\n'
            '- **Run your own station:** download `slipstream-station.zip` and follow '
            '[the station setup guide](https://github.com/Psychronic-Games/Slipstream-Remote/blob/main/docs/STATION-SETUP.md). '
            'Stations already running 0.27 or newer install this release by themselves (or with **Install update** on their admin page).\n'
            '- **Just the app:** `slipstream-windows-x86_64.exe`, `slipstream-linux-x86_64` or `slipstream-android.apk`. '
            'Downloaded from here, the app uses [Slipstream Public](https://public.slipstreamremote.com/); '
            'apps from a station\'s own download page connect to that station.\n'
            '- `station-update.json` is the signed file list stations verify before updating.\n')


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--station', type=Path, default=ROOT / 'dist/slipstream-station')
    parser.add_argument('--out', type=Path, default=ROOT / 'dist/github-release')
    parser.add_argument('--release-tool', type=Path, default=ROOT / 'target/release/slipstream-release')
    args = parser.parse_args()

    station = args.station
    stamp = json.loads((station / 'lib/version.json').read_text())
    version, build, commit = stamp['version'], int(stamp['build']), stamp['commit']
    manifest = json.loads((station / 'clients/manifest.json').read_text())
    if (manifest['version'], int(manifest['build'])) != (version, build):
        sys.exit('clients/manifest.json and lib/version.json disagree: rebuild the station')
    if not (station / 'lib/release-keys.txt').read_text().strip():
        sys.exit('lib/release-keys.txt is empty: rebuild with scripts/build-station.sh')
    out = args.out / version
    out.mkdir(parents=True, exist_ok=True)
    for old in out.iterdir():
        old.unlink()

    files, assets = [], {}
    web = io.BytesIO()
    with tarfile.open(fileobj=web, mode='w', format=tarfile.USTAR_FORMAT) as tar:
        for path in sorted(p for p in station.rglob('*') if p.is_file()):
            rel = path.relative_to(station).as_posix()
            data = path.read_bytes()
            entry = {'path': rel, 'size': len(data), 'sha256': sha256(data)}
            if rel in EXECUTABLE:
                entry['exec'] = True
            if rel.split('/')[0] in SEPARATE and path.name not in KEEP_IN_TAR:
                (out / path.name).write_bytes(data)
                assets[path.name] = {'size': len(data), 'sha256': entry['sha256']}
                entry['from'] = path.name
            else:
                info = tarfile.TarInfo(rel)
                info.size, info.mode, info.mtime = len(data), 0o755 if rel in EXECUTABLE else 0o644, 0
                info.uname = info.gname = ''
                tar.addfile(info, io.BytesIO(data))
                entry['from'], entry['in_tar'] = 'slipstream-station-web.tar', True
            files.append(entry)
    (out / 'slipstream-station-web.tar').write_bytes(web.getvalue())
    assets['slipstream-station-web.tar'] = {'size': len(web.getvalue()), 'sha256': sha256(web.getvalue())}

    payload = {'format': 1, 'version': version, 'build': build, 'commit': commit, 'assets': assets, 'files': files}
    (out / 'payload.json').write_text(json.dumps(payload, indent=1, sort_keys=True))
    subprocess.run([str(args.release_tool), 'station-update', str(out / 'payload.json'), str(out / 'station-update.json')], check=True)
    (out / 'payload.json').unlink()

    with zipfile.ZipFile(out / 'slipstream-station.zip', 'w', zipfile.ZIP_DEFLATED) as z:
        for path in sorted(p for p in station.rglob('*') if p.is_file()):
            rel = path.relative_to(station).as_posix()
            info = zipfile.ZipInfo(f'slipstream-station/{rel}', date_time=(2026, 1, 1, 0, 0, 0))
            info.external_attr = (0o100755 if rel in EXECUTABLE else 0o100644) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, path.read_bytes())
    (out / 'notes.md').write_text(notes(version))
    print(f'{out}: version {version} build {build} ({commit})')
    for path in sorted(out.iterdir()):
        print(f'  {path.name:42} {path.stat().st_size / 1048576:8.1f} MiB')


if __name__ == '__main__':
    main()
