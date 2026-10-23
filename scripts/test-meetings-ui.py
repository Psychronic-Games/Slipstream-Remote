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
import sys
import tempfile
import time
import tomllib
import urllib.request
from PIL import ImageGrab, Image, ImageOps


def main():
    # Xvfb alone does not isolate KDE's StatusNotifierWatcher or desktop portals.
    # Never register fixture trays or issue portal requests on the user's bus.
    if os.environ.get('SLIPSTREAM_QA_PRIVATE_BUS')!='1':
        runner=shutil.which('dbus-run-session')
        if not runner: raise RuntimeError('Native UI checks require dbus-run-session for desktop isolation')
        os.execvpe(runner,[runner,'--',sys.executable,str(Path(__file__).resolve()),*sys.argv[1:]],os.environ|{'SLIPSTREAM_QA_PRIVATE_BUS':'1'})
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path, default=Path('target/debug/slipstream'))
    parser.add_argument('--tabs', action='store_true', help='Audit native tab groups instead of meetings')
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

    def locate(win, phrase, name, last=False, controls=False, prefix=False):
        rows = words(win, name)
        normalize = lambda text: ''.join(c for c in text.upper() if c.isalnum())
        wanted = normalize(phrase)
        matches = []
        for index in range(0 if controls else len(rows)):
          for count in range(1,min(12,len(rows)-index)+1):
            group = rows[index:index + count]
            joined=''.join(normalize(row['text']) for row in group)
            # prefix: OCR runs a trailing number into a word ("Testcameral").
            if joined == wanted or (prefix and count == 1 and joined.startswith(wanted)):
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
                # Tight inset: 0.26.2's compact actions leave little padding around the label.
                crop=image.crop((left+6,top+4,right-6,bottom-4))
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

    def click_point(win,x,y,name):
        words(win,name)
        g=geometry(win);xdo('windowfocus',win);xdo('mousemove',g['X']+x,g['Y']+y,'click','1');time.sleep(.3)

    def camera_visible(win,name,preview=False):
        g=geometry(win)
        image=ImageGrab.grab(xdisplay=env['DISPLAY']).crop((g['X'],g['Y'],g['X']+g['WIDTH'],g['Y']+g['HEIGHT']))
        image.save(base/f'{name}.png')
        if preview:
            video=image.crop((380,350,min(800,image.width),min(780,image.height))).convert('RGB')
            pixels=getattr(video,'get_flattened_data',video.getdata)()
            return sum(r<45 and 35<green<140 and 45<b<180 and b>green for r,green,b in pixels)>100
        # Anywhere on the meeting stage (below the room's header): the test
        # camera's orange bar may be in a small strip tile.
        video=image.crop((0,200,image.width,image.height)).convert('RGB')
        video=video.resize((video.width//4,video.height//4))
        return sum(r>200 and 90<green<180 and b<80 for r,green,b in getattr(video,'get_flattened_data',video.getdata)())>20

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
        for index, name in enumerate(['Alpha', 'Beta'] if args.tabs else ['Technician fixture', 'Remote user fixture']):
            profile = base / f'profile-{index}'
            profile.mkdir()
            profiles.append(profile)
            (profile / 'settings.toml').write_text(f'device_name = {json.dumps(name)}\n[server]\nurl = "http://127.0.0.1:{port}"\nkey = "fixture-meeting-access"\nuse_public_relays = false\n')
            local = env | {'SLIPSTREAM_HOME': str(profile), 'SLIPSTREAM_TEST_SCREENS':'2'}
            if args.tabs:
                subprocess.run([str(args.client.resolve()),'set-password','fixture-tab-password'],env=local,check=True,capture_output=True)
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
        def click_text(win, phrase, name, last=False, prefix=False):
            point=wait(lambda:locate(win,phrase,name,last,prefix=prefix),f'{name}: {phrase}')
            g=geometry(win);xdo('windowfocus',win);xdo('mousemove',g['X']+point[0],g['Y']+point[1],'click','1');time.sleep(.25)
            return point
        if args.tabs:
            for win in windows: xdo('windowunmap',win)
            first=wait(lambda:tomllib.loads((profiles[0]/'settings.toml').read_text()).get('host',{}).get('last_device_id'),'first host ID')
            profile=base/'profile-viewer';profile.mkdir()
            (profile/'settings.toml').write_text(f'device_name = "Viewer"\n[server]\nurl = "http://127.0.0.1:{port}"\nkey = "fixture-meeting-access"\nuse_public_relays = false\n[viewer]\nseparate_windows = true\nplay_audio = false\n')
            viewer=launch([str(args.client.resolve()),'--test-pattern','--connect',str(first),'--password','fixture-tab-password','--window-size','1140x1100'],'viewer',env|{'SLIPSTREAM_HOME':str(profile)})
            def viewer_windows():
                result=subprocess.run([xdotool,'search','--onlyvisible','--pid',str(viewer.pid)],env=env,capture_output=True,text=True)
                return result.stdout.split()
            root_window=wait(lambda:next((w for w in viewer_windows() if xdo('getwindowname',w).startswith('Slipstream')),None),'viewer root')
            floating=wait(lambda:next((w for w in viewer_windows() if xdo('getwindowname',w).startswith('Alpha')),None),'first floating group')
            xdo('windowmove',root_window,0,0);xdo('windowmove',floating,1200,0)
            def local_click(win,x,y):
                g=geometry(win);xdo('windowfocus',win);xdo('mousemove',g['X']+x,g['Y']+y,'click','1');time.sleep(.3)
            def drag(win,x,y,to_x,to_y):
                g=geometry(win);xdo('windowfocus',win);xdo('mousemove',g['X']+x,g['Y']+y,'mousedown','1');time.sleep(.25)
                xdo('mousemove',g['X']+x+16,g['Y']+y);time.sleep(.25)
                for step in range(1,13):
                    xdo('mousemove',round(g['X']+x+(to_x-g['X']-x)*step/12),round(g['Y']+y+(to_y-g['Y']-y)*step/12));time.sleep(.05)
                time.sleep(.25);words(win,'drag-in-progress');xdo('mouseup','1');time.sleep(.8)
            # One tab (180px) followed by + in the floating strip below titlebar.
            words(floating,'group-one-before-plus')
            local_click(floating,204,56)
            click_text(root_window,'LINK BY ID','new-link-manual')
            station=wait(lambda:locate(root_window,'This station','new-link-fields'),'manual station selector')
            local_click(root_window,station[0],station[1]+40)
            xdo('type','--clearmodifiers','--delay','60',str(target));time.sleep(.5)
            local_click(root_window,station[0]+225,station[1]+40)
            xdo('type','--clearmodifiers','--delay','60','fixture-tab-password');time.sleep(.5)
            local_click(root_window,station[0]+430,station[1]+40)
            words(root_window,'after-manual-connect')
            wait(lambda:xdo('getwindowname',floating).startswith('Beta'),'second link in original floating group')
            assert len(viewer_windows())==2,viewer_windows()
            words(floating,'group-two-tabs')
            print('Floating + retained destination despite separate_windows=true; two live tabs share one native window',flush=True)
            # Tear Beta out, then move it back into Alpha's group.
            drag(floating,250,56,720,450)
            other=wait(lambda:next((w for w in viewer_windows() if w not in {root_window,floating}),None),'tear-out from floating group')
            words(other,'torn-out-tab')
            xdo('windowmove',other,0,0)
            drag(other,65,56,1500,56)
            wait(lambda:len(viewer_windows())==2,'floating-to-floating merge closes empty source window')
            words(floating,'merged-floating-group')
            assert xdo('getwindowname',floating).startswith('Beta')
            # Put Beta in the root strip, then drag it back into the group.
            drag(floating,250,56,250,56)
            wait(lambda:xdo('getwindowname',floating).startswith('Alpha'),'floating tab docked to root')
            words(root_window,'root-docked')
            drag(root_window,160,56,1500,56)
            words(floating,'root-to-floating-group')
            assert len(viewer_windows())==2
            # Closing one selected tab leaves its sibling connected.
            local_click(floating,350,56)
            wait(lambda:xdo('getwindowname',floating).startswith('Alpha'),'closing selected tab retains other group member')
            words(floating,'remaining-group-member')
            print('Floating tear-out, floating-to-floating merge, root/group moves and closing one member passed',flush=True)
            return
        click(host, 'Devices', 'host-setup-devices')
        click_text(host,'Test camera','host-camera-picker',prefix=True)
        click_text(host,'Test camera 2','host-second-camera')
        wait(lambda:tomllib.loads((profiles[0]/'settings.toml').read_text()).get('app',{}).get('meeting_camera')==1,'remember selected camera')
        wait(lambda:locate(host,'System default','host-microphone-name'),'named default microphone')
        click(host,'Start or join','host-back-to-setup')
        click(host, 'Create meeting', 'host-create')
        click(host,'People 1','host-people')
        # The ID field's faint hint defeats OCR: People column, 1140x1100 fixture.
        click_point(host,960,293,'host-room')
        xdo('type', '--clearmodifiers', '--delay', '0', str(target))
        click(host, 'Call system', 'host-calling-first')
        wait(lambda:locate(guest,'Answer','guest-ringing',controls=True),'incoming call prompt')
        click(host,'Cancel call','host-cancel-call')
        wait(lambda:not locate(guest,'Answer','guest-cancelled',controls=True),'cancel removed incoming prompt')
        time.sleep(5.1)
        click(host, 'Call system', 'host-calling')
        click(guest, 'Answer', 'guest-incoming', last=True)
        wait(lambda:locate(host,'People 2','host-answered',controls=True),'answered peer joining')
        assert not locate(host,'Close call','host-no-close-call',controls=True)
        assert not locate(host,'Cancel call','host-no-cancel-call',controls=True)
        click(host,'People 2','host-live')
        click(host, 'Camera', 'host-camera')
        wait(lambda: camera_visible(host,'host-preview'), 'local decoded camera preview')
        wait(lambda: camera_visible(guest,'guest-video'), 'remote decoded camera tile')
        wait(lambda:locate(host,'Test camera 2','host-selected-camera'),'selected camera in sharing status')
        print('Named camera choice/persistence, cancelled call, Answer, call-status cleanup and live camera passed',flush=True)
        click(host,'Chat','host-chat-column')
        # The message box sits at the foot of the chat column (1140x1100 fixture).
        click_point(host,960,1018,'host-chat')
        xdo('type', '--clearmodifiers', '--delay', '0', 'Meeting fixture message')
        xdo('key','Return')
        click(guest,'Chat 1','guest-unread-chat')
        wait(lambda:locate(guest,'Meeting fixture message','guest-chat'),'group Subspace chat')
        # React to the message (the ☺+ under it, then the first reaction).
        click_point(guest,820,306,'guest-react-menu')
        click_point(guest,828,336,'guest-react-chosen')
        time.sleep(1);words(guest,'guest-reacted')
        # The organizer's chat column is still open from sending.
        time.sleep(.5);words(host,'host-sees-reaction')
        click(host, 'Stop camera', 'host-stopping-camera')
        click(guest,'Chat','guest-live-again')
        wait(lambda:not camera_visible(guest,'guest-camera-stopped'),'camera stopped on peer')
        click(host,'Share screen','host-screen-chooser-cancel')
        wait(lambda:camera_visible(host,'host-screen-thumbnails',preview=True),'local chooser thumbnails')
        assert not camera_visible(guest,'guest-not-sharing-preview')
        click_text(host,'Screen 2 Test Pattern 2 1280 x 720','host-choose-screen-cancel')
        # Fixed 1140×1100/two-screen fixture: Antonio's Cancel caption is
        # intermittently misread by OCR; click the verified footer hit area.
        click_point(host,528,810,'host-cancel-screen')
        assert locate(host,'Share screen','host-cancel-kept-off',controls=True)
        click(host,'Share screen','host-screen-chooser')
        click_text(host,'Screen 2 Test Pattern 2 1280 x 720','host-choose-screen-two')
        click_point(host,400,810,'host-confirm-screen')
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
        print('Chat column/unread count, persistent media controls, screen choice/Cancel, narrow live view and confirmed teardown passed',flush=True)
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
