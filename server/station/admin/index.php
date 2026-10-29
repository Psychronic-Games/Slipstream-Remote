<?php
/**
 * Slipstream Station — admin page: first-run setup, status, devices, settings.
 */
require __DIR__ . '/../lib/slipstream.php';

header('Cache-Control: no-store');
header('X-Frame-Options: DENY');
slip_session_start('slipstream_admin');

if (empty($_SESSION['csrf'])) {
    $_SESSION['csrf'] = bin2hex(random_bytes(16));
}
$csrf = $_SESSION['csrf'];
$post = ($_SERVER['REQUEST_METHOD'] ?? '') === 'POST';
$action = $post ? (string)($_POST['action'] ?? '') : '';
if ($post && !hash_equals($csrf, (string)($_POST['csrf'] ?? ''))) {
    http_response_code(400);
    exit('Form expired — go back and try again.');
}
$flash = $_SESSION['flash'] ?? null;
unset($_SESSION['flash']);
function flash(string $msg, bool $err = false): void
{
    $_SESSION['flash'] = [$msg, $err];
}
function back(string $tab = ''): void
{
    header('Location: ./' . ($tab ? "?tab=$tab" : ''));
    exit;
}
$form = fn() => '<input type="hidden" name="csrf" value="' . h($GLOBALS['csrf']) . '">';

// ── First-run setup ────────────────────────────────────────────────────────
if (!slip_station_ready()) {
    $err = null;
    if ($action === 'setup') {
        $org = trim((string)($_POST['org'] ?? ''));
        $pw = (string)($_POST['password'] ?? '');
        if (strlen($pw) < 10) {
            $err = 'Use an admin password of at least 10 characters.';
        } elseif ($pw !== ($_POST['password2'] ?? '')) {
            $err = 'The passwords do not match.';
        } elseif (!is_writable(slip_data_dir())) {
            $err = 'The data folder ' . slip_data_dir() . ' is not writable by PHP.';
        } else {
            slip_station_save(slip_new_station(slip_clip($org, 80), $pw));
            slip_write_htaccess();
            slip_relay_ensure();
            session_regenerate_id(true);
            $_SESSION['admin'] = true;
            flash('Station ready. Next: add the client downloads (see Status), then share ' . slip_base_url() . ' with your clients.');
            back();
        }
    }
    slip_page_start('Set up Slipstream Station', '../');
    ?>
    <h2>Set up your station</h2>
    <div class="panel">
      <p>This site becomes your organisation's own private Slipstream service: a directory of your
      clients' devices, a relay, and a download page with apps preconfigured for it.</p>
      <?php if ($err): ?><div class="flash err"><?= h($err) ?></div><?php endif ?>
      <form method="post"><?= $form() ?><input type="hidden" name="action" value="setup">
        <label class="k">Organisation name (shown to your clients)</label>
        <input type="text" name="org" value="<?= h($_POST['org'] ?? '') ?>" placeholder="Acme IT Support" required>
        <label class="k">Admin password</label>
        <input type="password" name="password" required minlength="10">
        <label class="k">Admin password again</label>
        <input type="password" name="password2" required minlength="10">
        <br><button>Create station</button>
      </form>
    </div>
    <?php
    slip_page_end();
    exit;
}

$station = slip_station();

// ── Login ──────────────────────────────────────────────────────────────────
if ($action === 'logout') {
    session_destroy();
    back();
}
if (empty($_SESSION['admin'])) {
    $err = null;
    if ($action === 'login') {
        if (!slip_rate_ok('adminlogin', 8)) {
            $err = 'Too many attempts — wait a minute.';
        } elseif (password_verify((string)($_POST['password'] ?? ''), $station['admin_hash'])) {
            session_regenerate_id(true);
            $_SESSION['admin'] = true;
            back();
        } else {
            $err = 'Wrong password.';
        }
    }
    slip_page_start('Station admin', '../');
    ?>
    <h2>Station admin</h2>
    <div class="panel">
      <?php if ($err): ?><div class="flash err"><?= h($err) ?></div><?php endif ?>
      <form method="post"><?= $form() ?><input type="hidden" name="action" value="login">
        <label class="k">Admin password</label><input type="password" name="password" autofocus>
        <br><button>Sign in</button></form>
    </div>
    <?php
    slip_page_end();
    exit;
}

