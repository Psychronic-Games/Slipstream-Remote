#!/usr/bin/env python3
"""Native two-client call/camera/chat audit with an isolated PHP station and X display.

Run through scripts/check.sh. Uses --test-pattern for camera frames. Requires
Pillow, tesseract, Xvfb, xdotool and a PHP CLI with sodium. The workstation's
cached QA tools are supported without installing packages or changing profiles.
"""
import argparse
import csv
import hashlib
import hmac
import io
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import tomllib
import urllib.request
from PIL import ImageGrab, Image, ImageOps


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path, default=Path('target/debug/slipstream'))
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    cached = Path.home() / '.cache/slipstream-qa-tools'
    tools = cached / 'root/usr/bin'
    php = os.environ.get('SLIPSTREAM_TEST_PHP') or shutil.which('php') or str(cached / 'php')
    xvfb = shutil.which('Xvfb') or str(tools / 'Xvfb')
    xdotool = shutil.which('xdotool') or str(tools / 'xdotool')
    base = Path(tempfile.mkdtemp(prefix='slipstream-meetings-ui-'))
    print(f'Evidence: {base}', flush=True)
    children = []
    logs = []
    env = os.environ.copy()
    env.pop('WAYLAND_DISPLAY', None)
    env['LIBGL_ALWAYS_SOFTWARE'] = '1'
    env['LD_LIBRARY_PATH'] = str(cached / 'root/usr/lib') + ':' + env.get('LD_LIBRARY_PATH', '')

    def launch(command, name, environment=env):
        log = (base / f'{name}.log').open('w')
        logs.append(log)
        child = subprocess.Popen(command, env=environment, stdout=log, stderr=log)
        children.append(child)
        return child

    def wait(check, description, seconds=20):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            result = check()
            if result:
                return result
            time.sleep(.1)
        raise RuntimeError(f'Timed out: {description}')

    def xdo(*arguments):
        return subprocess.run([xdotool, *map(str, arguments)], env=env, check=True, capture_output=True, text=True).stdout.strip()

    def window(pid):
        found = subprocess.run([xdotool, 'search', '--onlyvisible', '--pid', str(pid)], env=env, capture_output=True, text=True)
        return next(iter(found.stdout.split()), None)

    def geometry(win):
        return {key: int(value) for key, value in (line.split('=', 1) for line in xdo('getwindowgeometry', '--shell', win).splitlines())}

    def words(win, name):
        g = geometry(win)
        image = ImageGrab.grab(xdisplay=env['DISPLAY']).crop((g['X'], g['Y'], g['X'] + g['WIDTH'], g['Y'] + g['HEIGHT']))
        path = base / f'{name}.png'
        image.save(path)
        result = subprocess.run(['tesseract', str(path), 'stdout', '--psm', '11', 'tsv'], capture_output=True, text=True, check=True)
        return [row for row in csv.DictReader(io.StringIO(result.stdout), delimiter='\t') if row.get('text', '').strip()]

    def locate(win, phrase, name, last=False, controls=False):
        rows = words(win, name)
        normalize = lambda text: ''.join(c for c in text.upper() if c.isalnum())
        wanted = normalize(phrase)
        matches = []
        for index in range(0 if controls else len(rows)):
          for count in range(1,min(12,len(rows)-index)+1):
            group = rows[index:index + count]
            if ''.join(normalize(row['text']) for row in group) == wanted:
                left = min(int(row['left']) for row in group)
                top = min(int(row['top']) for row in group)
                right = max(int(row['left']) + int(row['width']) for row in group)
                bottom = max(int(row['top']) + int(row['height']) for row in group)
                matches.append(((left + right) // 2, (top + bottom) // 2))
        if not matches:
            # Antonio's narrow capitals are readable in isolated button crops.
            # Detect flat colored controls without relying on their coordinates.
            image=Image.open(base / f'{name}.png').convert('RGB')
            palette={color for count,color in sorted(image.getcolors(image.width*image.height),reverse=True)[:40]
                     if count>100 and max(color)>120 and max(color)-min(color)>45}
            small=image.resize((image.width//3,image.height//3),Image.Resampling.NEAREST)
            pixels=small.load();visited=set()
            for y in range(small.height):
              for x in range(small.width):
                color=pixels[x,y]
                if color not in palette or (x,y) in visited: continue
                queue=[(x,y)];visited.add((x,y));left=right=x;top=bottom=y
                while queue:
                    a,b=queue.pop();left=min(left,a);right=max(right,a);top=min(top,b);bottom=max(bottom,b)
                    for nx,ny in [(a-1,b),(a+1,b),(a,b-1),(a,b+1)]:
                        if 0<=nx<small.width and 0<=ny<small.height and (nx,ny) not in visited and pixels[nx,ny]==color:
                            visited.add((nx,ny));queue.append((nx,ny))
                left*=3;top*=3;right=(right+1)*3;bottom=(bottom+1)*3
                if right-left<35 or not 15<=bottom-top<=42: continue
                crop=image.crop((left+12,top+4,right-12,bottom-4))
                crop=ImageOps.grayscale(crop).resize((crop.width*5,crop.height*5)).point(lambda p:255 if p>100 else 0)
                crop=ImageOps.expand(crop,border=20,fill=255);path=base/'button-ocr.png';crop.save(path)
                text=subprocess.run(['tesseract',str(path),'stdout','--psm','6'],capture_output=True,text=True,check=True).stdout
                observed=normalize(text)
                if len(observed)==len(wanted):
                    observed=''.join(expected if expected.isdigit() and character in {'I','L','O','Z'} and {'I':'1','L':'1','O':'0','Z':'2'}[character]==expected else character for character,expected in zip(observed,wanted))
                if observed.replace("G","C")==wanted.replace("G","C"): matches.append(((left+right)//2,(top+bottom)//2))
        return matches[-1 if last else 0] if matches else None

    def click(win, phrase, name, last=False):
        point = wait(lambda: locate(win, phrase, name, last, controls=True), f'{name}: {phrase}')
        g = geometry(win)
        xdo('windowfocus', win)
        xdo('mousemove', g['X'] + point[0], g['Y'] + point[1], 'click', '1')
        time.sleep(.25)
        return point

    def camera_visible(win,name):
        g=geometry(win)
        image=ImageGrab.grab(xdisplay=env['DISPLAY']).crop((g['X'],g['Y'],g['X']+g['WIDTH'],g['Y']+g['HEIGHT']))
        image.save(base/f'{name}.png')
        video=image.crop((200,360,min(980,image.width),min(920,image.height))).convert('RGB').resize((200,160))
        return sum(r>200 and green>90 and b<80 for r,green,b in video.getdata())>50

    try:
        # The fixture key authorizes the directory, while its lease authorizes
        # only relay access. Neither fixture data nor accounts touch production.
        web = base / 'web'
        shutil.copytree(root / 'server/station', web)
        data = base / 'data'
        (web / 'config.php').write_text('<?php return ["data_dir"=>' + repr(str(data)) + '];\n')
        subprocess.run([php, '-r', "require $argv[1]; slip_station_save(['private'=>true,'access_key'=>'fixture-meeting-access','org'=>'Meeting fixture','public_relays'=>false,'admin_hash'=>'disabled']);", str(web / 'lib/slipstream.php')], check=True, capture_output=True)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        launch([php, '-S', f'127.0.0.1:{port}', '-t', str(web)], 'station')
        def api(body):
            request = urllib.request.Request(f'http://127.0.0.1:{port}/api/', data=json.dumps(body).encode(), headers={'Content-Type': 'application/json'})
            with urllib.request.urlopen(request, timeout=5) as response:
                return json.load(response)
        def started():
            try:
                return api({'op': 'hello', 'key': 'fixture-meeting-access'})
            except OSError:
                return None
        wait(started, 'PHP fixture')
        token = api({'op': 'meeting_relay', 'key': 'fixture-meeting-access'})['token']
        body, signature = token.rsplit('.', 1)
        assert hmac.compare_digest(signature, hmac.new(b'fixture-meeting-access', body.encode(), hashlib.sha256).hexdigest())
        try:
            api({'op': 'lookup', 'id': 123456789, 'key': token})
            raise AssertionError('Relay lease authorized directory access')
        except urllib.error.HTTPError as error:
            assert error.code == 403
        print('PHP relay lease signature and directory isolation passed', flush=True)
        # Find a free isolated display; never attach automation to the user's desktop.
        display = next(number for number in range(149, 180) if not Path(f'/tmp/.X11-unix/X{number}').exists())
        env['DISPLAY'] = f':{display}'
        launch([xvfb, env['DISPLAY'], '-screen', '0', '2400x1200x24', '-nolisten', 'tcp', '-ac'], 'xvfb')
        wait(lambda: Path(f'/tmp/.X11-unix/X{display}').exists(), 'Xvfb')
        windows = []
        profiles = []
        for index, name in enumerate(['Technician fixture', 'Remote user fixture']):
            profile = base / f'profile-{index}'
            profile.mkdir()
            profiles.append(profile)
            (profile / 'settings.toml').write_text(f'device_name = {json.dumps(name)}\n[server]\nurl = "http://127.0.0.1:{port}"\nkey = "fixture-meeting-access"\nuse_public_relays = false\n')
            local = env | {'SLIPSTREAM_HOME': str(profile), 'SLIPSTREAM_TEST_SCREENS':'2'}
            process = launch([str(args.client.resolve()), '--test-pattern', '--page', 'meetings', '--window-size', '1140x1100'], f'client-{index}', local)
            win = wait(lambda: window(process.pid), f'client {index} window')
            xdo('windowmove', win, index * 1200, 0)
            windows.append(win)
        host, guest = windows
        def device_id():
            try:
                return tomllib.loads((profiles[1] / 'settings.toml').read_text()).get('host', {}).get('last_device_id')
            except (OSError, ValueError):
                return None
        target = wait(device_id, 'remote device ID')
        def click_text(win, phrase, name, last=False):
            point=wait(lambda:locate(win,phrase,name,last),f'{name}: {phrase}')
            g=geometry(win);xdo('windowfocus',win);xdo('mousemove',g['X']+point[0],g['Y']+point[1],'click','1');time.sleep(.25)
            return point
        click(host, 'Devices', 'host-setup-devices')
        click_text(host,'Test camera','host-camera-picker')
        click_text(host,'Test camera 2','host-second-camera')
        wait(lambda:tomllib.loads((profiles[0]/'settings.toml').read_text()).get('app',{}).get('meeting_camera')==1,'remember selected camera')
        wait(lambda:locate(host,'System default','host-microphone-name'),'named default microphone')
        click(host,'Start or join','host-back-to-setup')
        click(host, 'Create meeting', 'host-create')
        click(host,'Participants 1','host-people')
        point = wait(lambda: locate(host, 'System', 'host-room',last=True), 'call form')
        g = geometry(host)
        xdo('mousemove', g['X'] + point[0] + 125, g['Y'] + point[1], 'click', '1')
        xdo('type', '--clearmodifiers', '--delay', '0', str(target))
        click(host, 'Call system', 'host-calling-first')
        wait(lambda:locate(guest,'Answer','guest-ringing',controls=True),'incoming call prompt')
        click(host,'Cancel call','host-cancel-call')
        wait(lambda:not locate(guest,'Answer','guest-cancelled',controls=True),'cancel removed incoming prompt')
        time.sleep(5.1)
        click(host, 'Call system', 'host-calling')
        click(guest, 'Answer', 'guest-incoming', last=True)
        wait(lambda:locate(host,'Participants 2','host-answered',controls=True),'answered peer joining')
        assert not locate(host,'Close call','host-no-close-call',controls=True)
        assert not locate(host,'Cancel call','host-no-cancel-call',controls=True)
        click(host,'Live meeting','host-live')
        click(host, 'Camera', 'host-camera')
        wait(lambda: camera_visible(host,'host-preview'), 'local decoded camera preview')
        wait(lambda: camera_visible(guest,'guest-video'), 'remote decoded camera tile')
        wait(lambda:locate(host,'Test camera 2','host-selected-camera'),'selected camera in sharing status')
        print('Named camera choice/persistence, cancelled call, Answer, call-status cleanup and live camera passed',flush=True)
        click(host,'Chat','host-chat-tab')
        point = wait(lambda: locate(host, 'Message everyone', 'host-chat'), 'meeting message field')
        g=geometry(host)
        xdo('mousemove', g['X'] + point[0] + 100, g['Y'] + point[1], 'click', '1')
        xdo('type', '--clearmodifiers', '--delay', '0', 'Meeting fixture message')
        xdo('key','Return')
        click(guest,'Chat 1 new','guest-unread-chat')
        wait(lambda:locate(guest,'Meeting fixture message','guest-chat'),'group Subspace chat')
        click(host, 'Stop camera', 'host-stopping-camera')
        click(guest,'Live meeting','guest-live-again')
        wait(lambda:locate(guest,'Cameras','guest-camera-stopped'),'camera stopped on peer')
        click(host,'Share screen','host-screen-chooser-cancel')
        click_text(host,'Screen 2 Test Pattern 2 1280 x 720','host-choose-screen-cancel')
        click(host,'Cancel','host-cancel-screen')
        assert locate(host,'Share screen','host-cancel-kept-off',controls=True)
        click(host,'Share screen','host-screen-chooser')
        click_text(host,'Screen 2 Test Pattern 2 1280 x 720','host-choose-screen-two')
        click(host,'Share selected screen','host-confirm-screen')
        wait(lambda:locate(host,'Test Pattern 2','host-screen-two-shared'),'explicit second screen sharing status')
        wait(lambda:camera_visible(guest,'guest-screen-video'),'selected screen reaches peer')
        xdo('windowsize',guest,480,800)
        time.sleep(.5);words(guest,'guest-narrow-live')
        click(host,'Stop sharing','host-stop-screen')
        click(host,'End meeting','host-end-confirmation')
        click(host,'Keep meeting open','host-cancel-end')
        assert locate(guest,'Unmute','guest-still-connected',controls=True)
        click(host,'End meeting','host-end-again')
        click(host,'End for everyone','host-ended')
        wait(lambda:locate(guest,'Close meeting','guest-ended',controls=True),'organizer ending the room')
        print('Separate Chat/unread count, persistent media controls, screen choice/Cancel, narrow live view and confirmed teardown passed',flush=True)
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
        for child in reversed(children):
            try:
                child.wait(timeout=8)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        for log in logs:
            log.close()


if __name__ == '__main__':
    main()
