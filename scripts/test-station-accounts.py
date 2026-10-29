#!/usr/bin/env python3
"""Exercise saved devices, private enrollment and inventory on a local PHP station.

Set SLIPSTREAM_TEST_PHP if php isn't on PATH. Optionally pass --client PATH
for the Rust CLI/vault/cache round trip. No real station or account is used.
"""
import argparse
import http.cookiejar
import hashlib
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
        # Exercise the bounded SQL batch as well as the file registry used by
        # the HTTP fixture below. It must return only IDs and online lamps.
        sql_test = '''require $argv[1];
$store=new SlipPdoStore(['dsn'=>'sqlite::memory:']);
$id=$store->register(str_repeat('ab',32),['name'=>'SQL host','os'=>'Linux','app_version'=>'fixture'],'{}');
$unknown=$id===100000000 ? 100000001 : 100000000;
$expected=[['id'=>$id,'status'=>'online'],['id'=>$unknown,'status'=>'not_found']];
if ($store->presence([$id,$unknown])!==$expected || $store->presence([])!==[]) exit(1);
$store->unregister(str_repeat('ab',32));
if ($store->presence([$id])!==[['id'=>$id,'status'=>'offline']]) exit(1);
'''
        subprocess.run([php, '-r', sql_test, str(web / 'lib/slipstream.php')], check=True, capture_output=True)
        print('PHP station: SQL batch online/offline/unknown statuses and minimal response passed', flush=True)
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

                def station_shared():
                    state = json.loads((data / 'shared.json').read_text())
                    return state['devices'] if isinstance(state, dict) else state

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
                assert station_shared()[0]['group'] == 'Shared servers'
                assert not station_shared()[0].get('secret'), 'Shared entry retained a password'
                assert not state['shared'] and not call('account', other)['shared'], 'Unassigned station group was visible'
                shared.pop('group')
                call('shared_save', owner, device=shared)
                assert station_shared()[0]['group'] == 'Shared servers'
                call('shared_save', owner, device={**shared, 'group': ''})
                assert station_shared()[0]['group'] == ''
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
                           'memory_bytes': 16 * 1024**3, 'motherboard': 'Fixture X870', 'motherboard_serial': 'BOARD-ABC123',
                           'report': [{'title': 'Network <adapters>', 'rows': [['IPv4 address', '192.0.2.42'], ['Printer', '<script>alert(1)</script>']], 'password': 'must-never-be-stored'}],
                           'password': 'must-never-be-stored'}
                report = signed('inventory', inventory=summary)
                assert call('hello')['inventory'] is False
                public_id = call('register', **registration)['id']

                # Refreshing hundreds of online lamps uses one independent
                # request, not hundreds of connection lookups.
                endpoint = json.loads(registration['payload'])['endpoint']

                def seed_rate(kind, count, age=0, subject='127.0.0.1'):
                    subprocess.run([php, '-r', """require $argv[1];
$cfg=slip_config();
if ($cfg['lookups_per_minute']!==600 || $cfg['registers_per_minute']!==30) exit(1);
$file=slip_data_dir().'/rate/'.sha1($argv[2].'|'.$argv[5]);
file_put_contents($file,(time()-(int)$argv[4]).' '.$argv[3]);""",
                                    str(web / 'lib/slipstream.php'), kind, str(count), str(age), subject], check=True, capture_output=True)

                ids = [public_id] + [id for id in range(100000000, 100000500) if id != public_id][:499]
                lamps = call('presence', ids=ids)['devices']
                assert len(lamps) == 500 and lamps[0] == {'id': public_id, 'status': 'online'}
                assert all(set(lamp) == {'id', 'status'} for lamp in lamps), 'Presence leaked addresses or inventory'
                assert call('presence', ids=[public_id, public_id])['devices'] == [lamps[0]], 'Duplicate devices were polled twice'
                assert call('presence', ids=[])['devices'] == []
                for invalid in (None, '123456789', [True], [42], [123456789.5], [public_id] * 501):
                    call('presence', ids=invalid, status=400)

                seed_rate('presence', 119)
                call('presence', ids=ids)
                call('presence', ids=ids, status=429)
                call('lookup', id=public_id)
                call('register', **registration)
                seed_rate('presence', 121, age=61)
                call('presence', ids=ids)
                seed_rate('lookup', 599)
                call('lookup', id=public_id)
                call('lookup', id=public_id, status=429)
                call('presence', ids=ids)
                seed_rate('lookup', 601, age=61)
                call('lookup', id=public_id)

                seed_rate('register', 29, subject='device:' + endpoint)
                call('register', **registration)
                call('register', status=429, **registration)
                # Neither another device on this IP nor status/Connect is blocked.
                call('register', **signed('register', seed='aa', addr={'id': '', 'addrs': []}, info=info))
                call('presence', ids=ids)
                call('lookup', id=public_id)
                forged = {**registration, 'sig': '00' * 64}
                call('register', status=403, **forged)
                assert (data / 'rate' / hashlib.sha1(('register|device:' + endpoint).encode()).hexdigest()).read_text().endswith('31'), 'Forged signature consumed the device allowance'
                seed_rate('register', 31, age=61, subject='device:' + endpoint)
                call('register', **registration)
                print('PHP station: bounded batch presence, independent connection budgets, per-device signed heartbeats, signature isolation and expiry passed', flush=True)

                assert not any(d['id'] == public_id for d in station_shared()), 'Public device was enrolled'
                call('inventory', status=403, **report)
                assert not (data / 'inventory').exists(), 'Public inventory was retained'

                # A legacy shared-list file is upgraded without losing entries.
                legacy_shared = station_shared()
                (data / 'shared.json').write_text(json.dumps(legacy_shared))
                key = 'isolated-private-station-key'
                settings(private=True, access_key=key)
                call('hello', status=403)
                assert call('hello', key=key)['inventory'] is True
                assert call('hello', key=key)['inventory_report'] is True
                call('register', status=403, **registration)
                device_id = call('register', key=key, **registration)['id']
                assert device_id == public_id, 'Changing station mode changed an existing ID'
                shared = station_shared()
                enrolled = next(d for d in shared if d['id'] == device_id)
                assert enrolled['name'] == 'OFFICE-PC' and enrolled['group'] == ''
                assert not enrolled.get('secret'), 'Enrollment created a credential'
                assert all(d in shared for d in legacy_shared), 'Legacy shared list was lost'
                assert not call('account', other)['shared'], 'Automatic enrollment leaked unassigned workstations'
                call('register', key=key, **registration)
                assert len([d for d in station_shared() if d['id'] == device_id]) == 1, 'Duplicate enrollment'
                call('register', key=key, status=403, **{**registration, 'sig': '00' * 64})
                renamed = {**enrolled, 'name': 'Reception', 'group': 'Office', 'notes': 'Keep my notes'}
                call('shared_save', owner, device=renamed)
                heartbeat = signed('register', addr={'id': '', 'addrs': []}, info={**info, 'name': 'HOST-RENAMED'})
                call('register', key=key, **heartbeat)
                preserved = next(d for d in station_shared() if d['id'] == device_id)
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

                for invalid in [None, 'bad', [{'title': 'Bad', 'rows': [['one']]}], [{'title': 3, 'rows': []}], [{'title': 'Bad', 'rows': [['key', []]]}], [{'title': 'Bad', 'rows': [['key', 'x' * 8193]]}]]:
                    if invalid is not None:
                        call('inventory', key=key, status=400, **signed('inventory', inventory={**summary, 'report': invalid}))
                legacy_summary = {k: v for k, v in summary.items() if k != 'report'}
                call('inventory', key=key, **signed('inventory', inventory=legacy_summary))
                retained = json.loads(inventory_files[0].read_text())
                assert retained['report'] == stored['report'] and retained['report_at'] == stored['report_at'], 'Legacy summary erased or redated the full report'

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
                assert '192.0.2.42' not in admin('/admin/?tab=devices&device=' + str(device_id)), 'Report was visible before sign-in'
                csrf = re.search(r'name="csrf" value="([a-f0-9]+)"', page).group(1)
                admin(csrf=csrf, action='device_assign', id=device_id, username='assigned')
                assert not json.loads((data / 'accounts/assigned.json').read_text())['devices'], 'Assignment worked without admin sign-in'
                admin(csrf=csrf, action='station_group_create', group='Unauthorized group')
                assert 'Unauthorized group' not in json.loads((data / 'shared.json').read_text())['groups']
                admin(csrf=csrf, action='group_visibility', owner='', group='', **{'usernames[]': ['other']})
                assert not json.loads((data / 'shared.json').read_text()).get('group_assignments'), 'Visibility changed without admin sign-in'
                admin(csrf=csrf, action='login', username='admin', password='local-admin-password')
                page = admin('/admin/?tab=devices')
                assert 'SERIAL-ABC123' in page and '16 GiB RAM' in page and 'Fixture CPU' in page
                assert 'Fixture &lt;Model&gt;' in page and 'Fixture <Model>' not in page, 'Inventory HTML was not escaped'
                report_page = admin('/admin/?tab=devices&device=' + str(device_id))
                assert '192.0.2.42' in report_page and 'BOARD-ABC123' in report_page and 'Fixture X870' in report_page
                assert 'Network &lt;adapters&gt;' in report_page and '<script>alert(1)</script>' not in report_page
                assert '&lt;script&gt;alert(1)&lt;/script&gt;' in report_page and 'Last reported information' in report_page
                admin('/admin/?tab=devices&device=42', status=404)
                assert 'Reception' in page and 'Office' in page and 'HOST-RENAMED' in page
                admin(csrf=csrf, action='shared_add', id=device_id, name='Edited on station', group='Servers', notes='Web notes', return_tab='devices')
                call('register', key=key, **heartbeat)
                edited = next(d for d in station_shared() if d['id'] == device_id)
                assert edited['name'] == 'Edited on station' and edited['group'] == 'Servers' and edited['notes'] == 'Web notes'
                assert 'Assign to technician' in page and 'Assigned technician' in page and 'value="assigned"' in page
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
                # A source-aware catalog includes technician groups, without exposing their vault.
                call('group_save', owner, group='Youthnet <main>')
                youthnet = {**device, 'id': 482913080, 'name': 'Youthnet PC', 'group': 'Youthnet <main>'}
                call('device_save', owner, device=youthnet)
                call('group_save', other, group='Youthnet <main>')
                admin(csrf=csrf, action='station_group_create', group='Empty station')
                admin(csrf=csrf, action='station_group_create', group='Laptop pool')
                assert call('account', other)['shared_groups'] == [], 'New empty groups were visible without assignment'
                page = admin('/admin/?tab=groups')
                assert 'Groups &amp; technician access' in page and 'Save workstations' in page
                assert 'Youthnet &lt;main&gt;' in page and 'Youthnet <main>' not in page
                assert 'Owner (owner)' in page and 'Other (other)' in page and 'Personal technician group' in page
                assert 'No technicians assigned' in page and 'Visible to these technicians' in page
                assert device['secret']['c'] not in page and device['secret']['n'] not in page, 'Group catalog exposed sealed credentials'
                admin(status=400, csrf='invalid-csrf', action='group_visibility', owner='', group='Empty station', **{'usernames[]': ['other']})
                admin(csrf=csrf, action='group_visibility', owner='', group='Empty station', usernames='other')
                assert call('account', other)['shared_groups'] == [], 'Malformed recipient list changed visibility'
                admin(csrf=csrf, action='station_group_move', group='Laptop pool', **{'ids[]': [device_id, 482913078]})
                moved = station_shared()
                assert all(next(d for d in moved if d['id'] == selected)['group'] == 'Laptop pool' for selected in [device_id, 482913078])
                grant = {'owner': '', 'group': 'Laptop pool', 'usernames[]': ['assigned', 'other', 'assigned', 'full-tech']}
                assert 'Group assignments saved' in admin(csrf=csrf, action='group_visibility', **grant)
                admin(csrf=csrf, action='group_visibility', owner='', group='Empty station', **{'usernames[]': ['other']})
                assert not call('account', owner)['shared'], 'Admin role bypassed explicit group visibility'
                for token in [assigned_token, other]:
                    state = call('account', token)
                    assert 'Laptop pool' in state['shared_groups']
                    assert {d['id'] for d in state['shared']} == {device_id, 482913078}
                    assert all(not d.get('secret') for d in state['shared'])
                full_token = call('login', username='full-tech', password='local-full-password')['token']
                full_state = call('account', full_token)
                assert len(full_state['devices']) == 500 and 'Laptop pool' in full_state['shared_groups'], 'Shared visibility unnecessarily copied into a full personal list'
                assert len(full_state['shared']) == 2 and not full_state['groups']
                assert 'Empty station' in call('account', other)['shared_groups']
                assert 'Empty station' not in call('account', assigned_token)['shared_groups']
                # Unlike the old copying action, groups consume no personal vault entries.
                assert 'Laptop pool' not in call('account', assigned_token)['groups']
                personal = next(d for d in call('account', assigned_token)['devices'] if d['id'] == device_id)
                assert all(personal[field] == personalized[field] for field in ['name', 'group', 'notes', 'secret'])
                # Saved form selections must reflect the existing assignment.
                page = admin('/admin/?tab=groups')
                forms = re.findall(r'<form method="post"[^>]*>(.*?)</form>', page, re.S)
                selected_form = next(f for f in forms if 'value="group_visibility"' in f and 'name="owner" value=""' in f and 'name="group" value="Laptop pool"' in f)
                assert re.search(r'value="assigned"\s+checked', selected_form) and re.search(r'value="other"\s+checked', selected_form)
                assert not re.search(r'value="owner"\s+checked', selected_form)
                # Permission edits are atomic: one invalid technician must not replace the old grants.
                assert 'no longer exists' in admin(csrf=csrf, action='group_visibility', owner='', group='Laptop pool', **{'usernames[]': ['other', 'missing-tech']})
                assert 'Laptop pool' in call('account', assigned_token)['shared_groups']
                admin(csrf=csrf, action='station_group_rename', group='Laptop pool', new_name='Support laptops')
                current = call('account', other)
                assert 'Support laptops' in current['shared_groups'] and 'Laptop pool' not in current['shared_groups']
                assert next(d for d in current['shared'] if d['id'] == device_id)['group'] == 'Support laptops'
                assert 'already has that name' in admin(csrf=csrf, action='station_group_rename', group='Support laptops', new_name='Empty station')
                assert 'Move workstations out' in admin(csrf=csrf, action='station_group_delete', group='Support laptops')
                # New members arrive on refresh, and clearing all checkboxes revokes access.
                call('shared_save', owner, device={'id': 482913081, 'name': 'New laptop', 'group': 'Support laptops'})
                assert any(d['id'] == 482913081 for d in call('account', other)['shared'])
                admin(csrf=csrf, action='group_visibility', owner='', group='Support laptops', **{'usernames[]': ['other']})
                assert not call('account', assigned_token)['shared'] and not call('account', assigned_token)['shared_groups']
                assert next(d for d in call('account', assigned_token)['devices'] if d['id'] == device_id)['secret'] == personalized['secret']
                admin(csrf=csrf, action='group_visibility', owner='', group='Support laptops')
                assert 'Support laptops' not in call('account', other)['shared_groups']
                # Technician groups share by source, including an independently named group in another account.
                admin(csrf=csrf, action='group_visibility', owner='owner', group='Youthnet <main>', **{'usernames[]': ['assigned', 'owner']})
                youth = call('account', assigned_token)
                assert 'Youthnet <main>' in youth['shared_groups']
                assert next(d for d in youth['shared'] if d['id'] == youthnet['id'])['name'] == youthnet['name']
                assert not any(d.get('secret') for d in youth['shared']), 'Personal group sharing leaked a password'
                assert 'Youthnet <main>' not in call('account', other)['shared_groups']
                # Changes in the owner's group propagate, and membership is independent of similarly named groups.
                youthnet2 = {**youthnet, 'id': 482913082, 'name': 'Second Youthnet PC'}
                call('device_save', owner, device=youthnet2)
                assert {d['id'] for d in call('account', assigned_token)['shared']} == {youthnet['id'], youthnet2['id']}
                call('device_save', owner, device={**youthnet2, 'group': ''})
                assert {d['id'] for d in call('account', assigned_token)['shared']} == {youthnet['id']}
                admin(csrf=csrf, action='group_visibility', owner='other', group='Youthnet <main>', **{'usernames[]': ['batch-new']})
                assert {d['id'] for d in call('account', assigned_token)['shared']} == {youthnet['id']}
                admin(csrf=csrf, action='group_visibility', owner='owner', group='Youthnet <main>')
                assert not call('account', assigned_token)['shared_groups']
                assert next(d for d in call('account', owner)['devices'] if d['id'] == youthnet['id'])['secret'] == youthnet['secret']
                # A personal folder inferred from devices remains manageable when shared and emptied.
                inferred = {**device, 'id': 482913083, 'group': 'Inferred team'}
                call('device_save', owner, device=inferred)
                admin(csrf=csrf, action='group_visibility', owner='owner', group='Inferred team', **{'usernames[]': ['assigned']})
                call('device_save', owner, device={**inferred, 'group': ''})
                assert 'Inferred team' in call('account', assigned_token)['shared_groups']
                assert 'Inferred team' in call('account', owner)['groups']
                assert 'Inferred team' in admin('/admin/?tab=groups')
                call('group_delete', owner, group='Inferred team')
                assert 'Inferred team' not in call('account', assigned_token)['shared_groups']
                assert 'Inferred team' not in call('account', owner)['groups']
                assert next(d for d in call('account', owner)['devices'] if d['id'] == inferred['id'])['secret'] == inferred['secret']
                # Ungrouped uses an explicit grant as well; moves enforce the destination's visibility.
                admin(csrf=csrf, action='station_group_move', group='', **{'ids[]': [482913078]})
                assert not call('account', assigned_token)['shared']
                admin(csrf=csrf, action='group_visibility', owner='', group='', **{'usernames[]': ['assigned']})
                assert any(d['id'] == 482913078 for d in call('account', assigned_token)['shared'])
                admin(csrf=csrf, action='group_visibility', owner='', group='Support laptops', **{'usernames[]': ['assigned']})
                admin(csrf=csrf, action='station_group_move', group='Servers', **{'ids[]': [482913078]})
                assert not any(d['id'] == 482913078 for d in call('account', assigned_token)['shared'])
                assert '' in call('account', assigned_token)['shared_groups'], 'Assigned empty Ungrouped folder disappeared'
                # Deletion and recreation never restore old permissions.
                admin(csrf=csrf, action='station_group_delete', group='Empty station')
                admin(csrf=csrf, action='station_group_create', group='Empty station')
                assert 'Empty station' not in call('account', other)['shared_groups']
                admin(csrf=csrf, action='group_visibility', owner='other', group='Youthnet <main>', **{'usernames[]': ['assigned']})
                call('group_delete', other, group='Youthnet <main>')
                call('group_save', other, group='Youthnet <main>')
                assert 'Youthnet <main>' not in call('account', assigned_token)['shared_groups']
                assert 'no longer exists' in admin(csrf=csrf, action='group_visibility', owner='', group='Missing group')
                page = admin('/admin/?tab=techs')
                assert 'Personal / assigned groups' in page and 'Manage group assignments' in page
                # Deleting an account also clears its source shares and received assignments.
                admin(csrf=csrf, action='group_visibility', owner='other', group='Youthnet <main>', **{'usernames[]': ['assigned']})
                admin(csrf=csrf, action='tech_delete', username='other')
                assert not any(a['owner'] == 'other' or 'other' in a['usernames'] for a in json.loads((data / 'shared.json').read_text())['group_assignments'])
                call('unregister', key=key, **signed('unregister'))
                assert call('lookup', key=key, id=device_id)['status'] == 'offline'
                admin(csrf=csrf, action='device_assign', id=device_id, username='owner')
                assert any(d['id'] == device_id for d in call('account', owner)['devices']), 'Offline device could not be assigned'
                call('shared_delete', owner, id=device_id)
                call('register', key=key, **heartbeat)
                assert not any(d['id'] == device_id for d in station_shared()), 'Heartbeat resurrected a removed device'
                clone_ids = []
                for seed, computer in [('03', 'IMAGED-LAPTOP-01'), ('04', 'IMAGED-LAPTOP-02')]:
                    clone_request = signed('register', seed=seed, addr={'id': '', 'addrs': []}, info={**info, 'name': computer})
                    clone_id = call('register', key=key, **clone_request)['id']
                    assert clone_id != device_id and clone_id not in clone_ids, 'An imaged laptop reused an existing station ID'
                    assert call('register', key=key, **clone_request)['id'] == clone_id
                    clone_ids.append(clone_id)
                    record = next(d for d in station_shared() if d['id'] == clone_id)
                    assert record['name'] == computer and record['group'] == '' and not record.get('secret')
                # Multiple station memberships project only the folders granted to each client.
                multi_id, extra_id = clone_ids
                call('shared_save', owner, device={'id': multi_id, 'name': 'Multi-group PC', 'groups': ['Support laptops', 'Servers']})
                admin(csrf=csrf, action='group_visibility', owner='', group='Support laptops', **{'usernames[]': ['assigned']})
                admin(csrf=csrf, action='group_visibility', owner='', group='Servers', **{'usernames[]': ['batch-new']})
                batch_token = call('login', username='batch-new', password='local-batch-password')['token']
                assigned_rows = [d for d in call('account', assigned_token)['shared'] if d['id'] == multi_id]
                batch_rows = [d for d in call('account', batch_token)['shared'] if d['id'] == multi_id]
                assert [d['group'] for d in assigned_rows] == ['Support laptops']
                assert [d['group'] for d in batch_rows] == ['Servers']
                assert all('groups' not in d and not d.get('secret') for d in assigned_rows + batch_rows), 'Other memberships or passwords leaked'
                # A legacy edit from either projected folder keeps all memberships.
                for row in assigned_rows + batch_rows:
                    call('shared_save', owner, device={**row, 'notes': 'Keep both folders'})
                    assert set(next(d for d in station_shared() if d['id'] == multi_id)['groups']) == {'Support laptops', 'Servers'}
                for bad_groups in ['Servers', [None], [''], ['x'] * 33]:
                    call('shared_save', owner, status=400, device={'id': multi_id, 'groups': bad_groups})
                admin(csrf=csrf, action='shared_add', return_tab='devices', id=multi_id, name='Multi-group PC', groups_present='1', new_group='Inline group', **{'groups[]': ['Support laptops', 'Servers']})
                current = next(d for d in station_shared() if d['id'] == multi_id)
                assert set(current['groups']) == {'Support laptops', 'Servers', 'Inline group'}
                assert 'Inline group' in json.loads((data / 'shared.json').read_text())['groups']
                before = (data / 'shared.json').read_bytes()
                admin(status=400, csrf='invalid', action='group_members', owner='', group='Servers', **{'ids[]': [multi_id]})
                assert (data / 'shared.json').read_bytes() == before
                assert 'valid workstations' in admin(csrf=csrf, action='group_members', owner='', group='Servers', **{'ids[]': ['invalid']})
                assert (data / 'shared.json').read_bytes() == before
                admin(csrf=csrf, action='group_members', owner='', group='Servers', **{'ids[]': [multi_id, extra_id]})
                assert set(next(d for d in station_shared() if d['id'] == multi_id)['groups']) == {'Support laptops', 'Servers', 'Inline group'}
                assert {d['id'] for d in call('account', batch_token)['shared'] if d['group'] == 'Servers'} == {multi_id, extra_id}
                admin(csrf=csrf, action='group_members', owner='', group='Servers', **{'ids[]': [extra_id]})
                assert set(next(d for d in station_shared() if d['id'] == multi_id)['groups']) == {'Support laptops', 'Inline group'}
                assert not any(d['id'] == multi_id for d in call('account', batch_token)['shared'])
                assert any(d['id'] == multi_id for d in call('account', assigned_token)['shared'])
                admin(csrf=csrf, action='station_group_rename', group='Inline group', new_name='Renamed group')
                assert set(next(d for d in station_shared() if d['id'] == multi_id)['groups']) == {'Support laptops', 'Renamed group'}
                assert 'Move workstations out' in admin(csrf=csrf, action='station_group_delete', group='Renamed group')
                # Personal memberships preserve saved credentials and other folders too.
                account_before = json.loads((data / 'accounts/owner.json').read_text())
                personal_before = next(d for d in account_before['devices'] if d['id'] == device_id)
                youth_ids = [d['id'] for d in account_before['devices'] if d.get('group') == 'Youthnet <main>']
                admin(csrf=csrf, action='group_members', owner='owner', group='Youthnet <main>', **{'ids[]': youth_ids + [device_id, multi_id]})
                account_after = json.loads((data / 'accounts/owner.json').read_text())
                personal_after = next(d for d in account_after['devices'] if d['id'] == device_id)
                assert personal_after['secret'] == personal_before['secret']
                assert 'Youthnet <main>' in personal_after['groups']
                if personal_before.get('group'):
                    assert personal_before['group'] in personal_after['groups']
                admin(csrf=csrf, action='group_members', owner='owner', group='Youthnet <main>', **{'ids[]': youth_ids})
                account_after = json.loads((data / 'accounts/owner.json').read_text())
                personal_after = next(d for d in account_after['devices'] if d['id'] == device_id)
                assert personal_after['secret'] == personal_before['secret'] and personal_after['group'] == personal_before.get('group', '')
                admin(csrf=csrf, action='shared_add', return_tab='devices', id=multi_id, name='Multi-group PC', groups_present='1')
                assert next(d for d in station_shared() if d['id'] == multi_id)['groups'] == []
                assert not any(d['id'] == multi_id for d in call('account', assigned_token)['shared'] if d['group'] != '')
                print('PHP station: multiple groups, inline creation, per-group visibility, legacy edits, membership changes, rename, clear and personal vault preservation passed', flush=True)
                settings(private=False)
                assert call('hello')['inventory'] is False
                call('inventory', status=403, **report)
                another = call('register', **signed('register', seed='02', addr={'id': '', 'addrs': []}, info=info))['id']
                assert not any(d['id'] == another for d in station_shared())
                public_page = admin('/admin/?tab=devices')
                assert 'data-device-controls' in public_page and 'data-device-rows' in public_page, 'Public directory lacks device search/sort controls'
                assert 'data-device-hardware=' not in public_page and 'data-device-sort-option="hardware"' not in public_page, 'Public directory exposed private hardware controls'
                assert 'SERIAL-ABC123' not in public_page and 'BOARD-ABC123' not in public_page, 'Public search metadata exposed retained private inventory'
                assert '192.0.2.42' not in admin('/admin/?tab=devices&device=' + str(device_id), status=404), 'Public station exposed a retained full report'
                assert len(list((data / 'inventory').glob('*.json'))) == 1
                print('PHP station: signed private enrollment, legacy migration, Ungrouped, durable edits/removal, hardware summary, admin editing/escaping and public exclusion passed', flush=True)
                print('PHP station: batch technician assignment before first sign-in, admin/CSRF checks, offline devices, duplicate suppression, partial failures and preserved personal credentials/details passed', flush=True)
                print('PHP station: cloned laptops register separate stable IDs and computer names on the same private station passed', flush=True)
                print('PHP station: source-aware personal/station catalog, explicit multi-technician visibility, live membership, revocation, Ungrouped, saved selections, rename/delete cleanup and private vault preservation passed', flush=True)

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
