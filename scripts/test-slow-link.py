#!/usr/bin/env python3
"""Measures a session over emulated slow, lossy links (no root needed).

  scripts/check.sh python3 scripts/test-slow-link.py [--seconds 30] [--hardware] [--bin target/debug]

Each run gets a private network namespace (`unshare -rn`) whose loopback is
shaped with netem: a local directory server, a test-pattern host and a
headless viewer (`slipstream connect`) all talk through it. The host shows
a scrolling page of text (SLIPSTREAM_TEST_BUSY), encoded on the processor
unless --hardware. Prints, per link: seconds to the first picture, frames a
second after it, seconds frozen, the round trip, and whether the session
dropped. Needs `unshare`, `tc` and `ip` (iproute2); netem shapes both
directions on loopback together.
"""
import argparse
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

LINKS = [
    ('LAN', ''),
    ('20 Mbit/s, 30 ms', 'delay 15ms rate 20mbit limit 500'),
    ('2 Mbit/s, 80 ms, 0.5% loss', 'delay 40ms 5ms loss 0.5% rate 2mbit limit 150'),
    ('1 Mbit/s, 200 ms, 2% loss', 'delay 100ms 20ms loss 2% rate 1mbit limit 100'),
    ('600 kbit/s, 250 ms, 1% loss', 'delay 125ms 10ms loss 1% rate 600kbit limit 100'),
]


def run_inside(bin_dir, seconds, shape, out):
    """In the namespace: server, host, viewer; shaping starts once the host is online."""
    app, server = str(bin_dir / 'slipstream'), str(bin_dir / 'slipstream-server')
    base = Path(tempfile.mkdtemp(prefix='slip-link-'))
    env = {k: v for k, v in os.environ.items() if k not in ('DISPLAY', 'WAYLAND_DISPLAY')}
    subprocess.run(['ip', 'link', 'set', 'lo', 'up'], check=True)
    procs = []
    try:
        addr = 'http://127.0.0.1:21217'
        procs.append(subprocess.Popen([server, '--data-dir', str(base / 'server'), '--listen', '127.0.0.1:21217'],
                                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env))
        for _ in range(100):
            try:
                urllib.request.urlopen(addr + '/', timeout=1)
                break
            except OSError:
                time.sleep(.1)
        for who in ('host', 'viewer'):
            subprocess.run([app, 'set-server', addr], env=env | {'SLIPSTREAM_HOME': str(base / who)},
                           check=True, capture_output=True)
        host_env = env | {'SLIPSTREAM_HOME': str(base / 'host'), 'SLIPSTREAM_TEST_BUSY': '1'}
        procs.append(subprocess.Popen([app, 'host', '--test-pattern', '--password', 'slow-link-password'],
                                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=host_env))
        device = None
        for _ in range(200):
            info = subprocess.run([app, 'info'], env=host_env, capture_output=True, text=True).stdout
            m = re.search(r'device id\s*:\s*([0-9 ]{9,11})', info)
            if m and m.group(1).strip():
                device = m.group(1).replace(' ', '')
                break
            time.sleep(.2)
        if not device:
            raise SystemExit('the host never registered')
        if shape:
            subprocess.run(['tc', 'qdisc', 'add', 'dev', 'lo', 'root', 'netem', *shape.split()], check=True)
        r = subprocess.run([app, 'connect', '--test-pattern', device, '--password', 'slow-link-password',
                            '--seconds', str(seconds)], env=env | {'SLIPSTREAM_HOME': str(base / 'viewer')},
                           capture_output=True, text=True, timeout=seconds + 120)
        Path(out).write_text(r.stdout)
    finally:
        for p in reversed(procs):
            p.terminate()
            try:
                p.wait(5)
            except subprocess.TimeoutExpired:
                p.kill()


def summary(text):
    rows = re.findall(r'^\s*\d+x\d+\s+([\d.]+) fps\s+\d+ kbps\s+rtt\s+([\d.]+) ms', text, re.M)
    fps = [float(f) for f, _ in rows]
    rtt = [float(r) for _, r in rows]
    first = next((i for i, f in enumerate(fps) if f > 0), None)
    after = fps[first:] if first is not None else []
    return {
        'first': '-' if first is None else f'{first} s',
        'fps': sum(after) / max(len(after), 1),
        'frozen': sum(1 for f in after if f == 0),
        'rtt': sum(rtt) / max(len(rtt), 1),
        'dropped': 'disconnected' in text,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--seconds', type=int, default=30)
    parser.add_argument('--hardware', action='store_true', help='let the host encode on the graphics card')
    parser.add_argument('--bin', type=Path, default=Path(__file__).resolve().parent.parent / 'target/debug')
    parser.add_argument('--inside', nargs=2, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.inside:
        run_inside(args.bin.resolve(), args.seconds, args.inside[0], args.inside[1])
        return
    if not args.hardware:
        os.environ['SLIPSTREAM_NO_HWENC'] = '1'
    print(f'{"link":30} {"first picture":>14} {"fps":>6} {"frozen":>7} {"rtt":>8}')
    for name, shape in LINKS:
        with tempfile.NamedTemporaryFile('r', suffix='.txt') as out:
            subprocess.run(['unshare', '-rn', sys.executable, __file__, '--seconds', str(args.seconds),
                            '--bin', str(args.bin), '--inside', shape, out.name], check=True)
            s = summary(Path(out.name).read_text())
        print(f'{name:30} {s["first"]:>14} {s["fps"]:6.1f} {s["frozen"]:5} s {s["rtt"]:6.0f} ms'
              + ('  DROPPED' if s['dropped'] else ''), flush=True)


if __name__ == '__main__':
    main()
