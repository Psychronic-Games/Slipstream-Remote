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
