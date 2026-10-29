#!/usr/bin/env python3
"""Check named station admins, migration and revocation on an isolated PHP site."""
import argparse
import contextlib
import http.cookiejar
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        return None


class Browser:
    def __init__(self, address):
        self.address = address
        self.opener = urllib.request.build_opener(NoRedirect(), urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
        self.csrf = None

    def request(self, path='/admin/?tab=techs', status=200, **form):
        if form:
            form = {'csrf': self.csrf, **form}
        request = urllib.request.Request(self.address + path, data=urllib.parse.urlencode(form).encode() if form else None)
        try:
            response = self.opener.open(request, timeout=10)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            body = response.read().decode()
            assert response.status == status, (response.status, status, body[:160])
        match = re.search(r'name="csrf" value="([a-f0-9]+)"', body)
        if match:
            self.csrf = match.group(1)
        return body

    def login(self, username, password, status=302):
        self.request()
        return self.request(action='login', username=username, password=password, status=status)

    def authorized(self):
        return 'name="action" value="tech_add"' in self.request()

    def save_visibility(self, status=200, **form):
        fields = {'csrf': self.csrf, 'action': 'group_visibility', 'complete': '1', **form}
        request = urllib.request.Request(self.address + '/admin/?tab=groups',
            data=urllib.parse.urlencode(fields, doseq=True).encode(), headers={'Accept': 'application/json'})
        try:
            response = self.opener.open(request, timeout=10)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            assert response.status == status, (response.status, status)
            assert response.headers.get_content_type() == 'application/json'
            result = json.load(response)
            assert result['ok'] == (status == 200)
            return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--browser', action='store_true', help='Also check responsive layouts with Chromium')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    php = os.environ.get('SLIPSTREAM_TEST_PHP') or shutil.which('php') or str(Path.home() / '.cache/slipstream-qa-tools/php')
    for source in (root / 'server/station').rglob('*.php'):
        subprocess.run([php, '-l', str(source)], check=True, capture_output=True)
    print('PHP station sources: lint passed', flush=True)
    with tempfile.TemporaryDirectory(prefix='slipstream-admin-test-') as temp:
        base = Path(temp)
        web, data = base / 'web', base / 'data'
        shutil.copytree(root / 'server/station', web)
        (web / 'config.php').write_text('<?php return ["data_dir" => ' + repr(str(data)) + '];\n')

        def cli(code, stdin=None):
            return subprocess.run([php, '-r', 'require $argv[1]; ' + code, str(web / 'lib/slipstream.php')], check=True, capture_output=True, text=True, input=stdin).stdout

        cli("slip_update_save(['checked'=>time()]);")
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
                            raise RuntimeError('PHP fixture failed to start')
                        time.sleep(.05)
                else:
                    raise RuntimeError('PHP fixture startup timed out')
                admin = Browser(address)
                page = admin.request()
                assert 'first administrator account' in page
                admin.request(action='setup', org='Admin fixture', password='original-station-password', password2='original-station-password', status=302)
                account = json.loads((data / 'accounts/admin.json').read_text())
                assert account['admin'] and account['username'] == 'admin'
                assert not json.loads((data / 'station.json').read_text()).get('admin_hash')
                assert admin.authorized(), 'Setup did not establish a named admin session'
                admin.request(action='tech_admin', username='admin', admin='', status=302)
                assert 'Keep at least one' in admin.request()
                admin.request(action='tech_delete', username='admin', status=302)
                assert 'Keep at least one' in admin.request() and admin.authorized()
                admin.request(status=400, csrf='invalid', action='tech_add', username='forbidden', password='forbidden-password')
                assert not (data / 'accounts/forbidden.json').exists()
                admin.request(action='tech_add', username='ordinary', name='Ordinary technician', password='ordinary-password', status=302)
                ordinary = Browser(address)
                assert 'not a station admin' in ordinary.login('ordinary', 'ordinary-password', status=200)
                assert not ordinary.authorized()
                ordinary.request(action='tech_admin', username='ordinary', admin='1')
                assert not json.loads((data / 'accounts/ordinary.json').read_text())['admin'], 'Unauthorized POST changed permissions'
                admin.request(action='station_group_create', group='Done one', status=302)
                admin.request(action='station_group_create', group='Done two', status=302)
                admin.save_visibility(owner='', group='Done one', **{'usernames[]': ['ordinary']})
                admin.save_visibility(owner='', group='Done two', **{'usernames[]': ['admin', 'ordinary']})
                def assignments():
                    return {(a['owner'], a['group']): a['usernames'] for a in json.loads((data / 'shared.json').read_text())['group_assignments']}
                assert assignments()[('', 'Done one')] == ['ordinary']
                assert assignments()[('', 'Done two')] == ['admin', 'ordinary'], 'Saving the second group lost the first'
                before = (data / 'shared.json').read_bytes()
                admin.save_visibility(status=400, csrf='invalid', owner='', group='Done one')
                admin.save_visibility(status=400, complete='', owner='', group='Done one')
                admin.save_visibility(status=400, owner='', group='Done one', **{'usernames[]': ['missing-technician']})
                ordinary.save_visibility(status=401, owner='', group='Done one')
                assert (data / 'shared.json').read_bytes() == before, 'A rejected inline save changed permissions'
                mode = data.stat().st_mode & 0o777
                try:
                    data.chmod(0o500)
                    admin.save_visibility(status=500, owner='', group='Done one')
                finally:
                    data.chmod(mode)
                assert (data / 'shared.json').read_bytes() == before, 'A failed write changed permissions'
                admin.save_visibility(owner='', group='Done one')
                assert ('', 'Done one') not in assignments() and assignments()[('', 'Done two')] == ['admin', 'ordinary']
                print('Station groups: inline saves across rows, clearing, CSRF/auth, input truncation, validation and write-failure preservation passed', flush=True)
                assert 'Wrong username or password' in Browser(address).login('missing', 'ordinary-password', status=200)
                assert 'Wrong username or password' in Browser(address).login('admin', 'wrong-password', status=200)
                admin.request(action='tech_admin', username='ordinary', admin='1', status=302)
                ordinary.login(' ORDINARY ', 'ordinary-password')
                assert ordinary.authorized(), 'Promoted technician could not use the console'
                admin.request(action='tech_admin', username='ordinary', admin='', status=302)
                assert not ordinary.authorized(), 'Demotion left an authorized browser session'
                admin.request(action='tech_admin', username='ordinary', admin='1', status=302)
                ordinary.login('ordinary', 'ordinary-password')
                admin.request(action='tech_signout', username='ordinary', status=302)
                assert not ordinary.authorized(), 'Sign out everywhere left an authorized browser session'
                ordinary.login('ordinary', 'ordinary-password')
                admin.request(action='tech_password', username='ordinary', password='replacement-password', status=302)
                assert not ordinary.authorized(), 'Password reset left an authorized browser session'
                ordinary.login('ordinary', 'replacement-password')
                admin.request(action='tech_delete', username='ordinary', status=302)
                assert not ordinary.authorized(), 'Deleted technician retained web access'
                admin.request(action='logout', status=302)
                assert not admin.authorized()
                admin.login('admin', 'original-station-password')
                print('Station admin: fresh setup, named login, role enforcement, CSRF, last-admin protection, demotion/reset/sign-out/deletion revocation passed', flush=True)
                admin.request(action='tech_add', username='layout-tech', name='Technician with a longer display name', password='layout-tech-password', status=302)
                if args.browser:
                    # A local TCP listener satisfies supervisor health without launching a relay.
                    cli(f"slip_station_save(['relay_port'=>{port}]);")
                    cli("foreach(glob(slip_data_dir().'/rate/*') as $file) unlink($file);")
                    inventory = {
                        'computer_name': 'CUSTOM-PC', 'model': 'Motherboard: ASUS PRIME X870', 'serial_number': '12345670012',
                        'motherboard': 'ASUS PRIME X870', 'motherboard_serial': 'BOARD-123456789',
                        'report': [
                            {'title': 'Computer', 'rows': [['Motherboard', 'ASUS PRIME X870'], ['Motherboard serial', 'BOARD-123456789']]},
                            {'title': 'Network', 'rows': [['IPv4 address', '192.0.2.42']]},
                        ],
                    }
                    device_id = cli("""
$endpoint=bin2hex(sodium_crypto_sign_publickey(sodium_crypto_sign_keypair()));
$id=slip_store()->register($endpoint,['name'=>'CUSTOM-PC','os'=>'Linux','app_version'=>'fixture'],json_encode(['id'=>$endpoint,'addrs'=>[]]));
slip_enroll_device($endpoint,$id,'CUSTOM-PC');
slip_station_group('create','Engineering');
slip_station_group('create','Lab');
slip_user_update('admin',function($a){$a['groups'][]='Admin personal';return $a;});
slip_shared_update(function($devices) use($id) { foreach($devices as &$d) if($d['id']===$id) $d=slip_set_device_groups($d,['Engineering','Lab']); return $devices; });
slip_set_group_assignment('','Engineering',['admin']);
$second=bin2hex(sodium_crypto_sign_publickey(sodium_crypto_sign_keypair()));
$secondId=slip_store()->register($second,['name'=>'SECOND-PC','os'=>'Linux','app_version'=>'fixture'],json_encode(['id'=>$second,'addrs'=>[]]));
slip_enroll_device($second,$secondId,'SECOND-PC');
foreach ([['Bench 10','HARDWARE-BENCH-10',['Zone 10','Zone 2']],['Bench 2','HARDWARE-2\"><img data-device-xss>',['Zone 10']],['UNASSIGNED-PC','UNASSIGNED-PC',[]]] as [$alias,$computer,$groups]) {
    $key=bin2hex(sodium_crypto_sign_publickey(sodium_crypto_sign_keypair()));
    $extraId=slip_store()->register($key,['name'=>$computer,'os'=>$alias==='Bench 2'?'Windows':'Linux','app_version'=>'fixture'],json_encode(['id'=>$key,'addrs'=>[]]));
    slip_enroll_device($key,$extraId,$computer);
    slip_shared_update(function($devices)use($extraId,$alias,$groups){foreach($devices as &$d)if($d['id']===$extraId){$d['name']=$alias;$d=slip_set_device_groups($d,$groups);}return $devices;});
    if ($alias==='Bench 10') slip_store()->unregister($key);
}
slip_inventory_save($endpoint,slip_clean_inventory(json_decode(stream_get_contents(STDIN))));
echo $id;
""", json.dumps(inventory))
                    (data / 'relay.log').write_text(
                        '2026-10-29T12:34:56.000Z INFO slipstream_relay: Relay client connected event="relay_client_connected" device_name="CUSTOM-PC" device_id=' + device_id + ' peer="192.0.2.42" endpoint=' + 'a' * 64 + '\n'
                        + '2026-10-29T12:35:56.000Z WARN slipstream_relay: Transport failed event="relay_transport_failed" reason="' + 'long_reason_' * 100 + '"\n'
                        + '2026-10-29T12:36:56.000Z INFO slipstream_relay: Relay client disconnected event="relay_client_disconnected" duration_ms=60000 bytes_from_client=2048 bytes_to_client=4096 device_name="<img data-xss-fixture>"\n')
                    subprocess.run(['node', str(root / 'scripts/test-station-layout.mjs'), address, device_id], check=True)
                # Migrate a legacy station with an existing unrelated "admin" technician.
                cli("slip_station_save(['admin_account'=>null,'admin_hash'=>password_hash('legacy-station-password', PASSWORD_DEFAULT)]); slip_user_update('admin',function($a){$a['admin']=false;return $a;});")
                cli("foreach(glob(slip_data_dir().'/rate/*') as $file) unlink($file);")
                legacy = Browser(address)
                assert 'station-admin' in legacy.request()
                assert not json.loads((data / 'accounts/admin.json').read_text())['admin'], 'Migration elevated a colliding technician'
                assert json.loads((data / 'station.json').read_text())['admin_account'] == 'station-admin'
                assert 'Wrong username or password' in legacy.request(action='login', password='legacy-station-password')
                legacy.login('station-admin', 'legacy-station-password')
                assert legacy.authorized()
                legacy.request(action='tech_admin', username='station-admin', admin='', status=302)
                assert 'Keep at least one' in legacy.request()
                assert json.loads(cli("echo json_encode(password_verify('original-station-password',slip_user_load('admin')['hash']));")), 'Migration replaced the colliding account password'
                print('Station admin: legacy hash migration, username collision, preserved technician password and named legacy sign-in passed', flush=True)
            finally:
                server.terminate()
                with contextlib.suppress(subprocess.TimeoutExpired):
                    server.wait(timeout=5)
                if server.poll() is None:
                    server.kill()
                    server.wait()


if __name__ == '__main__':
    main()
