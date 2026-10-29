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
  const deviceControls = document.querySelector('[data-device-controls]');
  if (deviceControls) {
    const search = deviceControls.querySelector('[data-device-search]');
    const sort = deviceControls.querySelector('[data-device-sort]');
    const sortSummary = sort.querySelector('summary');
    const sortMenu = sort.querySelector('[role=menu]');
    const sortChoices = [...sort.querySelectorAll('[data-device-sort-option]')];
    const sortLabels = new Map(sortChoices.map(choice => [choice.dataset.deviceSortOption, choice.querySelector('[data-sort-option-label]').textContent]));
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
    const closeSortMenu = (focus = false) => {
      sort.open = false;
      sortSummary.setAttribute('aria-expanded', 'false');
      if (focus) sortSummary.focus({preventScroll: true});
    };
    const placeSortMenu = () => {
      if (!sort.open) return;
      const anchor = sortSummary.getBoundingClientRect();
      if (anchor.bottom < 0 || anchor.top > innerHeight) { closeSortMenu(); return; }
      const width = Math.min(250, innerWidth - 24);
      const below = innerHeight - anchor.bottom - 12, above = anchor.top - 12;
      const down = below >= Math.min(340, above);
      sortMenu.style.width = `${width}px`;
      sortMenu.style.maxHeight = `${Math.max(80, Math.min(380, down ? below : above, innerHeight - 24))}px`;
      sortMenu.style.left = `${Math.max(12, Math.min(anchor.left, innerWidth - width - 12))}px`;
      const height = sortMenu.getBoundingClientRect().height;
      sortMenu.style.top = `${Math.max(12, Math.min(down ? anchor.bottom + 7 : anchor.top - height - 7, innerHeight - height - 12))}px`;
    };
    const focusSortChoice = choice => {
      choice.focus({preventScroll: true});
      choice.scrollIntoView({block: 'nearest', inline: 'nearest'});
    };
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
      sort.dataset.value = sortKey;
      sort.querySelector('[data-device-sort-label]').textContent = sortLabels.get(sortKey);
      for (const choice of sortChoices) {
        const active = choice.dataset.deviceSortOption === sortKey;
        choice.setAttribute('aria-checked', String(active));
        choice.tabIndex = active ? 0 : -1;
      }
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
    for (const choice of sortChoices) choice.addEventListener('click', () => {
      chooseSort(choice.dataset.deviceSortOption);
      closeSortMenu(true);
    });
    for (const header of headers) header.addEventListener('click', () => {
      chooseSort(header.dataset.deviceColumnSort, true);
      closeSortMenu();
    });
    sort.addEventListener('toggle', () => {
      sortSummary.setAttribute('aria-expanded', String(sort.open));
      if (sort.open) {
        closePicker(); placeSortMenu();
        const visibleChoice = sortChoices.includes(document.activeElement) ? document.activeElement : sortChoices.find(choice => choice.getAttribute('aria-checked') === 'true');
        visibleChoice.scrollIntoView({block: 'nearest', inline: 'nearest'});
      }
    });
    sortSummary.addEventListener('keydown', event => {
      if (!['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(event.key)) return;
      event.preventDefault();
      if (sort.open && ['Enter', ' '].includes(event.key)) { closeSortMenu(true); return; }
      sort.open = true;
      placeSortMenu();
      const choice = event.key === 'ArrowUp' ? sortChoices.at(-1) : sortChoices.find(option => option.getAttribute('aria-checked') === 'true');
      focusSortChoice(choice);
    });
    sort.addEventListener('keydown', event => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); closeSortMenu(true); return; }
      if (!event.target.matches('[data-device-sort-option]') || !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault();
      const index = sortChoices.indexOf(event.target);
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? sortChoices.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : -1) + sortChoices.length) % sortChoices.length;
      for (const [i, option] of sortChoices.entries()) option.tabIndex = i === next ? 0 : -1;
      focusSortChoice(sortChoices[next]);
    });
    for (const event of ['pointerdown', 'focusin']) document.addEventListener(event, e => {
      if (sort.open && !sort.contains(e.target)) closeSortMenu();
    });
    for (const picker of document.querySelectorAll('[data-picker]')) picker.addEventListener('toggle', () => {
      if (picker.open) closeSortMenu();
    });
    addEventListener('resize', placeSortMenu);
    addEventListener('scroll', placeSortMenu, true);
    search.addEventListener('input', filter);
    direction.addEventListener('click', () => { descending = !descending; order(); });
    clear.addEventListener('click', () => { search.value = ''; filter(); search.focus(); });
    deviceControls.hidden = false;
    order();
  }
  const report = document.querySelector('[data-relay-report]');
  if (report) {
    const search = report.querySelector('[data-log-search]'), level = report.querySelector('[data-log-level]');
    const rows = [...report.querySelectorAll('[data-log-event]')];
    const filter = () => {
      const words = search.value.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
      let visible = 0;
      for (const row of rows) {
        const match = (!level.value || (level.value === 'attention' ? ['WARN', 'ERROR'].includes(row.dataset.level) : row.dataset.level === level.value)) && words.every(word => row.textContent.toLocaleLowerCase().includes(word));
        row.hidden = !match; if (match) visible++;
      }
      report.querySelector('[data-log-count]').textContent = `${visible} of ${rows.length} events · newest first · UTC`;
      report.querySelector('[data-log-empty]').hidden = visible !== 0;
    };
    search.addEventListener('input', filter); level.addEventListener('change', filter);
  }
  document.querySelector('[data-group-search]')?.addEventListener('input', event => {
    const words = event.target.value.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
    const rows = [...document.querySelectorAll('[data-group-row]')];
    for (const row of rows) row.hidden = !words.every(word => row.dataset.groupSearchText.toLocaleLowerCase().includes(word));
    document.querySelector('[data-group-empty]').hidden = rows.some(row => !row.hidden);
    closePicker();
  });
})();
