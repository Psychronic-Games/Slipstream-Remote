<?php
/** Small station controls. Only loaded by the authenticated admin page. */
if (!defined('SLIP_ROOT')) { http_response_code(404); exit; }

function slip_picker(string $name, string $label, array $options, array $selected = [], array $settings = []): void
{
    static $next = 0;
    $id = 'picker-' . ++$next;
    $selected = array_map('strval', $selected);
    $chosen = array_values(array_filter($options, fn($o) => in_array((string)$o['value'], $selected, true)));
    $placeholder = $settings['placeholder'] ?? 'Choose ' . strtolower($label);
    $summary = $chosen ? (count($chosen) === 1 ? $chosen[0]['label'] : count($chosen) . ' selected') : $placeholder;
    ?>
    <div class="picker-field">
      <span class="k" id="<?= $id ?>-label"><?= h($label) ?></span>
      <details class="multi-picker" data-picker data-name="<?= h($name) ?>" data-placeholder="<?= h($placeholder) ?>" <?= !empty($settings['create']) ? 'data-create' : '' ?>>
        <summary aria-labelledby="<?= $id ?>-label <?= $id ?>-selection"><span id="<?= $id ?>-selection" data-picker-summary><?= h($summary) ?></span><span class="picker-chevron" aria-hidden="true"></span></summary>
        <div class="picker-menu" role="group" aria-label="<?= h($label) ?>">
          <input type="search" data-picker-search placeholder="Search <?= h(strtolower($label)) ?>…" aria-label="Search <?= h(strtolower($label)) ?>" autocomplete="off">
          <div class="picker-options">
          <?php foreach ($options as $option): ?>
            <label data-picker-option><input type="checkbox" name="<?= h($name) ?>[]" value="<?= h((string)$option['value']) ?>" <?= in_array((string)$option['value'], $selected, true) ? 'checked' : '' ?> <?= !empty($option['disabled']) ? 'disabled' : '' ?>><span><span data-option-label><?= h($option['label']) ?></span><?php if (!empty($option['detail'])): ?><small><?= h($option['detail']) ?></small><?php endif ?></span></label>
          <?php endforeach ?>
          </div>
          <p class="picker-empty dim" data-picker-empty hidden>No matches</p>
          <?php if (!empty($settings['create'])): ?>
            <div class="picker-create"><label class="k">New group</label><input type="text" name="new_group" maxlength="64" placeholder="New group name" data-picker-new><button type="button" class="small blue" data-picker-create>Create &amp; select group</button></div>
          <?php endif ?>
          <div class="picker-footer"><span data-picker-status class="dim" aria-live="polite"><?= count($chosen) ?> selected</span><button type="button" class="small blue" data-picker-done>Done</button></div>
        </div>
      </details>
    </div>
    <?php
}

function slip_notes(string $value, string $id): void
{
    ?><div class="notes-editor"><textarea id="<?= h($id) ?>" name="notes" maxlength="500" rows="3" aria-label="Notes"><?= h($value) ?></textarea><div class="notes-resize" data-resize-notes role="separator" aria-orientation="horizontal" aria-label="Resize notes" aria-controls="<?= h($id) ?>" tabindex="0"><span aria-hidden="true"></span></div></div><?php
}

/** Parse bounded plain tracing/supervisor lines while retaining the original text. */
function slip_log_entries(string $text): array
{
    $entries = [];
    foreach (preg_split('/\r?\n/', trim($text)) as $line) {
        if ($line === '') continue;
        $entry = ['time' => '', 'level' => 'INFO', 'message' => $line, 'fields' => [], 'raw' => $line];
        if (preg_match('/^(\d{4}-\d\d-\d\dT\S+)\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+(\S+):\s*(.*)$/', $line, $match)) {
            $entry['time'] = $match[1]; $entry['level'] = $match[2];
            $body = $match[4];
            preg_match_all('/\b([A-Za-z_][A-Za-z0-9_]*)=("(?:\\\\.|[^"\\\\])*"|\S+)/', $body, $pairs, PREG_SET_ORDER | PREG_OFFSET_CAPTURE);
            $entry['message'] = trim($pairs ? substr($body, 0, $pairs[0][0][1]) : $body);
            foreach ($pairs as $pair) {
                $raw = $pair[2][0];
                $entry['fields'][$pair[1][0]] = str_starts_with($raw, '"') ? (json_decode($raw, true) ?? $raw) : $raw;
            }
        }
        $entries[] = $entry;
    }
    return array_reverse($entries);
}

function slip_log_size($bytes): string
{
    $bytes = max(0, (float)$bytes);
    foreach (['B', 'KiB', 'MiB', 'GiB'] as $unit) {
        if ($bytes < 1024 || $unit === 'GiB') return rtrim(rtrim(number_format($bytes, $unit === 'B' ? 0 : 1, '.', ''), $unit === 'B' ? '' : '0'), '.') . ' ' . $unit;
        $bytes /= 1024;
    }
    return '';
}

function slip_log_escape($value): string
{
    return htmlspecialchars((string)$value, ENT_QUOTES | ENT_SUBSTITUTE, 'UTF-8');
}

