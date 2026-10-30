/* Native station controls; no remote libraries or account data leave this page. */
(() => {
  'use strict';
  document.documentElement.classList.add('admin-enhanced');
  let activePicker = null;
  const options = picker => [...picker.querySelectorAll('[data-picker-option]')];
  const selected = picker => options(picker).filter(row => row.querySelector('input').checked);
  const closePicker = (restoreFocus = false) => {
    if (!activePicker) return;
    const picker = activePicker;
    activePicker = null;
    picker.open = false;
    if (restoreFocus) picker.querySelector('summary').focus();
  };
  const placePicker = () => {
    if (!activePicker?.open) return;
    const trigger = activePicker.querySelector('summary').getBoundingClientRect();
    const menu = activePicker.querySelector('.picker-menu');
    const width = Math.min(380, innerWidth - 24);
    const below = innerHeight - trigger.bottom - 12;
    const above = trigger.top - 12;
    const down = below >= Math.min(320, above);
    if (trigger.bottom < 0 || trigger.top > innerHeight) { closePicker(); return; }
    const height = Math.max(80, Math.min(420, down ? below : above, innerHeight - 24));
    menu.style.width = `${width}px`;
    menu.style.maxHeight = `${height}px`;
    menu.style.left = `${Math.max(12, Math.min(trigger.left, innerWidth - width - 12))}px`;
    menu.style.bottom = 'auto';
    const actualHeight = menu.getBoundingClientRect().height;
    const desiredTop = down ? trigger.bottom + 6 : trigger.top - actualHeight - 6;
    menu.style.top = `${Math.max(12, Math.min(desiredTop, innerHeight - actualHeight - 12))}px`;

  };
  const refresh = picker => {
    const rows = options(picker);
    const chosen = selected(picker);
    const names = chosen.map(row => row.querySelector('[data-option-label]').textContent.trim());
    const summary = picker.querySelector('[data-picker-summary]');
    summary.textContent = names.length === 1 ? names[0] : names.length ? `${names.length} selected` : picker.dataset.placeholder;
    summary.title = names.join(', ');
    picker.querySelector('[data-picker-status]').textContent = `${names.length} selected`;
    const query = picker.querySelector('[data-picker-search]').value.trim().toLocaleLowerCase();
    let visible = 0;
    for (const row of rows) {
      row.hidden = !row.textContent.toLocaleLowerCase().includes(query);
      if (!row.hidden) visible++;
    }
    picker.querySelector('[data-picker-empty]').hidden = visible > 0;
    const create = picker.querySelector('[data-picker-create]');
    if (create) {
      const text = picker.querySelector('[data-picker-search]').value.trim();
      create.hidden = !text || names.length >= 32 || rows.some(row => row.querySelector('[data-option-label]').textContent.trim().toLocaleLowerCase() === query);
      create.textContent = `Create & select “${text}”`;
      create.disabled = [...text].length > 64;
    }
    placePicker();
  };
  for (const picker of document.querySelectorAll('[data-picker]')) {
    picker.addEventListener('toggle', () => {
      if (picker.open) {
        if (activePicker && activePicker !== picker) activePicker.open = false;
        activePicker = picker;
        refresh(picker);
        picker.querySelector('[data-picker-search]').focus({preventScroll: true});
      } else if (activePicker === picker) activePicker = null;
    });
    picker.querySelector('[data-picker-search]').addEventListener('input', () => refresh(picker));
    picker.addEventListener('change', () => refresh(picker));
    picker.querySelector('[data-picker-done]').addEventListener('click', () => {
      const form = picker.closest('form[data-picker-save]');
      if (form) form.requestSubmit();
      else closePicker(true);
    });
    picker.addEventListener('keydown', event => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); closePicker(true); }
      if (event.key === 'Enter' && event.target.matches('[data-picker-search]')) {
        event.preventDefault();
        const create = picker.querySelector('[data-picker-create]:not([hidden]):not(:disabled)');
        if (create) create.click();
        else {
          const first = options(picker).find(row => !row.hidden && !row.querySelector('input').disabled);
          first?.querySelector('input').focus();
        }
      }
    });
    picker.querySelector('[data-picker-create]')?.addEventListener('click', () => {
      const search = picker.querySelector('[data-picker-search]');
      const name = search.value.trim();
      if (!name || [...name].length > 64 || selected(picker).length >= 32) return;
      const row = document.createElement('label');
      row.dataset.pickerOption = '';
      row.dataset.pickerCreated = '';
      const input = document.createElement('input');
      input.type = 'checkbox'; input.name = `${picker.dataset.name}[]`; input.value = name; input.checked = true;
      const text = document.createElement('span'); text.dataset.optionLabel = ''; text.textContent = name;
      row.append(input, text);
      picker.querySelector('.picker-options').append(row);
      search.value = '';
      refresh(picker);
      search.focus();
    });
    refresh(picker);
  }
  const saves = [];
  for (const form of document.querySelectorAll('form[data-picker-save]')) {
    const picker = form.querySelector('[data-picker]');
    const done = picker.querySelector('[data-picker-done]');
    const status = form.querySelector('[data-picker-save-status]');
    const selection = () => JSON.stringify(selected(picker).map(row => row.querySelector('input').value).sort());
    const state = {saved: selection(), dirty: false, saving: false};
    saves.push(state);
    const message = (kind, text) => {
      form.dataset.saveState = kind;
      status.textContent = text;
      status.hidden = !text;
    };
    const closeSaved = () => {
      if (activePicker === picker) closePicker(true);
      else picker.open = false;
    };
    form.addEventListener('change', () => {
      state.dirty = selection() !== state.saved;
      message(state.dirty ? 'dirty' : '', state.dirty ? 'Not saved yet — press Done.' : '');
    });
    form.addEventListener('submit', async event => {
      event.preventDefault();
      if (state.saving) return;
      if (!state.dirty) { closeSaved(); return; }
      // Capture before disabling inputs: disabled checkboxes must not turn a
      // selected list into an accidental revoke-all request.
      const fields = new FormData(form);
      fields.set('complete', '1'); // Detect PHP input truncation before any write.
      const submitted = selection();
      const controls = [...picker.querySelectorAll('input, button')].map(control => [control, control.disabled]);
      state.saving = true;
      form.setAttribute('aria-busy', 'true');
      for (const [control] of controls) control.disabled = true;
      done.textContent = 'Saving…';
      message('saving', 'Saving…');
      let saved = false;
      try {
        const response = await fetch(form.action, {
          method: 'POST', body: fields, credentials: 'same-origin', redirect: 'error',
          headers: {Accept: 'application/json'},
        });
        const reply = response.headers.get('content-type')?.includes('application/json') ? await response.json() : null;
        if (!response.ok || !reply?.ok) throw new Error(reply?.error || 'The station did not confirm the save. Press Done to retry.');
        state.saved = submitted;
        state.dirty = selection() !== state.saved;
        saved = !state.dirty;
        message(saved ? 'saved' : 'dirty', saved ? 'Saved' : 'Not saved yet — press Done.');
      } catch (error) {
        message('error', `${error.message || 'Could not reach the station.'} Your selections are still here.`);
      } finally {
        for (const [control, disabled] of controls) control.disabled = disabled;
        done.textContent = 'Done';
        state.saving = false;
        form.removeAttribute('aria-busy');
      }
      if (saved) closeSaved();
      else if (!activePicker || activePicker === picker) { picker.open = true; placePicker(); }
    });
  }
  addEventListener('beforeunload', event => {
    if (saves.some(state => state.dirty || state.saving)) {
      event.preventDefault();
      event.returnValue = '';
    }
  });
  document.addEventListener('pointerdown', event => {
    if (activePicker && !activePicker.contains(event.target)) closePicker();
  });
  document.addEventListener('focusin', event => {
    if (activePicker && !activePicker.contains(event.target)) closePicker();
  });
  addEventListener('resize', placePicker);
  addEventListener('scroll', placePicker, true);

  document.addEventListener('click', event => {
    const opener = event.target.closest('[data-dialog-open]');
    if (opener) {
      closePicker();
      document.getElementById(opener.dataset.dialogOpen)?.showModal();
    }
    if (event.target.closest('[data-dialog-close]')) {
      closePicker(); event.target.closest('dialog')?.close();
    }
  });
  for (const dialog of document.querySelectorAll('dialog')) {
    dialog.addEventListener('close', () => {
      if (activePicker && dialog.contains(activePicker)) closePicker();
      dialog.querySelector('form')?.reset();
    });
    dialog.addEventListener('cancel', event => {
      if (activePicker) { event.preventDefault(); closePicker(true); }
    });
  }
  document.addEventListener('reset', event => {
    queueMicrotask(() => {
      for (const picker of event.target.querySelectorAll('[data-picker]')) {
        for (const row of picker.querySelectorAll('[data-picker-created]')) row.remove();
        refresh(picker);
      }
    });
  });
  let stopNotesDrag = () => {};
  document.addEventListener('close', () => stopNotesDrag(), true);
  for (const handle of document.querySelectorAll('[data-resize-notes]')) {
    const textarea = document.getElementById(handle.getAttribute('aria-controls'));
    const setHeight = height => {
      height = Math.max(88, Math.min(640, height));
      textarea.style.height = `${height}px`;
      handle.setAttribute('aria-valuenow', Math.round(height));
    };
    handle.setAttribute('aria-valuemin', '88'); handle.setAttribute('aria-valuemax', '640');
    handle.addEventListener('pointerdown', event => {
      if (event.button !== 0) return;
      event.preventDefault();
      stopNotesDrag();
      const pointer = event.pointerId;
      const start = event.clientY, height = textarea.getBoundingClientRect().height;
      const controller = new AbortController();
      const stop = () => {
        controller.abort();
        handle.classList.remove('is-resizing');
        if (handle.hasPointerCapture(pointer)) handle.releasePointerCapture(pointer);
        if (stopNotesDrag === stop) stopNotesDrag = () => {};
      };
      stopNotesDrag = stop;
      const move = e => {
        if (e.pointerId !== pointer) return;
        if (!e.buttons) { stop(); return; }
        setHeight(height + e.clientY - start);
      };
      const end = e => { if (e.pointerId === pointer) stop(); };
      // Keep tracking when the pointer leaves the grip, including when capture
      // is unavailable. Cleanup remains bounded to this one drag.
      document.addEventListener('pointermove', move, {signal: controller.signal});
      document.addEventListener('pointerup', end, {signal: controller.signal});
      document.addEventListener('pointercancel', end, {signal: controller.signal});
      addEventListener('blur', stop, {signal: controller.signal});
      handle.classList.add('is-resizing');
      try { handle.setPointerCapture(pointer); } catch { /* Document tracking remains active. */ }
    });
    handle.addEventListener('keydown', event => {
      if (!['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault();
      const height = textarea.getBoundingClientRect().height;
      setHeight(event.key === 'Home' ? 88 : event.key === 'End' ? 640 : height + (event.key === 'ArrowDown' ? 24 : -24));
    });
  }
  // The same themed, keyboard-operable single-choice menu serves every page.
  const choiceMenus = new Map();
  for (const menu of document.querySelectorAll('[data-choice-menu]')) {
    const summary = menu.querySelector('summary');
    const popup = menu.querySelector('[role=menu]');
    const choices = [...menu.querySelectorAll('[data-choice-option]')];
    const labels = new Map(choices.map(choice => [choice.dataset.choiceOption, choice.querySelector('[data-choice-option-label]').textContent]));
    const close = (focus = false) => {
      menu.open = false;
      summary.setAttribute('aria-expanded', 'false');
      if (focus) summary.focus({preventScroll: true});
    };
    const setValue = value => {
      if (!labels.has(value)) return;
      menu.dataset.value = value;
      menu.querySelector('[data-choice-label]').textContent = labels.get(value);
      for (const choice of choices) {
        const active = choice.dataset.choiceOption === value;
        choice.setAttribute('aria-checked', String(active));
        choice.tabIndex = active ? 0 : -1;
      }
    };
    const place = () => {
      if (!menu.open) return;
      const anchor = summary.getBoundingClientRect();
      if (anchor.bottom < 0 || anchor.top > innerHeight) { close(); return; }
      const width = Math.min(250, innerWidth - 24);
      const below = innerHeight - anchor.bottom - 12, above = anchor.top - 12;
      const down = below >= Math.min(340, above);
      popup.style.width = `${width}px`;
      popup.style.maxHeight = `${Math.max(80, Math.min(380, down ? below : above, innerHeight - 24))}px`;
      popup.style.left = `${Math.max(12, Math.min(anchor.left, innerWidth - width - 12))}px`;
      const height = popup.getBoundingClientRect().height;
      popup.style.top = `${Math.max(12, Math.min(down ? anchor.bottom + 7 : anchor.top - height - 7, innerHeight - height - 12))}px`;
    };
    const focusChoice = choice => {
      choice.focus({preventScroll: true});
      choice.scrollIntoView({block: 'nearest', inline: 'nearest'});
    };
    for (const choice of choices) choice.addEventListener('click', () => {
      setValue(choice.dataset.choiceOption);
      menu.dispatchEvent(new CustomEvent('choicechange', {bubbles: true, detail: {value: menu.dataset.value}}));
      close(true);
    });
    menu.addEventListener('toggle', () => {
      summary.setAttribute('aria-expanded', String(menu.open));
      if (!menu.open) return;
      closePicker();
      for (const [other, control] of choiceMenus) if (other !== menu) control.close();
      place();
      const visible = choices.includes(document.activeElement) ? document.activeElement : choices.find(choice => choice.getAttribute('aria-checked') === 'true');
      visible.scrollIntoView({block: 'nearest', inline: 'nearest'});
    });
    summary.addEventListener('keydown', event => {
      if (!['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(event.key)) return;
      event.preventDefault();
      if (menu.open && ['Enter', ' '].includes(event.key)) { close(true); return; }
      menu.open = true; place();
      focusChoice(event.key === 'ArrowUp' ? choices.at(-1) : choices.find(choice => choice.getAttribute('aria-checked') === 'true'));
    });
    menu.addEventListener('keydown', event => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(true); return; }
      if (!choices.includes(event.target) || !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault();
      const index = choices.indexOf(event.target);
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? choices.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : -1) + choices.length) % choices.length;
      for (const [i, choice] of choices.entries()) choice.tabIndex = i === next ? 0 : -1;
      focusChoice(choices[next]);
    });
    for (const event of ['pointerdown', 'focusin']) document.addEventListener(event, e => {
      if (menu.open && !menu.contains(e.target)) close();
    });
    for (const picker of document.querySelectorAll('[data-picker]')) picker.addEventListener('toggle', () => {
      if (picker.open) close();
    });
    addEventListener('resize', place);
    addEventListener('scroll', place, true);
    setValue(menu.dataset.value);
    choiceMenus.set(menu, {close, setValue, labels});
  }
  const deviceControls = document.querySelector('[data-device-controls]');
  if (deviceControls) {
    const search = deviceControls.querySelector('[data-device-search]');
    const sort = deviceControls.querySelector('[data-device-sort]');
    const sortControl = choiceMenus.get(sort);
    const sortLabels = sortControl.labels;
    const headers = [...document.querySelectorAll('[data-device-column-sort]')];
    const direction = deviceControls.querySelector('[data-device-direction]');
    const clear = deviceControls.querySelector('[data-device-clear]');
    const body = document.querySelector('[data-device-rows]');
    const collator = new Intl.Collator(undefined, {numeric: true, sensitivity: 'base'});
    const fold = text => text.normalize('NFKC').toLocaleLowerCase().replace(/\s+/g, ' ');
    const rows = [...body.querySelectorAll('[data-device-row]')].map(row => ({
      element: row,
      id: Number(row.dataset.deviceId),
      seen: Number(row.dataset.deviceSeen),
      name: row.querySelector('[data-device-name]').textContent.trim(),
      group: [...row.querySelectorAll('[data-device-groups] .group-chip')].map(chip => chip.textContent.trim()).sort(collator.compare).join('\n'),
      system: row.querySelector('[data-device-system]').dataset.deviceSystem,
      hardware: row.querySelector('[data-device-hardware]')?.dataset.deviceHardware || '',
      // Read only the device's metadata, never hidden dialogs/picker options:
      // otherwise every row would match every group and technician name.
      search: fold(row.dataset.deviceSearchText),
    }));
    let sortKey = sort.dataset.value;
    let descending = true;
    const filter = () => {
      const query = fold(search.value).trim();
      const idQuery = /^[\d\s-]+$/.test(query) ? query.replace(/[\s-]/g, '') : null;
      let visible = 0;
      for (const row of rows) {
        row.element.hidden = !(row.search.includes(query) || (idQuery && String(row.id).includes(idQuery)));
        if (!row.element.hidden) visible++;
      }
      deviceControls.querySelector('[data-device-count]').textContent = `${visible} of ${rows.length} devices`;
      document.querySelector('[data-device-empty]').hidden = visible !== 0;
      clear.disabled = search.value.length === 0;
      closePicker();
    };
    const order = () => {
      const labels = sortKey === 'recent' ? ['Oldest first', 'Newest first'] : sortKey === 'id' ? ['Lowest first', 'Highest first'] : ['A → Z', 'Z → A'];
      direction.textContent = labels[Number(descending)];
      direction.setAttribute('aria-label', `Sort direction: ${direction.textContent}`);
      sortControl.setValue(sortKey);
      for (const header of headers) {
        const active = header.dataset.deviceColumnSort === sortKey;
        header.disabled = false;
        header.setAttribute('aria-pressed', String(active));
        header.setAttribute('aria-label', `${sortLabels.get(header.dataset.deviceColumnSort)}: ${active ? `sorted ${descending ? 'descending' : 'ascending'}; reverse order` : 'sort column'}`);
        header.querySelector('[data-sort-arrow]').textContent = active ? descending ? '↓' : '↑' : '↕';
      }
      for (const column of document.querySelectorAll('[data-device-sort-column]')) {
        if (column.dataset.deviceSortColumn.split(' ').includes(sortKey)) column.setAttribute('aria-sort', descending ? 'descending' : 'ascending');
        else column.removeAttribute('aria-sort');
      }
      rows.sort((a, b) => {
        const primary = sortKey === 'id' ? a.id - b.id : sortKey === 'recent' ? a.seen - b.seen : collator.compare(a[sortKey], b[sortKey]);
        return primary * (descending ? -1 : 1) || collator.compare(a.name, b.name) || a.id - b.id;
      });
      const fragment = document.createDocumentFragment();
      for (const row of rows) fragment.append(row.element);
      body.append(fragment);
      filter();
    };
    const chooseSort = (key, toggle = false) => {
      if (!sortLabels.has(key)) return;
      descending = key === sortKey ? toggle ? !descending : descending : key === 'recent';
      sortKey = key;
      order();
    };
    sort.addEventListener('choicechange', event => chooseSort(event.detail.value));
    for (const header of headers) header.addEventListener('click', () => {
      chooseSort(header.dataset.deviceColumnSort, true);
      sortControl.close();
    });
    search.addEventListener('input', filter);
    direction.addEventListener('click', () => { descending = !descending; order(); });
    clear.addEventListener('click', () => { search.value = ''; filter(); search.focus(); });
    deviceControls.hidden = false;
    order();
  }
  const report = document.querySelector('[data-relay-report]');
  if (report) {
    const search = report.querySelector('[data-log-search]'), level = report.querySelector('[data-log-level]');
    const events = report.querySelector('[data-log-events]');
    const navigation = report.querySelector('[data-log-navigation]');
    const error = report.querySelector('[data-log-error]');
    const buttons = [...navigation.querySelectorAll('[data-log-nav]')];
    let page = JSON.parse(report.dataset.logPage), busy = false;
    const updateNavigation = () => {
      for (const button of buttons) button.disabled = busy || (button.dataset.logNav === 'older' && page.start === 0) || (button.dataset.logNav === 'newer' && page.end >= page.size);
      const position = page.start === 0 ? page.end >= page.size ? 'All retained events' : 'Beginning of retained history' : page.end >= page.size ? 'Latest retained events' : 'Earlier retained events';
      report.querySelector('[data-log-page-label]').textContent = `${position} · ${page.order} first · UTC`;
      report.querySelector('[data-log-continued]').hidden = !page.continued;
      report.setAttribute('aria-busy', String(busy));
    };
    const filter = () => {
      const words = search.value.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
      const rows = [...events.querySelectorAll('[data-log-event]')];
      const severity = level.dataset.value;
      let visible = 0;
      for (const row of rows) {
        const match = (!severity || (severity === 'attention' ? ['WARN', 'ERROR'].includes(row.dataset.level) : row.dataset.level === severity)) && words.every(word => row.textContent.toLocaleLowerCase().includes(word));
        row.hidden = !match; if (match) visible++;
      }
      report.querySelector('[data-log-count]').textContent = `${visible} of ${rows.length} events on this page`;
      report.querySelector('[data-log-empty]').hidden = visible !== 0;
    };
    const navigate = async mode => {
      if (busy) return;
      busy = true; error.hidden = true; updateNavigation();
      const url = new URL(location.href);
      url.search = '';
      url.searchParams.set('relay_log', 'page');
      url.searchParams.set('mode', mode);
      url.searchParams.set('order', ['oldest', 'newest'].includes(mode) ? mode : page.order);
      if (['older', 'newer'].includes(mode)) {
        url.searchParams.set('at', mode === 'older' ? page.start : page.end);
        url.searchParams.set('source', page.source);
      }
      try {
        const response = await fetch(url, {credentials: 'same-origin', redirect: 'error', headers: {'Accept': 'application/json'}});
        const result = await response.json();
        if (!response.ok || !result.ok) throw new Error(result.error || 'Could not load relay history. Try again.');
        events.innerHTML = result.html;
        page = result.page;
        report.dataset.logPage = JSON.stringify(page);
        events.scrollTop = 0;
        filter();
      } catch (failure) {
        error.textContent = failure instanceof TypeError || failure instanceof SyntaxError ? 'Could not load relay history. Your current page and filters are still here; try again.' : failure.message;
        error.hidden = false;
      } finally {
        busy = false; updateNavigation();
      }
    };
    for (const button of buttons) button.addEventListener('click', () => navigate(button.dataset.logNav));
    search.addEventListener('input', filter); level.addEventListener('choicechange', filter);
    navigation.hidden = false;
    updateNavigation(); filter();
  }
  document.querySelector('[data-group-search]')?.addEventListener('input', event => {
    const words = event.target.value.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
    const rows = [...document.querySelectorAll('[data-group-row]')];
    for (const row of rows) row.hidden = !words.every(word => row.dataset.groupSearchText.toLocaleLowerCase().includes(word));
    document.querySelector('[data-group-empty]').hidden = rows.some(row => !row.hidden);
    closePicker();
  });
})();