// ── Actions ────────────────────────────────────────────────────────────────
switch ($action) {
    case 'relay_restart':
        slip_write_htaccess();
        flash(slip_relay_ensure(true) ? 'Relay restarted.' : 'The relay could not be started — see the checks below.', !slip_relay_running());
        back();
    case 'relay_stop':
        slip_relay_stop();
        flash('Relay stopped. It starts again automatically when clients connect.');
        back();
    case 'delete_device':
        $id = (int)($_POST['id'] ?? 0);
        slip_store()->delete($id);
        slip_shared_update(fn(array $list) => array_filter($list, fn($x) => ($x['id'] ?? 0) !== $id));
        flash('Device removed. It will get a new ID if it connects again.');
        back('devices');
    case 'station_group_create':
    case 'station_group_rename':
    case 'station_group_delete':
        try {
            slip_station_group(substr($action, strlen('station_group_')), $_POST['group'] ?? null, $_POST['new_name'] ?? null);
            flash('Station groups updated. Personal technician groups are kept separately.');
        } catch (InvalidArgumentException $e) { flash($e->getMessage(), true); }
        back('groups');
    case 'station_group_move':
        try {
            if (!is_array($_POST['ids'] ?? null)) throw new InvalidArgumentException('Choose at least one workstation.');
            $moved = slip_move_shared_devices($_POST['ids'], $_POST['group'] ?? null);
            flash("Moved $moved workstation(s). Personal technician groups are unchanged.");
        } catch (InvalidArgumentException $e) { flash($e->getMessage(), true); }
        back('groups');
    case 'group_visibility':
        try {
            $usernames = $_POST['usernames'] ?? [];
            if (!is_array($usernames)) throw new InvalidArgumentException('Choose valid technicians.');
            slip_set_group_assignment($_POST['owner'] ?? null, $_POST['group'] ?? null, $usernames);
            flash('Group assignments saved. Selected technicians see the current group on their next list refresh. Device passwords are still required.');
        } catch (InvalidArgumentException $e) { flash($e->getMessage(), true); }
        back('groups');
    case 'device_assign':
        try {
            $usernames = $_POST['usernames'] ?? [$_POST['username'] ?? ''];
            if (!is_array($usernames)) throw new InvalidArgumentException('Choose at least one technician.');
            $result = slip_assign_devices($usernames, (int)($_POST['id'] ?? 0));
            $message = count($result['added']) . ' added; ' . count($result['existing'])
                . ' already assigned. Existing details and saved passwords were kept.';
            foreach ($result['failed'] as $username => $reason) $message .= " $username: $reason";
            $message .= ' Technicians still need the device password to connect.';
            flash($message, !empty($result['failed']));
        } catch (InvalidArgumentException $e) {
            flash($e->getMessage(), true);
        }
        back('devices');
    case 'settings':
        $changes = [
            'org' => slip_clip(trim((string)($_POST['org'] ?? '')), 80),
            'motd' => slip_clip(trim((string)($_POST['motd'] ?? '')), 300),
            'private' => !empty($_POST['private']),
            'public_relays' => !empty($_POST['public_relays']),
            'approve_links' => !empty($_POST['approve_links']),
            'auto_update' => !empty($_POST['auto_update']),
        ];
        $dl = (string)($_POST['download_password'] ?? '');
        if (!empty($_POST['clear_download_password'])) {
            $changes['download_password_hash'] = null;
        } elseif ($dl !== '') {
            $changes['download_password_hash'] = password_hash($dl, PASSWORD_DEFAULT);
        }
        $privacyChanged = $changes['private'] !== !empty($station['private']);
        slip_station_save($changes);
        if ($privacyChanged) {
            slip_relay_ensure(true); // the relay enforces the key, so restart it
        }
        flash('Settings saved.');
        back('settings');
    case 'update_check':
        $u = slip_update_check(true);
        flash($u['error'] ?? null ? 'Couldn\'t check for updates: ' . $u['error'] : (slip_update_available($u)
            ? 'Version ' . $u['latest']['version'] . ' is available.' : 'The station is up to date.'), (bool)($u['error'] ?? null));
        back();
    case 'update_step':
        // One piece of the install per request (the page calls this until done).
        header('Content-Type: application/json');
        echo json_encode(slip_update_step());
        exit;
    case 'rotate_key':
        slip_station_save(['access_key' => bin2hex(random_bytes(16))]);
        slip_relay_ensure(true);
        flash('New access key created. Existing apps stop working until they are downloaded again.', true);
        back('settings');
    case 'tech_add':
        try {
            slip_user_create((string)($_POST['username'] ?? ''), trim((string)($_POST['name'] ?? '')), (string)($_POST['password'] ?? ''), !empty($_POST['admin']));
            flash('Technician added. They sign in from the app under Config → Stations with ' . slip_server_address() . ', their username and password.');
        } catch (InvalidArgumentException $e) {
            flash($e->getMessage(), true);
        }
        back('techs');
    case 'tech_password':
        try {
            slip_user_set_password(slip_user_norm($_POST['username'] ?? ''), (string)($_POST['password'] ?? ''));
            flash('Password changed. They are signed out everywhere, and passwords they had saved for devices are cleared.');
        } catch (InvalidArgumentException $e) {
            flash($e->getMessage(), true);
        }
        back('techs');
    case 'tech_admin':
        $u = slip_user_norm($_POST['username'] ?? '');
        $on = !empty($_POST['admin']);
        slip_user_update($u, function (array $a) use ($on) {
            $a['admin'] = $on;
            return $a;
        });
        flash($on ? "$u can now change the shared devices." : "$u is no longer a station admin.");
        back('techs');
    case 'tech_signout':
        slip_user_signout_all(slip_user_norm($_POST['username'] ?? ''));
        flash('Signed out on every computer.');
        back('techs');
    case 'tech_delete':
        slip_user_delete(slip_user_norm($_POST['username'] ?? ''));
        flash('Technician removed, with their saved devices.');
        back('techs');
    case 'shared_add':
        $input = ['id' => (int)preg_replace('/\D/', '', (string)($_POST['id'] ?? '')), 'name' => $_POST['name'] ?? '', 'notes' => $_POST['notes'] ?? ''];
        if (isset($_POST['group'])) $input['group'] = $_POST['group'];
        $d = slip_clean_device($input, false);
        if (!$d) {
            flash('Enter the device\'s 9-digit ID.', true);
        } else {
            slip_shared_update(function (array $list) use ($d) {
                $old = array_values(array_filter($list, fn($x) => ($x['id'] ?? 0) === $d['id']))[0] ?? [];
                $list = array_filter($list, fn($x) => ($x['id'] ?? 0) !== $d['id']);
                $d['group'] = $d['group'] ?? ($old['group'] ?? '');
                $list[] = $d + ['added_by' => $old['added_by'] ?? 'admin page', 'added' => $old['added'] ?? time()];
                return $list;
            });
            flash('Station device saved. Its group assignments control which technicians see it.');
        }
        back(($_POST['return_tab'] ?? '') === 'devices' ? 'devices' : 'techs');
    case 'shared_delete':
        $id = (int)($_POST['id'] ?? 0);
        slip_shared_update(fn(array $list) => array_filter($list, fn($x) => ($x['id'] ?? 0) !== $id));
        flash('Removed from the shared list.');
        back('techs');
    case 'admin_password':
        $pw = (string)($_POST['password'] ?? '');
        if (strlen($pw) < 10) {
            flash('Use at least 10 characters.', true);
        } else {
            slip_station_save(['admin_hash' => password_hash($pw, PASSWORD_DEFAULT)]);
            flash('Admin password changed.');
        }
        back('settings');
}

