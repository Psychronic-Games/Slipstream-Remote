#!/usr/bin/env python3
"""Exercise saved-device groups against an isolated PHP station.

Set SLIPSTREAM_TEST_PHP if php isn't on PATH. Optionally pass --client PATH
for the Rust CLI/vault/cache round trip. No real station or account is used.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path)
    args = parser.parse_args()
    php = os.environ.get('SLIPSTREAM_TEST_PHP') or shutil.which('php')
    if not php:
        raise SystemExit('Set SLIPSTREAM_TEST_PHP to a PHP CLI executable')
    root = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix='slipstream-account-test-') as temp:
        base = Path(temp)
        web = base / 'web'
        shutil.copytree(root / 'server/station', web)
        data = base / 'data'
        (web / 'config.php').write_text('<?php return ["data_dir" => ' + repr(str(data)) + '];\n')
        init = '''require $argv[1];
slip_station_save(['admin_hash'=>password_hash('local-admin-password', PASSWORD_DEFAULT), 'org'=>'Group test', 'public_relays'=>false]);
slip_user_create('owner','Owner','local-owner-password',true);
slip_user_create('other','Other','local-other-password',false);
'''
        subprocess.run([php, '-r', init, str(web / 'lib/slipstream.php')], check=True, capture_output=True)
        apk = b'PK\x03\x04isolated-signed-apk-fixture'
        (web / 'clients/slipstream-android.apk').write_bytes(apk)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        address = f'http://127.0.0.1:{port}'
        with (base / 'php.log').open('w') as log:
            server = subprocess.Popen([php, '-S', f'127.0.0.1:{port}', '-t', str(web)], stdout=log, stderr=log)
            try:
                for _ in range(100):
                    try:
                        with socket.create_connection(('127.0.0.1', port), timeout=.1):
                            break
                    except OSError:
                        if server.poll() is not None:
                            raise RuntimeError('PHP station did not start')
                        time.sleep(.05)
                else:
                    raise RuntimeError('PHP station startup timed out')

                with urllib.request.urlopen(address + '/', timeout=10) as response:
                    page = response.read().decode()
                    assert 'download="Slipstream Remote - Group test.apk"' in page
                with urllib.request.urlopen(address + '/download.php?os=android', timeout=10) as response:
                    assert response.headers['Content-Type'] == 'application/vnd.android.package-archive'
                    assert response.headers['Content-Disposition'] == 'attachment; filename="Slipstream Remote - Group test.apk"'
                    assert response.read() == apk, 'APK signature bytes changed during download'
                print('PHP station: Android download hint, attachment filename, MIME type and unchanged APK passed', flush=True)

                def call(op, token=None, status=200, **fields):
                    body = {'op': op, **fields}
                    if token:
                        body['token'] = token
                    request = urllib.request.Request(address + '/api/', data=json.dumps(body).encode(), headers={'Content-Type': 'application/json'})
                    try:
                        response = urllib.request.urlopen(request, timeout=10)
                    except urllib.error.HTTPError as error:
                        response = error
                    with response:
                        result = json.load(response)
                        assert response.status == status, (op, response.status, result.get('error'))
                        assert result['ok'] == (status == 200), op
                        return result

                owner = call('login', username='owner', password='local-owner-password')['token']
                other = call('login', username='other', password='local-other-password')['token']
                device = {'id': 482913077, 'name': 'Server', 'group': 'Servers', 'notes': 'First line\nSecond line', 'secret': {'n': 'ab' * 24, 'c': 'cd' * 16}}
                state = call('device_save', owner, device=device)
                assert state['devices'][0]['group'] == 'Servers'
                assert state['devices'][0]['notes'] == device['notes']
                assert state['devices'][0]['secret'] == device['secret']
                assert not call('account', other)['devices'], 'Private groups leaked to another account'

                legacy = {k: v for k, v in device.items() if k != 'group'}
                legacy['name'] = 'Legacy edit'
                state = call('device_save', owner, device=legacy)
                assert state['devices'][0]['group'] == 'Servers', 'Old client erased group'
                assert state['devices'][0]['secret'] == device['secret']
                moved = {**device, 'group': '  Lab  '}
                assert call('device_save', owner, device=moved)['devices'][0]['group'] == 'Lab'
                assert call('device_save', owner, device={**device, 'group': ''})['devices'][0]['group'] == ''
                call('device_save', owner, status=400, device={**device, 'group': []})
                long_group = '\u00e9' * 80
                assert call('device_save', owner, device={**device, 'group': long_group})['devices'][0]['group'] == long_group[:64]

                shared = {**device, 'id': 482913078, 'group': 'Shared servers'}
                call('shared_save', other, status=403, device=shared)
                state = call('shared_save', owner, device=shared)
                assert state['shared'][0]['group'] == 'Shared servers'
                assert not state['shared'][0].get('secret'), 'Shared entry retained a password'
                assert call('account', other)['shared'][0]['group'] == 'Shared servers'
                shared.pop('group')
                assert call('shared_save', owner, device=shared)['shared'][0]['group'] == 'Shared servers'
                assert call('shared_save', owner, device={**shared, 'group': ''})['shared'][0]['group'] == ''
                call('account', 'invalid-local-token', status=401)
                print('PHP station: groups, moves, Ungrouped, multiline notes, legacy clients, secret preservation and account/shared permissions passed', flush=True)

                if args.client:
                    client = str(args.client.resolve())
                    profile = base / 'client'
                    env = {**os.environ, 'SLIPSTREAM_HOME': str(profile)}

                    def cli(*arguments):
                        result = subprocess.run([client, *arguments], env=env, capture_output=True, text=True, timeout=30)
                        assert result.returncode == 0, result.stderr[-1500:]
                        return result.stdout

                    cli('set-server', address)
                    # Keep this isolated test off public relays.
                    settings = profile / 'settings.toml'
                    settings.write_text(settings.read_text().replace('use_public_relays = true', 'use_public_relays = false'))
                    cli('sign-in', address, 'owner', '--password', 'local-owner-password')
                    cli('save', '482913079', '--station', address, '--name', 'CLI device', '--group', 'Workstations', '--notes', 'One\nTwo', '--password', 'local-device-password')
                    record = next(d for d in call('account', owner)['devices'] if d['id'] == 482913079)
                    assert record['group'] == 'Workstations' and record['notes'] == 'One\nTwo'
                    secret = record['secret']
                    assert 'local-device-password' not in json.dumps(record)
                    cli('save', '482913079', '--station', address, '--name', 'Renamed')
                    record = next(d for d in call('account', owner)['devices'] if d['id'] == 482913079)
                    assert record['group'] == 'Workstations' and record['secret'] == secret
                    cli('save', '482913079', '--station', address, '--name', 'Renamed', '--group', '')
                    record = next(d for d in call('account', owner)['devices'] if d['id'] == 482913079)
                    assert record['group'] == '' and record['secret'] == secret
                    print('Rust CLI: station synchronization, --group, omitted-group preservation, explicit clearing and encrypted password retention passed', flush=True)
            finally:
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()


if __name__ == '__main__':
    main()
