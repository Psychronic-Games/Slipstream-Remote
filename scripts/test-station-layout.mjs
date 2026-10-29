// Native Chromium layout checks. Invoke through test-station-admin.py --browser
// and scripts/check.sh; every viewport gets a fresh browser that is then closed.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {mkdtemp, readFile, writeFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';

const [address, device] = process.argv.slice(2);
assert(address && /^[1-9][0-9]{8}$/.test(device), 'Expected isolated fixture address and device ID');
assert(['127.0.0.1', 'localhost', '[::1]'].includes(new URL(address).hostname), 'Browser audits require a loopback fixture station');
const evidence = await mkdtemp(join(tmpdir(), 'slipstream-station-layout-'));
console.log(`Layout evidence: ${evidence}`);
const widths = (process.env.SLIPSTREAM_LAYOUT_WIDTHS || '1680,1024,390').split(',').map(Number);
assert(widths.every(w => [1680,1024,390].includes(w)), 'Unsupported audit width');
for (const width of widths) {
  const profile = await mkdtemp(join(tmpdir(), 'slipstream-browser-'));
  const browser = spawn(process.env.CHROMIUM || 'chromium', [
    '--headless=new', `--window-size=${width},1100`, '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage',
    '--no-first-run', '--no-default-browser-check', '--disable-background-networking',
    '--disable-extensions', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank',
  ], {stdio: 'ignore'});
  let socket;
  let next = 0;
  const pending = new Map();
  try {
    let port;
    for (let retry = 0; retry < 100; retry++) {
      try { port = (await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]; break; }
      catch { await delay(100); }
    }
    assert(port, 'Chromium did not start');
    const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    socket = new WebSocket(targets.find(t => t.type === 'page').webSocketDebuggerUrl);
    await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
    socket.onmessage = event => {
      const reply = JSON.parse(event.data);
      if (!reply.id) return;
      const handlers = pending.get(reply.id);
      pending.delete(reply.id);
      if (!handlers) return;
      if (reply.error) handlers.reject(new Error(JSON.stringify(reply.error)));
      else handlers.resolve(reply.result);
    };
    const call = (method, params = {}) => new Promise((resolve, reject) => {
      const id = ++next;
      pending.set(id, {resolve, reject});
      socket.send(JSON.stringify({id, method, params}));
    });
    const evaluate = async expression => {
      const result = await call('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true});
      assert(!result.exceptionDetails, JSON.stringify(result.exceptionDetails));
      return result.result.value;
    };
    const waitFor = async expression => {
      for (let retry = 0; retry < 150; retry++) {
        if (await evaluate(expression)) return;
        await delay(100);
      }
      throw new Error(`Timed out: ${expression}`);
    };
    await call('Page.enable');
    await call('Emulation.setDeviceMetricsOverride', {width, height: 1100, deviceScaleFactor: 1, mobile: false});
    await call('Page.navigate', {url: address + '/admin/?tab=techs'});
    await waitFor('!!document.querySelector("input[name=username]")');
    await evaluate(`(async () => {
      const form = document.querySelector('form');
      const fields = new FormData(form);
      fields.set('username', 'admin'); fields.set('password', 'original-station-password');
      await fetch('/admin/?tab=techs', {method: 'POST', body: fields});
    })()`);
    let pageNumber = 0;
    const navigate = async (tab, selector) => {
      const url = address + '/admin/?tab=' + tab + '&audit=' + ++pageNumber;
      await call('Page.navigate', {url});
      await waitFor(`location.href === ${JSON.stringify(url)} && !!document.querySelector(${JSON.stringify(selector)}) && document.readyState === 'complete'`);
      await evaluate('document.fonts.ready');
    };
    const screenshot = async name => {
      const result = await call('Page.captureScreenshot', {format: 'png', captureBeyondViewport: true});
      await writeFile(join(evidence, `${width}-${name}.png`), Buffer.from(result.data, 'base64'));
    };
    const checkLayout = async name => {
      const layout = await evaluate(`({
        mainWidth: document.querySelector('main').getBoundingClientRect().width,
        document: document.documentElement.scrollWidth,
        scrollTables: [...document.querySelectorAll('.table-scroll, .relay-report, .event-details pre')].filter(e => e.clientWidth > 0 && e.scrollWidth > e.clientWidth + 1).length,
        clippedControls: [...document.querySelectorAll('button, input:not([type=hidden]), textarea, select, .multi-picker > summary')].filter(e => {
          if (e.checkVisibility && !e.checkVisibility()) return false;
          const r = e.getBoundingClientRect();
          return r.width > 0 && (r.left < -1 || r.right > innerWidth + 1);
        }).map(e=>({tag:e.tagName,name:e.name,text:e.textContent.slice(0,60),left:e.getBoundingClientRect().left,right:e.getBoundingClientRect().right}))
      })`);
      await screenshot(name);
      if (width === 1680) assert(layout.mainWidth > 1500, 'Desktop console is still narrow');
      assert(layout.document <= width + 1, `${width}/${name}: page overflow ${JSON.stringify(layout)}`);
      assert.equal(layout.scrollTables, 0, `${width}/${name}: horizontal scrolling`);
      assert.equal(layout.clippedControls.length, 0, `${width}/${name}: clipped controls ${JSON.stringify(layout.clippedControls)}`);
    };
    const openPicker = async selector => {
      await evaluate(`(() => { const p = ${selector}; p.querySelector('summary').scrollIntoView({block:'center'}); p.open = true; })()`);
      await delay(60);
      const fits = await evaluate(`(() => { const p = ${selector}; const r = p.querySelector('.picker-menu').getBoundingClientRect(); return p.open && r.width > 0 && r.left >= 0 && r.right <= innerWidth && r.top >= 0 && r.bottom <= innerHeight + 1; })()`);
      assert(fits, `${width}: dropdown escapes the viewport`);
    };
    const choose = async (picker, value, toggle = false) => {
      await evaluate(`(() => { const p = ${picker}; const search=p.querySelector('[data-picker-search]'); search.value=${JSON.stringify(value)}; search.dispatchEvent(new Event('input',{bubbles:true})); const row=[...p.querySelectorAll('[data-picker-option]')].find(e=>!e.hidden && e.textContent.includes(${JSON.stringify(value)})); if(!row) throw new Error('Missing picker choice'); const input=row.querySelector('input'); if(${toggle} || !input.checked) input.click(); })()`);
    };
    const submit = async form => {
      const ok = await evaluate(`(async () => { const response = await fetch('/admin/', {method:'POST', body:new FormData(${form})}); return response.ok; })()`);
      assert(ok, `${width}: form submission failed`);
    };
    const groupForm = (action, group = 'Engineering', owner = '') => `[...document.querySelectorAll('form')].find(f=>f.querySelector('input[name=action]')?.value===${JSON.stringify(action)} && f.querySelector('input[name=owner]')?.value===${JSON.stringify(owner)} && f.querySelector('input[name=group]')?.value===${JSON.stringify(group)})`;

    // Four page cases per viewport; each batch owns and closes its browser.
    await navigate('status', '[data-relay-report]');
    assert(await evaluate(`!![...document.querySelectorAll('[data-log-event]')].find(e=>e.textContent.includes('CUSTOM-PC'))`));
    assert(await evaluate(`!document.querySelector('img[data-xss-fixture]')`), 'Log markup was not escaped');
    await evaluate(`(() => { const level=document.querySelector('[data-log-level]'); level.value='attention'; level.dispatchEvent(new Event('change')); })()`);
    assert(await evaluate(`[...document.querySelectorAll('[data-log-event]:not([hidden])')].every(e=>['WARN','ERROR'].includes(e.dataset.level))`));
    await evaluate(`(() => { const level=document.querySelector('[data-log-level]'); level.value=''; level.dispatchEvent(new Event('change')); const search=document.querySelector('[data-log-search]'); search.value='CUSTOM-PC'; search.dispatchEvent(new Event('input')); })()`);
    assert(await evaluate(`document.querySelectorAll('[data-log-event]:not([hidden])').length===1`), 'Log search did not filter events');
    await evaluate(`(() => { const search=document.querySelector('[data-log-search]'); search.value=''; search.dispatchEvent(new Event('input')); for(const d of document.querySelectorAll('.event-details')) d.open=true; })()`);
    await checkLayout('relay-log');

    await navigate('groups', '.group-access');
    await evaluate(`(() => { const search=document.querySelector('[data-group-search]'); search.value='not-a-real-group'; search.dispatchEvent(new Event('input')); })()`);
    assert(await evaluate(`document.querySelectorAll('[data-group-row]:not([hidden])').length===0 && !document.querySelector('[data-group-empty]').hidden`), 'Group search did not filter rows');
    await evaluate(`(() => { const search=document.querySelector('[data-group-search]'); search.value=''; search.dispatchEvent(new Event('input')); })()`);
    const membersPicker = `(${groupForm('group_members')}).querySelector('[data-picker]')`;
    await openPicker(membersPicker);
    await choose(membersPicker, 'SECOND-PC');
    assert(await evaluate(`new FormData(${groupForm('group_members')}).getAll('ids[]').length>=2`), 'Filtering lost an existing workstation selection');
    await evaluate(`(${membersPicker}).querySelector('[data-picker-done]').click()`);
    await submit(groupForm('group_members'));
    await navigate('groups', '.group-access');
    assert(await evaluate(`[...(${membersPicker}).querySelectorAll('[data-picker-option]')].find(e=>e.textContent.includes('SECOND-PC')).querySelector('input').checked`), 'Workstation membership did not persist');
    const visibilityForm = groupForm('group_visibility');
    const visibilityPicker = `(${visibilityForm}).querySelector('[data-picker]')`;
    const labForm = groupForm('group_visibility', 'Lab');
    const labPicker = `(${labForm}).querySelector('[data-picker]')`;
    const initialTech = await evaluate(`[...(${visibilityPicker}).querySelectorAll('input[type=checkbox]')].find(e=>e.value==='layout-tech').checked`);
    await evaluate(`window.groupSaveAudit='same-document'`);
    await openPicker(visibilityPicker);
    await choose(visibilityPicker, 'layout-tech', true);
    assert(await evaluate(`new FormData(${visibilityForm}).getAll('usernames[]').includes('admin')`), 'Filtering lost existing visibility');
    assert(await evaluate(`(() => {const e=new Event('beforeunload',{cancelable:true});window.dispatchEvent(e);return e.defaultPrevented;})()`), 'Pending changes have no navigation protection');
    // Keep one group pending while saving another, exactly as on the real page.
    await openPicker(labPicker);
    await choose(labPicker, 'layout-tech', true);
    await evaluate(`(${labPicker}).querySelector('[data-picker-done]').click()`);
    await waitFor(`(${labForm}).dataset.saveState==='saved' && !(${labPicker}).open`);
    assert(await evaluate(`(${visibilityForm}).dataset.saveState==='dirty' && window.groupSaveAudit==='same-document'`), 'Saving a different group lost the pending edit or reloaded the page');
    await openPicker(visibilityPicker);
    await evaluate(`(${visibilityPicker}).querySelector('[data-picker-done]').click()`);
    await waitFor(`(${visibilityForm}).dataset.saveState==='saved' && !(${visibilityPicker}).open`);
    assert(await evaluate(`window.groupSaveAudit==='same-document'`), 'Done reloaded the page');
    // A failed request keeps the picker, selections and retry available.
    await openPicker(labPicker);
    await choose(labPicker, 'admin', true);
    const pendingAdmin = await evaluate(`[...(${labPicker}).querySelectorAll('input[type=checkbox]')].find(e=>e.value==='admin').checked`);
    await evaluate(`window.originalSaveFetch=window.fetch;window.fetch=()=>Promise.reject(new TypeError('Simulated connection failure'))`);
    await evaluate(`(${labPicker}).querySelector('[data-picker-done]').click()`);
    await waitFor(`(${labForm}).dataset.saveState==='error'`);
    assert(await evaluate(`(${labPicker}).open && [...(${labPicker}).querySelectorAll('input[type=checkbox]')].find(e=>e.value==='admin').checked===${pendingAdmin} && (${labForm}).querySelector('[data-picker-save-status]').textContent.includes('still here')`), 'A failed save discarded the selection or closed the picker');
    await evaluate(`window.fetch=window.originalSaveFetch;delete window.originalSaveFetch;(${labPicker}).querySelector('[data-picker-done]').click()`);
    await waitFor(`(${labForm}).dataset.saveState==='saved' && !(${labPicker}).open`);
    // Unchecking every recipient must be saved too.
    await openPicker(labPicker);
    await evaluate(`for(const input of (${labPicker}).querySelectorAll('input[type=checkbox]')) if(input.checked && !input.disabled) input.click()`);
    await evaluate(`(${labPicker}).querySelector('[data-picker-done]').click()`);
    await waitFor(`(${labForm}).dataset.saveState==='saved' && !(${labPicker}).open`);
    assert(await evaluate(`(() => {const e=new Event('beforeunload',{cancelable:true});window.dispatchEvent(e);return !e.defaultPrevented;})()`), 'Saved changes still block navigation');
    await navigate('groups', '.group-access');
    assert(await evaluate(`[...(${visibilityPicker}).querySelectorAll('input[type=checkbox]')].find(e=>e.value==='layout-tech').checked===${!initialTech}`), 'Technician visibility did not persist after Done');
    assert(await evaluate(`[...(${labPicker}).querySelectorAll('input[type=checkbox]')].every(e=>!e.checked)`), 'Clearing all visibility did not persist');
    const personalForm = groupForm('group_visibility', 'Admin personal', 'admin');
    const personalPicker = `(${personalForm}).querySelector('[data-picker]')`;
    await openPicker(personalPicker);
    await choose(personalPicker, 'layout-tech', true);
    await evaluate(`(${personalPicker}).querySelector('[data-picker-done]').click()`);
    await waitFor(`(${personalForm}).dataset.saveState==='saved' && !(${personalPicker}).open`);
    assert(await evaluate(`(() => {const e=[...(${personalPicker}).querySelectorAll('input[type=checkbox]')].find(e=>e.value==='admin');return e.checked && e.disabled;})()`), 'Saving re-enabled or removed the personal group owner');
    await checkLayout('groups');

    await navigate('devices', '.devices-table');
    await waitFor(`!document.querySelector('[data-device-controls]').hidden`);
    const visibleDevices = () => evaluate(`[...document.querySelectorAll('[data-device-row]')].filter(row=>!row.hidden).map(row=>({id:Number(row.dataset.deviceId),name:row.querySelector('[data-device-name]').textContent,seen:Number(row.dataset.deviceSeen)}))`);
    const searchDevices = async text => {
      await evaluate(`(() => {const input=document.querySelector('[data-device-search]');input.value=${JSON.stringify(text)};input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    };
    const sortDevices = async key => {
      await evaluate(`(() => {const select=document.querySelector('[data-device-sort]');select.value=${JSON.stringify(key)};select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
    };
    const flipDevices = () => evaluate(`document.querySelector('[data-device-direction]').click()`);
    const allDevices = await visibleDevices();
    assert.equal(allDevices.length, 5, 'Device sorting fixture is incomplete');
    assert(await evaluate(`[...document.querySelectorAll('[data-device-row]')].every(row=>row.checkVisibility())`), 'A device row disappeared at a narrow width');
    assert(!await evaluate(`!!document.querySelector('img[data-device-xss]')`), 'Device search metadata was not escaped');
    assert(allDevices.every((row,index)=>!index || row.seen<=allDevices[index-1].seen), 'Default order is not newest first');
    await evaluate(`window.deviceListAudit='same-document'`);
    await sortDevices('name');
    let ordered = await visibleDevices();
    assert(ordered.findIndex(row=>row.name==='Bench 2')<ordered.findIndex(row=>row.name==='Bench 10'), 'Device names do not use natural numeric ordering');
    await flipDevices();
    ordered = await visibleDevices();
    assert(ordered.findIndex(row=>row.name==='Bench 10')<ordered.findIndex(row=>row.name==='Bench 2'), 'Reverse name sort failed');
    await sortDevices('group');
    ordered = await visibleDevices();
    assert(ordered.findIndex(row=>row.name==='Bench 10')<ordered.findIndex(row=>row.name==='Bench 2'), 'Group sort did not use the first natural group name for a multi-group device');
    await flipDevices();
    ordered = await visibleDevices();
    assert(ordered.findIndex(row=>row.name==='Bench 2')<ordered.findIndex(row=>row.name==='Bench 10'), 'Reverse group sort failed');
    await sortDevices('id');
    ordered = await visibleDevices();
    assert(ordered.every((row,index)=>!index || row.id>ordered[index-1].id), 'Ascending ID sort failed');
    await flipDevices();
    ordered = await visibleDevices();
    assert(ordered.every((row,index)=>!index || row.id<ordered[index-1].id), 'Descending ID sort failed');
    await sortDevices('recent');
    await flipDevices();
    ordered = await visibleDevices();
    assert(ordered.every((row,index)=>!index || row.seen>=ordered[index-1].seen), 'Oldest-first sort failed');
    for (const query of ['custom-pc', 'Lab', 'BOARD-123456789', '12345670012', device, device.match(/.{3}/g).join(' '), device.match(/.{3}/g).join('-')]) {
      await searchDevices(query);
      const found = await visibleDevices();
      assert(found.length===1 && found[0].id===Number(device), `Device search failed for ${query}`);
    }
    await searchDevices('hardware-bench-10');
    assert((await visibleDevices())[0]?.name==='Bench 10', 'Original computer name was not searchable after an alias');
    await searchDevices('Zone 2');
    assert.deepEqual((await visibleDevices()).map(row=>row.name), ['Bench 10'], 'Hidden group options polluted the search');
    await sortDevices('id');
    await flipDevices();
    assert.equal((await visibleDevices()).length, 1, 'Sorting reset the active search');
    await searchDevices('Ungrouped');
    assert.deepEqual((await visibleDevices()).map(row=>row.name), ['UNASSIGNED-PC'], 'Ungrouped search matched grouped devices');
    await searchDevices('layout-tech');
    assert.equal((await visibleDevices()).length, 0, 'Hidden assignment options polluted the search');
    assert(await evaluate(`!document.querySelector('[data-device-empty]').hidden && document.querySelector('[data-device-count]').textContent==='0 of 5 devices'`), 'No-match feedback is missing');
    await evaluate(`document.querySelector('[data-device-clear]').click()`);
    ordered = await visibleDevices();
    assert.equal(ordered.length, 5, 'Clear search did not restore all devices');
    assert(ordered.every((row,index)=>!index || row.id<ordered[index-1].id), 'Clear search reset the chosen sort order');
    assert(await evaluate(`window.deviceListAudit==='same-document' && document.querySelector('[data-device-empty]').hidden`), 'Search/sort reloaded the page');
    const dialog = `document.getElementById('edit-device-${device}')`;
    await evaluate(`document.querySelector('[data-dialog-open="edit-device-${device}"]').click()`);
    assert(await evaluate(`${dialog}.open`), 'Device editor did not open');
    const resized = await evaluate(`(() => { const d=${dialog}; const text=d.querySelector('textarea'); const old=text.getBoundingClientRect().height; d.querySelector('[data-resize-notes]').dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true})); return text.getBoundingClientRect().height>=old+20 && getComputedStyle(text).resize==='none'; })()`);
    assert(resized, 'Themed notes bar did not resize the textarea');
    const grip = await evaluate(`(() => { const d=${dialog}; const r=d.querySelector('[data-resize-notes]').getBoundingClientRect(); return {x:r.left+r.width/2,y:r.top+r.height/2,height:d.querySelector('textarea').getBoundingClientRect().height}; })()`);
    const hit = await evaluate(`(() => { window.auditPointerEvents=[]; window.auditPointerErrors=[]; addEventListener('error',e=>window.auditPointerErrors.push(e.message)); for(const t of ['pointerdown','pointermove','pointerup']) document.addEventListener(t,e=>window.auditPointerEvents.push({type:e.type,button:e.button,buttons:e.buttons,target:e.target.className,y:e.clientY}),true); const e=document.elementFromPoint(${grip.x},${grip.y}); return {matches:!!e?.closest('[data-resize-notes]'),tag:e?.tagName,className:e?.className,width:innerWidth,height:innerHeight}; })()`);
    assert(hit.matches, `Notes bar hit test failed: ${JSON.stringify({grip,hit})}`);
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',x:grip.x,y:grip.y});
    await call('Input.dispatchMouseEvent',{type:'mousePressed',x:grip.x,y:grip.y,button:'left',buttons:1,clickCount:1});
    await delay(30);
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',x:grip.x,y:grip.y+60,buttons:1});
    await delay(30);
    await call('Input.dispatchMouseEvent',{type:'mouseReleased',x:grip.x,y:grip.y+60,button:'left',buttons:0,clickCount:1});
    const draggedHeight = await evaluate(`${dialog}.querySelector('textarea').getBoundingClientRect().height`);
    const dragDebug = await evaluate('({events:window.auditPointerEvents,errors:window.auditPointerErrors})');
    assert(draggedHeight >= grip.height + 50, `Dragging notes failed: ${JSON.stringify({grip,draggedHeight,hit,dragDebug})}`);
    const initialGroupCount = await evaluate(`new FormData(${dialog}.querySelector('form')).getAll('groups[]').length`);
    const groupsPicker = `${dialog}.querySelector('[data-picker]')`;
    await openPicker(groupsPicker);
    const created = `Field team ${width}`;
    await evaluate(`(() => { const p=${groupsPicker}; const search=p.querySelector('[data-picker-search]'); search.value=${JSON.stringify(created)}; search.dispatchEvent(new Event('input',{bubbles:true})); p.querySelector('[data-picker-create]').click(); const text=${dialog}.querySelector('textarea'); text.value=['First line','Second line'].join(String.fromCharCode(10)); })()`);
    assert(await evaluate(`new FormData(${dialog}.querySelector('form')).getAll('groups[]').includes(${JSON.stringify(created)})`), 'Inline group was not selected');
    assert.equal(await evaluate(`new FormData(${dialog}.querySelector('form')).getAll('groups[]').length`), initialGroupCount+1, 'Creating a group lost earlier selections');
    assert(await evaluate(`getComputedStyle((${groupsPicker}).querySelector('input[type=checkbox]')).appearance==='none'`), 'Checkboxes use native platform styling');
    await screenshot('device-editor');
    await evaluate(`(${groupsPicker}).querySelector('[data-picker-done]').click()`);
    await submit(`${dialog}.querySelector('form')`);
    await evaluate(`${dialog}.close()`);
    await navigate('devices', '.devices-table');
    assert(await evaluate(`[...document.querySelectorAll('.group-chip')].some(e=>e.textContent===${JSON.stringify(created)})`), 'New group did not persist');
    assert(await evaluate(`[...document.querySelectorAll('.group-chip')].some(e=>e.textContent==='Engineering') && [...document.querySelectorAll('.group-chip')].some(e=>e.textContent==='Lab')`), 'Saving lost existing memberships');
    await evaluate(`document.querySelector('[data-dialog-open="edit-device-${device}"]').click()`);
    assert(await evaluate(`${dialog}.querySelector('textarea').value.split(String.fromCharCode(10)).length===2`), 'Multiline notes did not persist');
    await evaluate(`${dialog}.close()`);
    await delay(30);
    await evaluate(`document.querySelector('[data-dialog-open="edit-device-${device}"]').click()`);
    await evaluate(`${dialog}.querySelector('textarea').value='Unsaved edit'`);
    await evaluate(`${dialog}.querySelector('[data-dialog-close]').click()`);
    await delay(30);
    await evaluate(`document.querySelector('[data-dialog-open="edit-device-${device}"]').click()`);
    assert(await evaluate(`${dialog}.querySelector('textarea').value.split(String.fromCharCode(10)).length===2`), 'Cancel retained an unsaved edit');
    await evaluate(`${dialog}.close()`);
    await delay(30);
    const assignForm = `[...document.querySelectorAll('.device-assign-form')].find(f=>f.querySelector('input[name=id]').value==='${device}')`;
    const assignPicker = `(${assignForm}).querySelector('[data-picker]')`;
    await openPicker(assignPicker);
    await choose(assignPicker, 'layout-tech');
    await evaluate(`(${assignPicker}).querySelector('[data-picker-done]').click()`);
    await sortDevices('name');
    assert(await evaluate(`new FormData(${assignForm}).getAll('usernames[]').includes('layout-tech')`), 'Assignment did not include the searched technician');
    assert(await evaluate(`(${assignForm}).querySelector('input[name=id]').value==='${device}'`), 'Sorting changed the assignment target');
    await submit(assignForm);
    await checkLayout('devices');
    await navigate(`devices&device=${device}`, '.system-report');
    await checkLayout('report');
    console.log(`Station ${width}px: device search/natural sorting, wrapped logs/search, group membership and Done-to-save visibility/retry, inline group creation, themed notes/checkboxes, searchable assignment and responsive layouts passed`);

  } finally {
    if (socket?.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({id: ++next, method: 'Browser.close'}));
      await delay(200);
    }
    socket?.close();
    browser.kill('SIGTERM');
    await Promise.race([new Promise(resolve => browser.once('exit', resolve)), delay(5000).then(() => browser.kill('SIGKILL'))]);
    await rm(profile, {recursive: true, force: true, maxRetries: 10, retryDelay: 100});
  }
}