// Make sure the relay is up whenever an admin looks.
slip_write_htaccess();
$relayUp = slip_relay_ensure();
$station = slip_station();
$tab = (string)($_GET['tab'] ?? 'status');

slip_page_start('Station admin', '../');
?>
<nav class="tabs">
  <a href="./" class="<?= $tab === 'status' ? 'on' : '' ?>">Status</a>
  <a href="?tab=devices" class="<?= $tab === 'devices' ? 'on' : '' ?>">Devices</a>
  <a href="?tab=groups" class="<?= $tab === 'groups' ? 'on' : '' ?>">Groups</a>
  <a href="?tab=techs" class="<?= $tab === 'techs' ? 'on' : '' ?>">Technicians</a>
  <a href="?tab=settings" class="<?= $tab === 'settings' ? 'on' : '' ?>">Settings</a>
  <a href="../" target="_blank">Download page ↗</a>
  <form method="post" style="margin-left:auto"><?= $form() ?><input type="hidden" name="action" value="logout"><button class="small lav">Sign out</button></form>
</nav>
<?php if ($flash): ?><div class="flash <?= $flash[1] ? 'err' : '' ?>"><?= h($flash[0]) ?></div><?php endif ?>

<?php if ($tab === 'status'):
    $store = null;
    $storeErr = null;
    try {
        $store = slip_store();
    } catch (Throwable $e) {
        $storeErr = $e->getMessage();
    }
    [$total, $online] = $store ? $store->counts() : [0, 0];
    $builds = slip_client_builds();
    $bin = slip_relay_binary();
    // Is Apache forwarding /relay to us? (/ping answers 200 only from the relay.)
    $ctx = stream_context_create(['http' => ['timeout' => 3, 'ignore_errors' => true]]);
    $ping = @fopen(slip_base_url() . 'ping', 'r', false, $ctx);
    $status = $ping ? (string)(stream_get_meta_data($ping)['wrapper_data'][0] ?? '') : '';
    if ($ping) {
        fclose($ping);
    }
    $proxied = strpos($status, ' 200') !== false;
    $dataDir = realpath(slip_data_dir());
    $docRoot = realpath($_SERVER['DOCUMENT_ROOT'] ?? '') ?: '';
    $dataInWebRoot = $docRoot && $dataDir && strpos($dataDir, $docRoot) === 0;
    $checks = [
        ['PHP ' . PHP_VERSION, version_compare(PHP_VERSION, '8.0', '>='), 'PHP 8.0 or newer is required.'],
        ['Signature checking (sodium)', function_exists('sodium_crypto_sign_verify_detached'), 'Enable the PHP sodium extension.'],
        ['Device storage: ' . ($store ? $store->name() : 'unavailable') . ($dataDir ? " ($dataDir)" : ''), $store !== null, (string)$storeErr],
        ['Device records outside the web root', !$dataInWebRoot, 'Records are in the web root, protected only by .htaccess. Set data_dir in config.php to a folder outside it.'],
        ['HTTPS', slip_scheme() === 'https', 'Enable HTTPS (e.g. Let\'s Encrypt) — apps expect it.'],
        ['Relay program present' . ($bin ? ' (' . basename($bin) . ')' : ''), $bin !== null, 'Upload slipstream-server-linux-' . php_uname('m') . ' into the bin/ folder.'],
        ['PHP may start the relay', slip_can_exec(), 'proc_open is disabled on this host. Run the relay as a service instead (see docs/SERVER.md).'],
        ['Relay running on 127.0.0.1:' . ($station['relay_port'] ?? '?'), $relayUp, 'See the relay log below.'],
        ['Web server forwards /relay to it', $proxied, 'Needs Apache with mod_rewrite + mod_proxy_http and AllowOverride FileInfo. Nginx: see docs/SERVER.md.'],
    ];
    ?>
  <h2>Station</h2>
  <div class="panel grid">
    <div><span class="k">Devices</span><div class="big"><?= (int)$total ?></div></div>
    <div><span class="k">Online now</span><div class="big"><?= (int)$online ?></div></div>
    <div><span class="k">Relay</span><div class="big <?= $relayUp && $proxied ? 'ok' : 'bad' ?>"><?= $relayUp && $proxied ? 'ACTIVE' : ($relayUp ? 'NOT ROUTED' : 'DOWN') ?></div></div>
    <div><span class="k">Access</span><div class="big"><?= !empty($station['private']) ? 'PRIVATE' : 'OPEN' ?></div></div>
  </div>

  <h2 class="blue">Share with your clients</h2>
  <div class="panel">
    <span class="k">Download page</span><div class="big"><a href="<?= h(slip_base_url()) ?>"><?= h(slip_base_url()) ?></a></div>
    <p class="dim">Apps downloaded there come preconfigured for this station<?= !empty($station['private']) ? ', including its access key' : '' ?>.
    Technicians can use the same downloads.</p>
  </div>

  <h2 class="lav">Checks</h2>
  <div class="panel"><table>
    <?php foreach ($checks as [$label, $ok, $hint]): ?>
      <tr><td style="width:2em" class="<?= $ok ? 'ok' : 'bad' ?>"><?= $ok ? '✔' : '✘' ?></td>
        <td><?= h($label) ?><?php if (!$ok && $hint): ?><br><span class="dim"><?= h($hint) ?></span><?php endif ?></td></tr>
    <?php endforeach ?>
  </table>
  <form method="post" style="display:inline"><?= $form() ?><input type="hidden" name="action" value="relay_restart"><button class="blue">Restart relay</button></form>
  <form method="post" style="display:inline"><?= $form() ?><input type="hidden" name="action" value="relay_stop"><button class="lav">Stop relay</button></form>
  </div>

  <?php
    $upd = slip_update_state();
    if (time() - (int)($upd['checked'] ?? 0) > 3600 && slip_release_keys()) {
        $upd = slip_update_check(true);
    }
    $installed = slip_installed();
    $avail = slip_update_available($upd);
    $fpm = function_exists('fastcgi_finish_request') || function_exists('litespeed_finish_request')
        || (slip_can_exec() && slip_php_cli() !== null);
  ?>
  <h2 class="violet">Station software</h2>
  <div class="panel">
    <p>Installed: version <b><?= h($installed['version']) ?></b> (build <?= (int)$installed['build'] ?><?= $installed['commit'] !== '' ? ' · ' . h($installed['commit']) : '' ?>).
    <?php if (!slip_release_keys()): ?>
      <span class="warn">No release key (lib/release-keys.txt): this station can't check signed updates. Install a release package from GitHub.</span>
    <?php elseif ($avail): ?>
      <span class="ok">Version <b><?= h($avail['version']) ?></b> (build <?= (int)$avail['build'] ?>) is available<?= $avail['page'] ? ' — <a href="' . h($avail['page']) . '" target="_blank" rel="noopener">release notes</a>' : '' ?>.</span>
    <?php elseif (!empty($upd['checked'])): ?>
      <span class="dim">Up to date (checked <?= h(date('Y-m-d H:i', (int)$upd['checked'])) ?>).</span>
    <?php endif ?></p>
    <?php if (!empty($upd['error'])): ?><p class="warn">Last update problem: <?= h($upd['error']) ?></p><?php endif ?>
    <p class="dim"><?= slip_update_auto()
        ? ($fpm ? 'New releases install automatically in the background.' : 'Automatic installs need PHP-FPM or PHP\'s command-line program (php_cli in config.php); this server will only show new releases here.')
        : 'Automatic installs are off (Settings).' ?>
      Updates replace the station's files, relay and apps; settings, devices and accounts stay as they are.</p>
    <form method="post" style="display:inline"><?= $form() ?><input type="hidden" name="action" value="update_check"><button class="lav">Check now</button></form>
    <?php if ($avail): ?>
      <button class="gold" id="install-update">Install update</button>
      <div id="update-progress" class="dim"></div>
      <script>
      document.getElementById('install-update').onclick = async function () {
        this.disabled = true;
        const out = document.getElementById('update-progress');
        const body = new URLSearchParams({action: 'update_step', csrf: <?= json_encode($csrf) ?>});
        for (let i = 0; i < 60; i++) {
          let r;
          try {
            r = await (await fetch('./', {method: 'POST', body})).json();
          } catch (e) {
            out.textContent = 'Lost contact with the station — reload the page to continue.';
            return;
          }
          out.textContent = r.message;
          if (r.done) { if (!r.error) setTimeout(() => location.reload(), 1500); return; }
          if (r.busy) await new Promise(ok => setTimeout(ok, 3000));
        }
      };
      </script>
    <?php endif ?>
  </div>

  <h2 class="gold">Client downloads</h2>
  <div class="panel">
    <?php $man = slip_client_manifest(); ?>
    <p><?= $man ? 'Serving version <b>' . h($man['version']) . '</b> (build ' . (int)$man['build'] . ' · ' . h($man['commit'] ?? '') . '). Apps update to this automatically.'
            . (isset($man['signer'])
                ? ' Signed with release key <span class="mono">' . h(substr((string)$man['signer'], 0, 16)) . '…</span>'
                : ' <span class="warn">Not signed: apps with signed updates won\'t install it. Rebuild with the current scripts/build-station.sh.</span>')
        : '<span class="warn">No manifest.json — apps can\'t auto-update. Upload builds made with scripts/build-station.sh.</span>' ?></p>
    <table>
    <?php foreach ($builds as $os => $b): ?>
      <tr><td><?= h($b['label']) ?></td><td class="mono"><?= h($b['file']) ?></td>
        <td class="<?= $b['path'] ? 'ok' : 'warn' ?>"><?= $b['path'] ? round($b['size'] / 1048576) . ' MB, ' . date('Y-m-d', filemtime($b['path'])) : 'missing — upload to clients/' ?></td></tr>
    <?php endforeach ?>
  </table></div>

  <?php $log = slip_relay_log_tail(); if ($log !== ''): ?>
  <h2 class="violet">Relay log (latest)</h2>
  <p class="dim">“Stream terminated, exiting” reports the end of an individual client relay stream. The relay status above shows whether the server is running. Timestamps are UTC.</p>
  <pre><?= h($log) ?></pre>
  <?php endif ?>

