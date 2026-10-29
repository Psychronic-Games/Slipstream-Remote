#!/usr/bin/env python3
"""Run a built relay and check production logging, redaction and SIGTERM shutdown."""
import argparse
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', required=True, type=Path)
    args = parser.parse_args()
    executable = str(args.server.resolve())
    for private in [False, True]:
        with tempfile.TemporaryDirectory(prefix='slipstream-relay-log-') as tmp:
            base = Path(tmp)
            with socket.socket() as sock:
                sock.bind(('127.0.0.1', 0))
                port = sock.getsockname()[1]
            env = {**os.environ, 'NO_COLOR': '1', 'RUST_LOG': 'warn,slipstream_server=info,slipstream_relay=info,iroh_relay::server::client=info'}
            env.pop('SLIPSTREAM_ACCESS_KEY', None)
            env.pop('SLIPSTREAM_STATION_DATA_DIR', None)
            secret = 'fixture-secret-must-never-appear-in-relay-log'
            if private:
                env['SLIPSTREAM_ACCESS_KEY'] = secret
                env['SLIPSTREAM_STATION_DATA_DIR'] = str(base / 'station')
            with (base / 'relay.log').open('wb') as output:
                process = subprocess.Popen([executable, '--relay-only', '--listen', f'127.0.0.1:{port}', '--data-dir', str(base / 'relay')], env=env, stdout=output, stderr=output)
                try:
                    address = f'http://127.0.0.1:{port}'
                    deadline = time.monotonic() + 15
                    while True:
                        try:
                            with urllib.request.urlopen(address + '/ping', timeout=.5) as response:
                                assert response.status == 200
                            break
                        except (OSError, urllib.error.URLError):
                            if process.poll() is not None or time.monotonic() > deadline:
                                raise RuntimeError('Relay did not become healthy')
                            time.sleep(.05)
                    with urllib.request.urlopen(address + '/generate_204', timeout=3) as response:
                        assert response.status == 204
                    request = urllib.request.Request(address + '/relay?token=' + secret, headers={'X-Forwarded-For': 'not-an-address', 'X-Slipstream-Relay-Tunnel': '99999999'})
                    try:
                        with urllib.request.urlopen(request, timeout=3) as response:
                            status = response.status
                    except urllib.error.HTTPError as error:
                        status = error.code
                        error.close()
                    assert status >= 400, 'Invalid WebSocket upgrade unexpectedly succeeded'
                    process.send_signal(signal.SIGTERM)
                    assert process.wait(timeout=8) == 0, 'SIGTERM did not stop the server cleanly'
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait(timeout=5)
            log = (base / 'relay.log').read_text()
            assert secret not in log, 'Relay log exposed the fixture access token'
            assert '99999999' not in log, 'Client-controlled correlation header reached the log'
            assert '\x1b' not in log and 'SLIPSTREAM SERVER ONLINE' not in log, 'File log contains terminal decoration'
            for event in ['server_started', 'relay_request_failed', 'websocket_upgrade_rejected', 'server_shutdown_requested', 'SIGTERM', 'server_stopped']:
                assert event in log, f'Missing diagnostic event: {event}'
            assert 'active_relay_connections=0' in log
            label = 'private' if private else 'public'
            print(f'{label} relay: health, specific upgrade failure, token/header redaction, plain file output and graceful SIGTERM passed', flush=True)


if __name__ == '__main__':
    main()
