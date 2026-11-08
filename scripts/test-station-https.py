#!/usr/bin/env python3
"""Station HTTPS enforcement (slip_enforce_https in lib/slipstream.php).

Runs a copy of the station on PHP's built-in server (plain HTTP) and plays
HTTPS through X-Forwarded-Proto, as a TLS-terminating proxy or host would:

- before the station has been reached over HTTPS, HTTP keeps working;
- an HTTPS request records the name; HSTS only with 'hsts' => true (it
  would stop people clicking past a certificate warning);
- then HTTP for that name redirects to HTTPS (301 for GET, 308 for POST,
  keeping the path and dropping a port), other names are left alone;
- 'force_https' => false turns it off.

    scripts/test-station-https.py
"""

import http.client
import json
import os
import shutil
import socket
import subprocess
import tempfile
import time
from pathlib import Path


def request(port, host, path='/', method='GET', https=False, body=None):
    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
    headers = {'Host': host}
    if https:
        headers['X-Forwarded-Proto'] = 'https'
    if body is not None:
        headers['Content-Type'] = 'application/json'
    conn.request(method, path, body=body, headers=headers)
    response = conn.getresponse()
    response.read()
    conn.close()
    return response.status, dict(response.getheaders())


def serve(php, web):
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    server = subprocess.Popen([php, '-S', f'127.0.0.1:{port}', '-t', str(web)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(100):
        try:
            with socket.create_connection(('127.0.0.1', port), timeout=.1):
                return server, port
        except OSError:
            time.sleep(.05)
    server.kill()
    raise RuntimeError('PHP fixture failed to start')


def main():
    root = Path(__file__).resolve().parent.parent
    php = os.environ.get('SLIPSTREAM_TEST_PHP') or shutil.which('php') or str(Path.home() / '.cache/slipstream-qa-tools/php')
    with tempfile.TemporaryDirectory(prefix='slipstream-https-test-') as temp:
        web, data = Path(temp) / 'web', Path(temp) / 'data'
        shutil.copytree(root / 'server/station', web)
        config = web / 'config.php'
        config.write_text('<?php return ["data_dir" => ' + repr(str(data)) + '];\n')
        server, port = serve(php, web)
        try:
            # No HTTPS seen yet: plain HTTP works (a station without a certificate).
            status, headers = request(port, 'station.test')
            assert status != 301 and 'Strict-Transport-Security' not in headers, (status, headers)

            # Reached over HTTPS: the name is recorded; no HSTS unless asked for.
            status, headers = request(port, 'station.test', https=True)
            assert 'Strict-Transport-Security' not in headers, headers
            assert json.loads((data / 'https-hosts.json').read_text()) == ['station.test']

            # Now HTTP for that name goes to HTTPS, path kept, port dropped.
            status, headers = request(port, 'station.test', '/download.php?os=linux')
            assert status == 301 and headers['Location'] == 'https://station.test/download.php?os=linux', (status, headers)
            status, headers = request(port, 'Station.Test:8080', '/admin/')
            assert status == 301 and headers['Location'] == 'https://station.test/admin/', (status, headers)
            # An app's POST keeps its method and body (308).
            status, headers = request(port, 'station.test', '/api/', 'POST', body=json.dumps({'op': 'hello'}))
            assert status == 308 and headers['Location'] == 'https://station.test/api/', (status, headers)
            # Names never reached over HTTPS are left alone.
            status, headers = request(port, 'other.test')
            assert status != 301, (status, headers)
        finally:
            server.kill()
            server.wait()

        # HSTS for a station with a proper certificate.
        config.write_text('<?php return ["data_dir" => ' + repr(str(data)) + ', "hsts" => true];\n')
        server, port = serve(php, web)
        try:
            status, headers = request(port, 'station.test', https=True)
            assert headers.get('Strict-Transport-Security') == 'max-age=31536000', headers
        finally:
            server.kill()
            server.wait()

        # Switched off for a proxy that doesn't say it terminated HTTPS.
        config.write_text('<?php return ["data_dir" => ' + repr(str(data)) + ', "force_https" => false];\n')
        server, port = serve(php, web)
        try:
            status, headers = request(port, 'station.test')
            assert status != 301, (status, headers)
        finally:
            server.kill()
            server.wait()
    print('Station HTTPS: HTTP kept before HTTPS works, then redirected (301/308, port dropped); HSTS only with hsts=true; other names and force_https=false left alone')


if __name__ == '__main__':
    main()