<?php elseif ($tab === 'devices'):
    $devices = slip_store()->all();
    $sharedById = array_column(slip_shared(), null, 'id');
    $technicians = slip_users();
    $private = slip_access_key() !== null;
    $stationGroups = slip_shared_groups();
    usort($devices, fn($a, $b) => ($b['last_seen'] ?? 0) <=> ($a['last_seen'] ?? 0));
    $cut = time() - SLIP_ONLINE_WINDOW;
    $ago = function ($t) {
        if (!$t) return 'signed off';
        $d = time() - $t;
        return $d < 90 ? 'just now' : ($d < 5400 ? round($d / 60) . ' min ago' : ($d < 172800 ? round($d / 3600) . ' h ago' : round($d / 86400) . ' days ago'));
    };
    ?>
  <h2>Devices</h2>
  <div class="panel">
    <p class="dim"><?= $private ? 'Workstations automatically enroll in the station under Ungrouped. Assign their group to technicians to show them in those apps. Names and groups you set stay as you left them. Connecting still requires the device password.' : 'Public directory: automatic shared enrollment and hardware inventory are disabled.' ?> <a href="?tab=groups">Organize groups and assign them to technicians</a></p>
    <datalist id="station-groups"><?php foreach ($stationGroups as $group): ?><option value="<?= h($group) ?>"><?php endforeach ?></datalist>
    <?php if (!$devices): ?><p class="dim">No devices yet. They appear here once someone runs the app from your download page.</p><?php else: ?>
    <table>
      <tr><th></th><th>ID</th><th>Name / group</th><th>System</th><?php if ($private): ?><th>Hardware</th><?php endif ?><th>Version</th><th>Last seen</th><th></th></tr>
      <?php foreach ($devices as $d):
          $on = ($d['last_seen'] ?? 0) >= $cut; $id = (string)$d['id'];
          $saved = $sharedById[$d['id']] ?? null;
          $inventory = $private ? slip_device_inventory($d['endpoint']) : [];
      ?>
        <tr>
          <td class="<?= $on ? 'ok' : 'dim' ?>">●</td>
          <td class="mono device-id"><?= h(substr($id, 0, 3) . ' ' . substr($id, 3, 3) . ' ' . substr($id, 6)) ?></td>
          <td><?= h($saved['name'] ?? $d['name'] ?? '') ?><div class="dim"><?= h(($saved['group'] ?? '') !== '' ? $saved['group'] : 'Ungrouped') ?></div>
            <?php if ($saved && ($saved['name'] ?? '') !== ($d['name'] ?? '')): ?><div class="dim">Computer: <?= h($d['name'] ?? '') ?></div><?php endif ?>
            <?php if ($private): ?>
            <details><summary>Edit shared device</summary>
              <form method="post"><?= $form() ?><input type="hidden" name="action" value="shared_add"><input type="hidden" name="return_tab" value="devices"><input type="hidden" name="id" value="<?= (int)$d['id'] ?>">
                <label class="k">Name</label><input type="text" name="name" maxlength="64" value="<?= h($saved['name'] ?? $d['name'] ?? '') ?>">
                <label class="k">Group</label><input type="text" name="group" maxlength="64" list="station-groups" placeholder="Ungrouped" value="<?= h($saved['group'] ?? '') ?>">
                <label class="k">Notes</label><textarea name="notes" maxlength="500"><?= h($saved['notes'] ?? '') ?></textarea>
                <button class="small">Save shared device</button>
              </form>
            </details>
            <?php endif ?>
          </td>
          <td><?= h($inventory['os_version'] ?? $d['os'] ?? '') ?><?php if (!empty($inventory['architecture'])): ?><div class="dim"><?= h($inventory['architecture']) ?></div><?php endif ?></td>
          <?php if ($private): ?><td>
            <?php if ($inventory): ?>
              <div><?= h($inventory['model'] ?? 'Model unavailable') ?></div>
              <?php if (!empty($inventory['processor'])): ?><div class="dim"><?= h($inventory['processor']) ?></div><?php endif ?>
              <?php if (!empty($inventory['memory_bytes'])): ?><div><?= h(round($inventory['memory_bytes'] / 1073741824, 1)) ?> GiB RAM</div><?php endif ?>
              <div class="mono">Serial: <?= h($inventory['serial_number'] ?? 'Unavailable') ?></div>
              <div class="dim">Reported <?= h(gmdate('Y-m-d H:i', $inventory['reported_at'])) ?> UTC</div>
            <?php else: ?><span class="dim">Awaiting host inventory</span><?php endif ?>
          </td><?php endif ?>
          <td class="dim"><?= h($d['app_version'] ?? '') ?></td>
          <td class="<?= $on ? 'ok' : 'dim' ?>"><?= $on ? 'online' : h($ago($d['last_seen'] ?? 0)) ?></td>
          <td>
            <?php if ($technicians): ?>
            <details><summary>Assign to technician</summary>
              <form method="post"><?= $form() ?><input type="hidden" name="action" value="device_assign"><input type="hidden" name="id" value="<?= (int)$d['id'] ?>">
                <fieldset class="technician-choices"><legend class="k">Select technicians</legend>
                  <?php foreach ($technicians as $username => $technician): ?><label><input type="checkbox" name="usernames[]" value="<?= h($username) ?>"><?= h(($technician['name'] ?? $username) . ' (' . $username . ')') ?></label><?php endforeach ?>
                </fieldset>
                <p class="dim">Adds the current name and group to their personal list. Existing entries and saved passwords stay intact.</p>
                <button class="small blue">Add to selected lists</button>
              </form>
            </details>
            <?php else: ?><a href="?tab=techs">Add a technician first</a><?php endif ?>
            <form method="post" onsubmit="return confirm('Remove this device?')"><?= $form() ?>
            <input type="hidden" name="action" value="delete_device"><input type="hidden" name="id" value="<?= (int)$d['id'] ?>">
            <button class="small red">Remove</button></form></td>
        </tr>
      <?php endforeach ?>
    </table>
    <?php endif ?>
  </div>