function slip_relay_events(array $entries): void
{
    ?>
      <?php foreach ($entries as $entry): $f = $entry['fields']; ?>
        <article class="relay-event" data-log-event data-level="<?= slip_log_escape($entry['level']) ?>">
          <div class="event-meta"><time datetime="<?= slip_log_escape($entry['time']) ?>" title="<?= slip_log_escape($entry['time']) ?>"><?= slip_log_escape($entry['time'] !== '' ? str_replace(['T', 'Z'], [' ', ''], preg_replace('/\.\d+Z$/', 'Z', $entry['time'])) : 'Time unavailable') ?></time><span class="level level-<?= strtolower($entry['level']) ?>"><?= slip_log_escape($entry['level']) ?></span></div>
          <div class="event-content"><strong><?= slip_log_escape($entry['message']) ?></strong>
            <div class="event-facts">
              <?php if (isset($f['device_name']) || isset($f['device_id'])): ?><span class="event-device"><?= slip_log_escape($f['device_name'] ?? 'Workstation') ?><?php if (isset($f['device_id'])): ?> <span class="mono"><?= slip_log_escape($f['device_id']) ?></span><?php endif ?></span><?php endif ?>
              <?php if (isset($f['peer'])): ?><span>Peer <span class="mono"><?= slip_log_escape($f['peer']) ?></span></span><?php endif ?>
              <?php if (isset($f['duration_ms']) && is_numeric($f['duration_ms'])): ?><span><?= slip_log_escape(round((float)$f['duration_ms'] / 1000, 1)) ?> s</span><?php endif ?>
              <?php if (isset($f['bytes_from_client'])): ?><span><?= slip_log_escape(slip_log_size($f['bytes_from_client'])) ?> received · <?= slip_log_escape(slip_log_size($f['bytes_to_client'] ?? 0)) ?> sent</span><?php endif ?>
              <?php if (isset($f['reason'])): ?><span><?= slip_log_escape(ucfirst(str_replace('_', ' ', $f['reason']))) ?></span><?php endif ?>
            </div>
            <details class="event-details"><summary>Technical details</summary><pre><?= slip_log_escape($entry['raw']) ?></pre></details>
          </div>
        </article>
      <?php endforeach ?>
    <?php
}

function slip_relay_report(array|string $page): void
{
    // Accept the previous caller while station files are replaced during updates.
    if (is_string($page)) $page = slip_relay_log_page();
    $entries = slip_log_entries($page['text']);
    $info = $page; unset($info['text']); $info['order'] = 'newest';
    ?>
    <section class="relay-report panel" data-relay-report data-log-page="<?= h(json_encode($info)) ?>" aria-label="Relay activity">
      <div class="report-toolbar">
        <label class="search-control"><span class="k">Search this page</span><input type="search" data-log-search placeholder="Device, address, event or message…"></label>
        <div class="device-sort-control log-level-control"><span class="k" id="relay-show-label">Show</span>
          <details class="choice-menu" data-choice-menu data-log-level data-value="">
            <summary aria-labelledby="relay-show-label relay-show-current" aria-haspopup="menu" aria-controls="relay-show-options" aria-expanded="false"><span id="relay-show-current" data-choice-label>All events</span><span class="picker-chevron" aria-hidden="true"></span></summary>
            <div class="choice-options" id="relay-show-options" role="menu" aria-labelledby="relay-show-label">
              <?php foreach (['' => 'All events', 'attention' => 'Warnings & errors', 'INFO' => 'Information'] as $key => $label): ?>
                <button type="button" role="menuitemradio" aria-checked="<?= $key === '' ? 'true' : 'false' ?>" tabindex="<?= $key === '' ? '0' : '-1' ?>" data-choice-option="<?= h($key) ?>"><span data-choice-option-label><?= h($label) ?></span><span class="sort-choice-mark" aria-hidden="true">✓</span></button>
              <?php endforeach ?>
            </div>
          </details>
        </div>
        <?php if ($page['available']): ?><a class="btn small blue log-export" href="?relay_log=download" data-log-download>Download full log (.txt)</a><?php endif ?>
      </div>
      <div class="log-navigation" data-log-navigation hidden aria-label="Relay log history">
        <button type="button" class="small lav" data-log-nav="oldest">Oldest</button>
        <button type="button" class="small blue" data-log-nav="older">← Older</button>
        <button type="button" class="small blue" data-log-nav="newer">Newer →</button>
        <button type="button" class="small lav" data-log-nav="newest" title="Show and refresh the latest events">Newest</button>
        <span class="dim" data-log-page-label>Latest retained events · newest first · UTC</span>
      </div>
      <p class="dim" data-log-count role="status" aria-live="polite"><?= count($entries) ?> events on this page</p>
      <p class="warn" data-log-continued <?= !$page['continued'] ? 'hidden' : '' ?>>A very long line continues on another page. Download the full log for unbroken text.</p>
      <p class="bad" data-log-error role="alert" hidden></p>
      <div class="relay-events" data-log-events><?php slip_relay_events($entries) ?></div>
      <p class="dim" data-log-empty <?= $entries ? 'hidden' : '' ?>>No matching activity on this page.</p>
    </section>
    <?php
}
