#!/usr/bin/env python3
"""Takes the README's screenshots on private X displays, never the desktop.

  scripts/check.sh python3 scripts/readme-screenshots.py [--client target/debug/slipstream]

Writes docs/screenshots/{bridge,session}.png; the meeting pictures
come from `scripts/test-meetings-ui.py --showcase docs/screenshots`. The
session shows a browser on Slipstream Public's download page, hosted by a
second copy of the app, so nothing from this computer appears. Both copies
use throwaway profiles and get temporary IDs from Slipstream Public.
Needs Xvfb, xdotool, openbox (the cached QA tools have them) and Chromium.
"""
import argparse
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
from PIL import ImageGrab

STATION = 'https://public.slipstreamremote.com/'


def stop_bus_services():
    """Stops what the private bus started (Chromium wakes the desktop portal
    and KDE Connect): they'd outlive the run and hold its output open."""
    address = os.environ.get('DBUS_SESSION_BUS_ADDRESS', '').encode()
    if not address:
        return
    for proc in Path('/proc').iterdir():
        if not proc.name.isdigit() or int(proc.name) == os.getpid():
            continue
        try:
            if b'DBUS_SESSION_BUS_ADDRESS=' + address + b'\0' in (proc / 'environ').read_bytes():
                os.kill(int(proc.name), 15)
        except (OSError, ValueError):
            pass


def main():
    # Keep trays and portal requests off the user's session bus.
    if os.environ.get('SLIPSTREAM_QA_PRIVATE_BUS') != '1':
        runner = shutil.which('dbus-run-session')
        if not runner:
            raise RuntimeError('needs dbus-run-session')
        os.execvpe(runner, [runner, '--', sys.executable, str(Path(__file__).resolve()), *sys.argv[1:]], os.environ | {'SLIPSTREAM_QA_PRIVATE_BUS': '1'})
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--client', type=Path, default=Path('target/debug/slipstream'))
    parser.add_argument('--out', type=Path, default=Path('docs/screenshots'))
    args = parser.parse_args()
    client = str(args.client.resolve())
    cached = Path.home() / '.cache/slipstream-qa-tools'
    tool = lambda name: shutil.which(name) or str(cached / 'root/usr/bin' / name)
    chromium = shutil.which('chromium') or shutil.which('chromium-browser') or shutil.which('google-chrome')
    if not chromium:
        raise RuntimeError('needs Chromium')
    base = Path(tempfile.mkdtemp(prefix='slipstream-readme-'))
    children = []
    env = os.environ.copy()
    env.pop('WAYLAND_DISPLAY', None)
    env |= {'LIBGL_ALWAYS_SOFTWARE': '1', 'LD_LIBRARY_PATH': str(cached / 'root/usr/lib') + ':' + env.get('LD_LIBRARY_PATH', '')}

    def launch(command, name, extra):
        log = (base / f'{name}.log').open('w')
        child = subprocess.Popen(command, env=env | extra, stdout=log, stderr=subprocess.STDOUT)
        children.append(child)
        return child

    def wait(check, what, seconds=30):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if result := check():
                return result
            time.sleep(.2)
        raise RuntimeError(f'timed out: {what} (logs in {base})')

    def display(size):
        number = next(n for n in range(149, 199) if not Path(f'/tmp/.X11-unix/X{n}').exists() and n not in used)
        used.add(number)
        launch([tool('Xvfb'), f':{number}', '-screen', '0', f'{size}x24', '-nolisten', 'tcp', '-ac'], f'xvfb-{number}', {})
        wait(lambda: Path(f'/tmp/.X11-unix/X{number}').exists(), 'Xvfb')
        return f':{number}'

    def window(pid, x):
        found = subprocess.run([tool('xdotool'), 'search', '--onlyvisible', '--pid', str(pid)], env=env | {'DISPLAY': x}, capture_output=True, text=True)
        return next(iter(found.stdout.split()), None)

    def grab(x, size, name):
        w, h = map(int, size.split('x'))
        path = args.out / f'{name}.png'
        ImageGrab.grab(xdisplay=x).crop((0, 0, w, h)).save(path)
        print('wrote', path, flush=True)

    def app(x, size, name, *extra):
        home = base / name
        home.mkdir()
        process = launch([client, '--window-size', size, *extra], name, {'DISPLAY': x, 'SLIPSTREAM_HOME': str(home)})
        win = wait(lambda: window(process.pid, x), f'{name} window')
        subprocess.run([tool('xdotool'), 'windowmove', win, '0', '0'], env=env | {'DISPLAY': x}, check=True)
        return process

    used = set()
    args.out.mkdir(parents=True, exist_ok=True)
    try:
        # Bridge, from a fresh profile on Slipstream Public.
        size = '1280x800'
        x = display(size)
        app(x, size, 'bridge')
        time.sleep(12)  # station hello, ID and the opening animation
        grab(x, size, 'bridge')

        # A session: a browser on a "remote" display, hosted by a second copy.
        remote = display('1600x900')
        launch([tool('openbox')], 'openbox', {'DISPLAY': remote})
        launch([chromium, '--ozone-platform=x11', '--no-first-run', '--no-default-browser-check', '--disable-gpu', '--password-store=basic', f'--user-data-dir={base}/chromium-remote',
                '--window-position=0,0', '--window-size=1600,900', STATION], 'chromium', {'DISPLAY': remote, 'HOME': str(base)})
        password = 'readme-' + secrets.token_hex(6)
        home = base / 'host'
        home.mkdir()
        # Not this computer's own name.
        (home / 'settings.toml').write_text('device_name = "Front desk PC"\n')
        host = launch([client, 'host', '--password', password], 'host', {'DISPLAY': remote, 'SLIPSTREAM_HOME': str(home)})
        log = base / 'host.log'
        # "public   Online { id: DeviceId(521568892) }" with no station of its own.
        found = lambda: next((''.join(c for c in l if c.isdigit()) for l in log.read_text().splitlines() if 'Online' in l), None)
        host_id = wait(found, 'host ID', 60)
        time.sleep(4)  # the page finishes loading
        viewer = display(size)
        app(viewer, size, 'viewer', '--connect', host_id, '--password', password)
        time.sleep(15)  # link, first frames, settle
        grab(viewer, size, 'session')
        if host.poll() is not None:
            raise RuntimeError(f'host exited (logs in {base})')
    finally:
        for child in reversed(children):
            child.terminate()
        for child in children:
            try:
                child.wait(10)
            except subprocess.TimeoutExpired:
                child.kill()
        stop_bus_services()
    shutil.rmtree(base, ignore_errors=True)


if __name__ == '__main__':
    main()