<?php elseif ($tab === 'groups'):
    $state = slip_shared_state(); $groups = slip_shared_groups($state); $shared = $state['devices']; $technicians = slip_users();
    $catalog = slip_group_catalog($state, $technicians);
    $technicianLabel = fn($u) => ($technicians[$u]['name'] ?? $u) . ' (' . $u . ')';
    ?>
  <h2>Groups &amp; technician access</h2>
  <div class="panel">
    <p>All station and personal technician groups are listed here. Each row shows its source and exactly who receives it in their app.</p>
    <p>Station groups are <b>hidden until assigned</b>, including Ungrouped. Personal groups always remain visible to their owner. Shared lists follow the group as workstations are added or moved. Saved passwords stay private, and connecting still requires the workstation’s password.</p>
    <p class="dim">Removing an assignment removes that shared group on the next app list refresh. Devices a technician separately saved or received through an earlier copy remain in their personal list.</p>
    <?php if (!$catalog): ?><p class="dim">No groups yet. Create a station group below, or create a personal group in the app.</p><?php else: ?>
    <div class="table-scroll"><table class="group-access">
      <tr><th>Group</th><th>Source</th><th>Workstations</th><th>Visible to</th><th>Manage</th></tr>
      <?php foreach ($catalog as $entry):
          $owner = $entry['owner']; $group = $entry['group']; $count = count($entry['devices']);
          $recipients = array_values(array_filter($entry['usernames'], fn($u) => isset($technicians[$u])));
          ?>
      <tr>
        <td data-label="Group"><b><?= h($group === '' ? 'Ungrouped' : $group) ?></b></td>
        <td data-label="Source"><?= $owner === '' ? 'Station' : h($technicianLabel($owner)) ?><div class="dim"><?= $owner === '' ? 'Managed by station admins' : 'Personal technician group' ?></div></td>
        <td data-label="Workstations"><details><summary><?= $count ?> workstation<?= $count === 1 ? '' : 's' ?></summary>
          <?php if (!$count): ?><p class="dim">Empty group</p><?php endif ?>
          <?php foreach ($entry['devices'] as $d): ?><div class="group-device"><span><?= h(($d['name'] ?? '') ?: 'Unnamed device') ?></span> <span class="mono device-id"><?= h((string)$d['id']) ?></span></div><?php endforeach ?>
        </details></td>
        <td data-label="Visible to">
          <?php if ($owner !== ''): ?><div><?= h($technicianLabel($owner)) ?> <span class="dim">(owner)</span></div><?php endif ?>
          <?php foreach ($recipients as $recipient): ?><div><?= h($technicianLabel($recipient)) ?></div><?php endforeach ?>
          <?php if (!$recipients): ?><span class="<?= $owner === '' ? 'warn' : 'dim' ?>"><?= $owner === '' ? 'No technicians assigned' : 'Owner only' ?></span><?php endif ?>
        </td>
        <td data-label="Manage"><details><summary>Edit assignments</summary>
          <form method="post"><?= $form() ?><input type="hidden" name="action" value="group_visibility"><input type="hidden" name="owner" value="<?= h($owner) ?>"><input type="hidden" name="group" value="<?= h($group) ?>">
            <p><b><?= h($group === '' ? 'Ungrouped' : $group) ?></b> · <?= $owner === '' ? 'Station' : h($technicianLabel($owner)) ?></p>
            <fieldset class="technician-choices"><legend class="k">Visible to these technicians</legend>
              <?php foreach ($technicians as $username => $technician): ?>
              <label><input type="checkbox" name="usernames[]" value="<?= h((string)$username) ?>" <?= in_array((string)$username, $recipients, true) || (string)$username === $owner ? 'checked' : '' ?> <?= (string)$username === $owner ? 'disabled' : '' ?>><?= h($technicianLabel($username)) ?><?= (string)$username === $owner ? ' · owner' : '' ?></label>
              <?php endforeach ?>
            </fieldset>
            <p class="dim">Clear all selections to <?= $owner === '' ? 'hide this group from every technician' : 'keep this group visible only to its owner' ?>. Multiple technicians may be selected.</p>
            <button class="small blue">Save group assignments</button>
          </form>
        </details>
        <?php if ($owner === '' && $group !== ''): ?>
          <details><summary>Rename group</summary><form method="post"><?= $form() ?><input type="hidden" name="action" value="station_group_rename"><input type="hidden" name="group" value="<?= h($group) ?>">
            <label class="k">New name</label><input type="text" name="new_name" maxlength="64" required value="<?= h($group) ?>"><button class="small blue">Rename group</button>
          </form></details>
          <?php if (!$count): ?><form method="post"><?= $form() ?><input type="hidden" name="action" value="station_group_delete"><input type="hidden" name="group" value="<?= h($group) ?>"><button class="small red">Remove empty group</button></form><?php endif ?>
        <?php endif ?>
        </td>
      </tr>
      <?php endforeach ?>
    </table></div><?php endif ?>
  </div>
  <h2>Create station group</h2>
  <div class="panel"><form method="post"><?= $form() ?><input type="hidden" name="action" value="station_group_create">
    <label class="k">New group</label><input type="text" name="group" maxlength="64" required placeholder="Workstations, Servers, Front desk…">
    <p class="dim">New groups are visible in station administration. Assign technicians above to show a group in their apps.</p>
    <button class="blue">Create group</button>
  </form></div>
  <?php if ($shared): ?>
  <h2>Move station workstations</h2>
  <div class="panel"><form method="post"><?= $form() ?><input type="hidden" name="action" value="station_group_move">
    <fieldset class="technician-choices"><legend class="k">Select workstations</legend>
      <?php foreach ($shared as $d): ?><label><input type="checkbox" name="ids[]" value="<?= (int)$d['id'] ?>"><?= h((($d['name'] ?? '') ?: $d['id']) . ' · ' . $d['id'] . ' · ' . (($d['group'] ?? '') ?: 'Ungrouped')) ?></label><?php endforeach ?>
    </fieldset>
    <label class="k">Move to</label><select name="group"><option value="">Ungrouped</option><?php foreach ($groups as $group): ?><option value="<?= h($group) ?>"><?= h($group) ?></option><?php endforeach ?></select>
    <p class="dim">The destination group's assignments determine who receives the workstation. Personal technician lists are preserved.</p>
    <button class="blue">Move selected workstations</button>
  </form></div>
  <?php endif ?>

