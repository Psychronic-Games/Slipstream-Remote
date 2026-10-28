#!/usr/bin/env python3
"""Exercise saved devices, private enrollment and inventory on a local PHP station.

Set SLIPSTREAM_TEST_PHP if php isn't on PATH. Optionally pass --client PATH
for the Rust CLI/vault/cache round trip. No real station or account is used.
"""
import argparse
import http.cookiejar
import json
import os
from pathlib import Path
import shutil
import re
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import urllib.parse


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
slip_station_save(['admin_hash'=>password_hash('local-admin-password', PASSWORD_DEFAULT), 'org'=>'Group test', 'public_relays'=>false, 'auto_update'=>false]);
slip_update_save(['checked'=>time()]);
slip_user_create('owner','Owner','local-owner-password',true);
slip_user_create('other','Other','local-other-password',false);
slip_user_create('assigned','Assigned technician','local-assigned-password',false);
slip_user_create('batch-new','New batch recipient','local-batch-password',false);
slip_user_create('full-tech','Full recipient','local-full-password',false);
slip_user_update('full-tech',function($account) { $account['devices']=array_map(fn($i)=>['id'=>100000000+$i],range(0,SLIP_MAX_DEVICES-1)); return $account; });
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
                assert call('group_save', owner, group='  Empty lab  ')['groups'] == ['Empty lab']
                assert call('group_save', owner, group='Empty lab')['groups'] == ['Empty lab'], 'Duplicate group'
                assert not call('account', other)['groups'], 'Private empty groups leaked'
                call('group_save', owner, status=400, group=[])
                call('group_save', owner, status=400, group='   ')
                call('group_save', 'invalid-local-token', status=401, group='Unauthorized')
                device = {'id': 482913077, 'name': 'Server', 'group': 'Servers', 'notes': 'First line\nSecond line', 'secret': {'n': 'ab' * 24, 'c': 'cd' * 16}}
                state = call('device_save', owner, device=device)
                assert state['devices'][0]['group'] == 'Servers'
                assert state['devices'][0]['notes'] == device['notes']
                assert state['devices'][0]['secret'] == device['secret']
                assert state['groups'] == ['Empty lab'], 'Device save erased empty groups'
                call('group_save', owner, group='Servers')
                call('group_delete', owner, status=400, group='Servers')
                assert not call('account', other)['devices'], 'Private groups leaked to another account'

                legacy = {k: v for k, v in device.items() if k != 'group'}
                legacy['name'] = 'Legacy edit'
                state = call('device_save', owner, device=legacy)
                assert state['devices'][0]['group'] == 'Servers', 'Old client erased group'
                assert state['devices'][0]['secret'] == device['secret']
                assert 'Empty lab' in state['groups'], 'Legacy client erased empty groups'
                moved = {**device, 'group': '  Lab  '}
                assert call('device_save', owner, device=moved)['devices'][0]['group'] == 'Lab'
                assert call('device_save', owner, device={**device, 'group': ''})['devices'][0]['group'] == ''
                assert call('group_delete', owner, group='Servers')['groups'] == ['Empty lab']
                assert call('group_delete', owner, group='Empty lab')['groups'] == []
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

                def settings(**changes):
                    subprocess.run([php, '-r', 'require $argv[1]; slip_station_save(json_decode($argv[2], true));',
                                    str(web / 'lib/slipstream.php'), json.dumps(changes)], check=True, capture_output=True)

                def signed(op, seed='01', **fields):
                    payload = {'op': op, 'time': int(time.time()), **fields}
                    sign = '''$input=json_decode(stream_get_contents(STDIN),true);
$keys=sodium_crypto_sign_seed_keypair(hex2bin($input['seed']));
$p=$input['payload']; $p['endpoint']=bin2hex(sodium_crypto_sign_publickey($keys));
if (isset($p['addr'])) $p['addr']['id']=$p['endpoint'];
$payload=json_encode($p,JSON_UNESCAPED_SLASHES);
echo json_encode(['payload'=>$payload,'sig'=>bin2hex(sodium_crypto_sign_detached("slipstream-directory-v2\\n".$payload,sodium_crypto_sign_secretkey($keys)))]);'''
                    result = subprocess.run([php, '-r', sign], input=json.dumps({'seed': seed * 32, 'payload': payload}),
                                            text=True, capture_output=True, check=True)
                    return json.loads(result.stdout)

                info = {'name': 'OFFICE-PC', 'os': 'Linux', 'app_version': 'fixture'}
                registration = signed('register', addr={'id': '', 'addrs': []}, info=info)
                summary = {'computer_name': 'OFFICE-PC', 'model': 'Fixture <Model>', 'serial_number': 'SERIAL-ABC123',
                           'os_version': 'Fixture Linux', 'architecture': 'x86_64', 'processor': 'Fixture CPU',
                           'memory_bytes': 16 * 1024**3, 'password': 'must-never-be-stored'}
                report = signed('inventory', inventory=summary)
                assert call('hello')['inventory'] is False
                public_id = call('register', **registration)['id']
                assert not any(d['id'] == public_id for d in call('account', owner)['shared']), 'Public device was enrolled'
                call('inventory', status=403, **report)
                assert not (data / 'inventory').exists(), 'Public inventory was retained'

                # A legacy shared-list file is upgraded without losing entries.
                legacy_shared = call('account', owner)['shared']
                (data / 'shared.json').write_text(json.dumps(legacy_shared))
                key = 'isolated-private-station-key'
                settings(private=True, access_key=key)
                call('hello', status=403)
                assert call('hello', key=key)['inventory'] is True
                call('register', status=403, **registration)
                device_id = call('register', key=key, **registration)['id']
                assert device_id == public_id, 'Changing station mode changed an existing ID'
                shared = call('account', owner)['shared']
                enrolled = next(d for d in shared if d['id'] == device_id)
                assert enrolled['name'] == 'OFFICE-PC' and enrolled['group'] == ''
                assert not enrolled.get('secret'), 'Enrollment created a credential'
                assert all(d in shared for d in legacy_shared), 'Legacy shared list was lost'
                assert any(d['id'] == device_id for d in call('account', other)['shared'])
                call('register', key=key, **registration)
                assert len([d for d in call('account', owner)['shared'] if d['id'] == device_id]) == 1, 'Duplicate enrollment'
                call('register', key=key, status=403, **{**registration, 'sig': '00' * 64})
                renamed = {**enrolled, 'name': 'Reception', 'group': 'Office', 'notes': 'Keep my notes'}
                call('shared_save', owner, device=renamed)
                heartbeat = signed('register', addr={'id': '', 'addrs': []}, info={**info, 'name': 'HOST-RENAMED'})
                call('register', key=key, **heartbeat)
                preserved = next(d for d in call('account', owner)['shared'] if d['id'] == device_id)
                assert preserved['name'] == 'Reception' and preserved['group'] == 'Office' and preserved['notes'] == 'Keep my notes'

                call('inventory', key=key, **report)
                inventory_files = list((data / 'inventory').glob('*.json'))
                assert len(inventory_files) == 1
                stored = json.loads(inventory_files[0].read_text())
                assert stored['serial_number'] == summary['serial_number'] and stored['memory_bytes'] == summary['memory_bytes']
                assert 'password' not in stored and 'must-never-be-stored' not in inventory_files[0].read_text()
                assert 'SERIAL-ABC123' not in json.dumps(call('lookup', key=key, id=device_id)), 'Directory lookup exposed inventory'
                call('inventory', key=key, status=403, **{**report, 'payload': report['payload'].replace('SERIAL-ABC123', 'FORGED')})
                call('inventory', key=key, status=400, **signed('inventory', inventory={**summary, 'memory_bytes': []}))

                browser = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

                def admin(path='/admin/', status=200, **form):
                    request = urllib.request.Request(address + path,
                        data=urllib.parse.urlencode(form, doseq=True).encode() if form else None)
                    try:
                        response = browser.open(request, timeout=10)
                    except urllib.error.HTTPError as error:
                        response = error
                    with response:
                        assert response.status == status, ('admin', response.status)
                        return response.read().decode()

                page = admin()
                assert 'SERIAL-ABC123' not in page, 'Inventory was visible before admin sign-in'
                csrf = re.search(r'name="csrf" value="([a-f0-9]+)"', page).group(1)
                admin(csrf=csrf, action='device_assign', id=device_id, username='assigned')
                assert not json.loads((data / 'accounts/assigned.json').read_text())['devices'], 'Assignment worked without admin sign-in'
                admin(csrf=csrf, action='station_group_create', group='Unauthorized group')
                assert 'Unauthorized group' not in call('account', owner)['shared_groups']
                admin(csrf=csrf, action='login', password='local-admin-password')
                page = admin('/admin/?tab=devices')
                assert 'SERIAL-ABC123' in page and '16 GiB RAM' in page and 'Fixture CPU' in page
                assert 'Fixture &lt;Model&gt;' in page and 'Fixture <Model>' not in page, 'Inventory HTML was not escaped'
                assert 'Reception' in page and 'Office' in page and 'HOST-RENAMED' in page
                admin(csrf=csrf, action='shared_add', id=device_id, name='Edited on station', group='Servers', notes='Web notes', return_tab='devices')
                call('register', key=key, **heartbeat)
                edited = next(d for d in call('account', owner)['shared'] if d['id'] == device_id)
                assert edited['name'] == 'Edited on station' and edited['group'] == 'Servers' and edited['notes'] == 'Web notes'
                assert 'Assign to technician' in page and 'Assigned technician (assigned)' in page
                admin(status=400, csrf='invalid-csrf', action='device_assign', id=device_id, username='assigned')
                assert not json.loads((data / 'accounts/assigned.json').read_text())['devices']
                batch = {'usernames[]': ['assigned', 'other', 'assigned']}
                result = admin(csrf=csrf, action='device_assign', id=device_id, **batch)
                assert '2 added' in result, 'One action did not assign to both selected technicians'
                # The recipient had never signed in when the admin assigned it.
                assigned_token = call('login', username='assigned', password='local-assigned-password')['token']
                assigned_device = next(d for d in call('account', assigned_token)['devices'] if d['id'] == device_id)
                assert assigned_device['name'] == 'Edited on station' and assigned_device['group'] == 'Servers'
                assert assigned_device['notes'] == 'Web notes' and not assigned_device.get('secret')
                other_copy = next(d for d in call('account', other)['devices'] if d['id'] == device_id)
                assert other_copy['name'] == assigned_device['name'] and not other_copy.get('secret')
                assert not any(d['id'] == device_id for d in call('account', owner)['devices']), 'Assignment leaked into an unselected personal list'
                personalized = {**assigned_device, 'name': 'Technician label', 'group': 'Personal', 'notes': 'Keep personal notes',
                                'secret': {'n': 'ab' * 24, 'c': 'cd' * 16}}
                call('device_save', assigned_token, device=personalized)
                result = admin(csrf=csrf, action='device_assign', id=device_id, **batch)
                assert '0 added; 2 already assigned' in result
                own_list = call('account', assigned_token)['devices']
                assert len([d for d in own_list if d['id'] == device_id]) == 1
                reassigned = next(d for d in own_list if d['id'] == device_id)
                for field in ['name', 'group', 'notes', 'secret']:
                    assert reassigned[field] == personalized[field], ('Assignment overwrote personal data', field)
                assert not next(d for d in call('account', other)['devices'] if d['id'] == device_id).get('secret'), 'Personal password leaked to another batch recipient'
                partial = admin(csrf=csrf, action='device_assign', id=device_id, **{'usernames[]': ['batch-new', 'full-tech']})
                assert '1 added' in partial and 'full-tech:' in partial and 'list is full' in partial
                assert any(d['id'] == device_id for d in json.loads((data / 'accounts/batch-new.json').read_text())['devices'])
                assert len(json.loads((data / 'accounts/full-tech.json').read_text())['devices']) == 500
                assert 'Choose at least one' in admin(csrf=csrf, action='device_assign', id=device_id, usernames=[])
                assert 'no longer exists' in admin(csrf=csrf, action='device_assign', id=device_id, username='missing-tech')
                assert 'registered device' in admin(csrf=csrf, action='device_assign', id=42, username='assigned')
                admin(csrf=csrf, action='station_group_create', group='Empty station')
                admin(csrf=csrf, action='station_group_create', group='Laptop pool')
                groups = call('account', other)['shared_groups']
                assert 'Empty station' in groups and 'Laptop pool' in groups
                page = admin('/admin/?tab=groups')
                assert 'Assign groups to technicians' in page and 'Move workstations' in page
                admin(status=400, csrf='invalid-csrf', action='station_group_create', group='Bad CSRF')
                admin(csrf=csrf, action='station_group_move', group='Laptop pool', **{'ids[]': [device_id, 482913078]})
                moved = call('account', owner)['shared']
                assert all(next(d for d in moved if d['id'] == selected)['group'] == 'Laptop pool' for selected in [device_id, 482913078])
                call('register', key=key, **heartbeat)
                assert next(d for d in call('account', owner)['shared'] if d['id'] == device_id)['group'] == 'Laptop pool'
                admin(csrf=csrf, action='station_group_rename', group='Laptop pool', new_name='Support laptops')
                current = call('account', other)
                assert 'Support laptops' in current['shared_groups'] and 'Laptop pool' not in current['shared_groups']
                assert next(d for d in current['shared'] if d['id'] == device_id)['group'] == 'Support laptops'
                assert 'already has that name' in admin(csrf=csrf, action='station_group_rename', group='Support laptops', new_name='Empty station')
                assert 'Move workstations out' in admin(csrf=csrf, action='station_group_delete', group='Support laptops')
                group_batch = {'groups[]': ['Support laptops', 'Empty station'], 'usernames[]': ['assigned', 'other']}
                assert '2 added' in admin(csrf=csrf, action='group_assign', **group_batch)
                for token in [assigned_token, other]:
                    state = call('account', token)
                    assert {'Support laptops', 'Empty station'} <= set(state['groups'])
                    grouped_device = next(d for d in state['devices'] if d['id'] == 482913078)
                    assert grouped_device['group'] == 'Support laptops' and not grouped_device.get('secret')
                personal = next(d for d in call('account', assigned_token)['devices'] if d['id'] == device_id)
                assert all(personal[field] == personalized[field] for field in ['name', 'group', 'notes', 'secret']), 'Group assignment overwrote personal preferences or credentials'
                assert '0 added; 2 already assigned' in admin(csrf=csrf, action='group_assign', **group_batch)
                partial_group = admin(csrf=csrf, action='group_assign', **{'groups[]': ['Support laptops', 'Empty station'], 'usernames[]': ['batch-new', 'full-tech']})
                assert '1 added' in partial_group and 'full-tech:' in partial_group
                assert not json.loads((data / 'accounts/full-tech.json').read_text()).get('groups'), 'Failed assignment left a partial group behind'
                assert 'no longer exists' in admin(csrf=csrf, action='group_assign', **{'groups[]': ['Missing group'], 'usernames[]': ['assigned']})
                admin(csrf=csrf, action='station_group_delete', group='Empty station')
                assert 'Empty station' not in call('account', other)['shared_groups']
                assert 'Empty station' in call('account', assigned_token)['groups'], 'Station cleanup removed a technician’s personal group'
                admin(csrf=csrf, action='station_group_move', group='', **{'ids[]': [482913078]})
                assert next(d for d in call('account', owner)['shared'] if d['id'] == 482913078)['group'] == ''
                call('unregister', key=key, **signed('unregister'))
                assert call('lookup', key=key, id=device_id)['status'] == 'offline'
                admin(csrf=csrf, action='device_assign', id=device_id, username='owner')
                assert any(d['id'] == device_id for d in call('account', owner)['devices']), 'Offline device could not be assigned'
                call('shared_delete', owner, id=device_id)
                call('register', key=key, **heartbeat)
                assert not any(d['id'] == device_id for d in call('account', owner)['shared']), 'Heartbeat resurrected a removed device'
                clone_ids = []
                for seed, computer in [('03', 'IMAGED-LAPTOP-01'), ('04', 'IMAGED-LAPTOP-02')]:
                    clone_request = signed('register', seed=seed, addr={'id': '', 'addrs': []}, info={**info, 'name': computer})
                    clone_id = call('register', key=key, **clone_request)['id']
                    assert clone_id != device_id and clone_id not in clone_ids, 'An imaged laptop reused an existing station ID'
                    assert call('register', key=key, **clone_request)['id'] == clone_id
                    clone_ids.append(clone_id)
                    record = next(d for d in call('account', owner)['shared'] if d['id'] == clone_id)
                    assert record['name'] == computer and record['group'] == '' and not record.get('secret')
                settings(private=False)
                assert call('hello')['inventory'] is False
                call('inventory', status=403, **report)
                another = call('register', **signed('register', seed='02', addr={'id': '', 'addrs': []}, info=info))['id']
                assert not any(d['id'] == another for d in call('account', owner)['shared'])
                assert 'SERIAL-ABC123' not in admin('/admin/?tab=devices'), 'Public admin page exposed retained private inventory'
                assert len(list((data / 'inventory').glob('*.json'))) == 1
                print('PHP station: signed private enrollment, legacy migration, Ungrouped, durable edits/removal, hardware summary, admin editing/escaping and public exclusion passed', flush=True)
                print('PHP station: batch technician assignment before first sign-in, admin/CSRF checks, offline devices, duplicate suppression, partial failures and preserved personal credentials/details passed', flush=True)
                print('PHP station: cloned laptops register separate stable IDs and computer names on the same private station passed', flush=True)
                print('PHP station: shared/empty group synchronization, batch moves, rename/delete rules, batch group assignment, personal credential preservation and atomic per-technician capacity failures passed', flush=True)

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