<?php elseif ($tab === 'techs'):
    $users = slip_users();
    $shared = slip_shared();
    $catalog = slip_group_catalog(null, $users);
    $when = fn($t) => $t ? date('Y-m-d H:i', (int)$t) : '—';
    ?>
  <h2>Technicians</h2>
  <div class="panel">
    <p>Technicians sign in from the app under <b>Config → Stations</b> with this station's address
    (<span class="mono"><?= h(slip_server_address()) ?></span>), their username and password. They can then
    reach your devices and keep their own list of them, here, from any computer, even an app from another
    organisation's station. Devices they save a password for keep it encrypted with a key only their app can
    make from their password: this station can't read it.</p>
    <?php if (!$users): ?><p class="dim">No technicians yet.</p><?php else: ?>
    <div class="table-scroll"><table class="technician-table">
      <tr><th>Username</th><th>Name</th><th>Role</th><th>Personal / assigned groups</th><th>Devices</th><th>Signed in</th><th></th></tr>
      <?php foreach ($users as $u => $a): $sessions = (array)($a['sessions'] ?? []); $last = $sessions ? max(array_column($sessions, 'used')) : 0; ?>
        <tr>
          <td class="mono"><?= h($u) ?></td>
          <td><?= h($a['name'] ?? '') ?></td>
          <td><?= !empty($a['admin']) ? '<span class="ok">admin</span>' : 'technician' ?></td>
          <td><?php foreach ($catalog as $entry): if ($entry['owner'] !== (string)$u && !in_array((string)$u, $entry['usernames'], true)) continue; ?>
            <div><?= h($entry['group'] === '' ? 'Ungrouped' : $entry['group']) ?> <span class="dim">· <?= $entry['owner'] === (string)$u ? 'personal' : ($entry['owner'] === '' ? 'station' : h($entry['owner'])) ?></span></div>
          <?php endforeach ?><a href="?tab=groups">Manage group assignments</a></td>
          <td><?= count($a['devices'] ?? []) ?></td>
          <td class="dim"><?= count($sessions) ?> app<?= count($sessions) === 1 ? '' : 's' ?><?= $last ? ' · ' . h($when($last)) : '' ?></td>
          <td style="white-space:nowrap">
            <form method="post" style="display:inline"><?= $form() ?><input type="hidden" name="action" value="tech_admin">
              <input type="hidden" name="username" value="<?= h($u) ?>"><input type="hidden" name="admin" value="<?= empty($a['admin']) ? '1' : '' ?>">
              <button class="small lav"><?= empty($a['admin']) ? 'Make admin' : 'Remove admin' ?></button></form>
            <form method="post" style="display:inline"><?= $form() ?><input type="hidden" name="action" value="tech_signout">
              <input type="hidden" name="username" value="<?= h($u) ?>"><button class="small blue">Sign out everywhere</button></form>
            <form method="post" style="display:inline" onsubmit="return confirm('Remove <?= h($u) ?> and their saved devices?')"><?= $form() ?>
              <input type="hidden" name="action" value="tech_delete"><input type="hidden" name="username" value="<?= h($u) ?>">
              <button class="small red">Remove</button></form>
            <form method="post" style="display:inline"><?= $form() ?><input type="hidden" name="action" value="tech_password">
              <input type="hidden" name="username" value="<?= h($u) ?>">
              <input type="password" name="password" minlength="10" placeholder="new password" style="width:11em;display:inline">
              <button class="small">Set</button></form>
          </td>
        </tr>
      <?php endforeach ?>
    </table></div>
    <?php endif ?>
  </div>

  <h2 class="blue">Add a technician</h2>
  <div class="panel">
    <form method="post"><?= $form() ?><input type="hidden" name="action" value="tech_add">
      <label class="k">Username</label><input type="text" name="username" required pattern="[A-Za-z0-9][A-Za-z0-9._\-]{1,31}" placeholder="jsmith">
      <label class="k">Name</label><input type="text" name="name" placeholder="Jane Smith">
      <label class="k">Password (at least 10 characters)</label><input type="password" name="password" required minlength="10">
      <label><input type="checkbox" name="admin" value="1"> Station admin: may change the shared devices from the app</label>
      <br><button>Add technician</button>
    </form>
  </div>

  <h2 class="gold">Station devices</h2>
  <div class="panel">
    <p>These are the station’s workstations. Visibility in technician apps is controlled by each group’s assignments on the <a href="?tab=groups">Groups tab</a>. Passwords aren't stored for shared devices.</p>
    <?php if ($shared): ?>
    <table>
      <tr><th>ID</th><th>Name</th><th>Group</th><th>Notes</th><th>Added by</th><th></th></tr>
      <?php foreach ($shared as $d): $id = (string)$d['id']; ?>
        <tr><td class="mono device-id"><?= h(substr($id, 0, 3) . ' ' . substr($id, 3, 3) . ' ' . substr($id, 6)) ?></td>
          <td><?= h($d['name'] ?? '') ?></td><td><?= h(($d['group'] ?? '') ?: 'Ungrouped') ?></td><td class="dim"><?= h($d['notes'] ?? '') ?></td><td class="dim"><?= h($d['added_by'] ?? '') ?></td>
          <td><form method="post"><?= $form() ?><input type="hidden" name="action" value="shared_delete"><input type="hidden" name="id" value="<?= (int)$d['id'] ?>">
            <button class="small red">Remove</button></form></td></tr>
      <?php endforeach ?>
    </table>
    <?php endif ?>
    <form method="post"><?= $form() ?><input type="hidden" name="action" value="shared_add">
      <label class="k">Device ID</label><input type="text" name="id" required placeholder="482 913 077">
      <label class="k">Name</label><input type="text" name="name" placeholder="Front desk PC">
      <label class="k">Notes (optional)</label><input type="text" name="notes">
      <br><button class="gold">Save station device</button>
    </form>
  </div>

<?php else: ?>
  <h2>Settings</h2>
  <div class="panel">
    <form method="post"><?= $form() ?><input type="hidden" name="action" value="settings">
      <label class="k">Organisation name</label>
      <input type="text" name="org" value="<?= h($station['org'] ?? '') ?>">
      <label class="k">Message shown in every app (optional)</label>
      <input type="text" name="motd" value="<?= h($station['motd'] ?? '') ?>">
      <label><input type="checkbox" name="private" value="1" <?= !empty($station['private']) ? 'checked' : '' ?>>
        Private — only apps from this station's download page can use it (recommended)</label>
      <label><input type="checkbox" name="public_relays" value="1" <?= !empty($station['public_relays']) ? 'checked' : '' ?>>
        Allow apps to use Slipstream Public as a backup relay</label>
      <label><input type="checkbox" name="approve_links" value="1" <?= !empty($station['approve_links']) ? 'checked' : '' ?>>
        Ask the person at the computer to allow links made with its one-time password (unattended passwords never ask;
        Slipstream Public always asks)</label>
      <label><input type="checkbox" name="auto_update" value="1" <?= slip_update_auto() ? 'checked' : '' ?>>
        Install new Slipstream releases automatically (checked on GitHub every 6 hours, signature-verified)</label>
      <label class="k">Download page password (optional)</label>
      <input type="password" name="download_password" placeholder="<?= !empty($station['download_password_hash']) ? 'set — type to change' : 'none — anyone can download' ?>">
      <?php if (!empty($station['download_password_hash'])): ?>
        <label><input type="checkbox" name="clear_download_password" value="1"> Remove the download password</label>
      <?php endif ?>
      <br><button>Save settings</button>
    </form>
  </div>

  <h2 class="lav">Access key</h2>
  <div class="panel">
    <p>Apps from your download page carry this key. Anyone with the key can use your station, so if
    an app falls into the wrong hands, make a new key — then everyone needs to download the app again.</p>
    <div class="mono dim"><?= h(substr($station['access_key'], 0, 6)) ?>…<?= h(substr($station['access_key'], -4)) ?></div>
    <form method="post" onsubmit="return confirm('All existing apps will stop working until downloaded again. Continue?')"><?= $form() ?>
      <input type="hidden" name="action" value="rotate_key"><button class="red">Make a new access key</button></form>
  </div>

  <h2 class="violet">Admin password</h2>
  <div class="panel">
    <form method="post"><?= $form() ?><input type="hidden" name="action" value="admin_password">
      <input type="password" name="password" minlength="10" placeholder="new admin password"><br><button class="lav">Change password</button></form>
  </div>
<?php endif ?>
<?php slip_page_end();
