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
$inlineSave = $post && $action === 'group_visibility'
    && str_contains(strtolower($_SERVER['HTTP_ACCEPT'] ?? ''), 'application/json');
if ($inlineSave) ini_set('display_errors', '0'); // Keep save failures valid JSON; PHP still logs them.
function slip_admin_json(array $data, int $status = 200): void
{
    http_response_code($status);
    header('Content-Type: application/json');
    echo json_encode($data, JSON_UNESCAPED_SLASHES);
    exit;
}
if ($post && !hash_equals($csrf, (string)($_POST['csrf'] ?? ''))) {
    if ($inlineSave) slip_admin_json(['ok' => false, 'error' => 'Form expired. Refresh the page and sign in again before retrying.'], 400);
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
    if ($inlineSave) slip_admin_json(['ok' => false, 'error' => 'This station has not been set up.'], 503);
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
            $username = slip_migrate_admin_account();
            slip_write_htaccess();
            slip_relay_ensure();
            session_regenerate_id(true);
            $_SESSION['admin_token'] = slip_token_issue($username, 'Station admin browser');
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
        <p>The first administrator account is <b class="mono">admin</b>. Add other administrators on the Technicians page.</p>
        <label class="k">Password for admin</label>
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

$bootstrapUsername = slip_migrate_admin_account();
$station = slip_station();

// ── Login ──────────────────────────────────────────────────────────────────
if ($action === 'logout') {
    slip_token_revoke($_SESSION['admin_token'] ?? null);
    $_SESSION = [];
    session_destroy();
    back();
}
// Revalidate account existence, role and sign-out on every request, including POSTs.
$admin = slip_token_user($_SESSION['admin_token'] ?? null);
unset($_SESSION['admin']); // Password-only sessions cannot authorize the new console.
if (!$admin || empty($admin['admin'])) {
    slip_token_revoke($_SESSION['admin_token'] ?? null);
    unset($_SESSION['admin_token']);
    if ($inlineSave) slip_admin_json(['ok' => false, 'error' => 'Your station admin sign-in has ended. Sign in again before retrying.'], 401);
    $err = null;
    if ($action === 'login') {
        if (!slip_rate_ok('adminlogin', 8)) {
            $err = 'Too many attempts — wait a minute.';
        } else {
            $username = slip_user_norm($_POST['username'] ?? '');
            $account = slip_user_load($username);
            $hash = $account['hash'] ?? '$2y$12$qbJtzQ04aH6DWxr8jJh0g.VgXGS8MRZznxb3JTdipy/6KCYInoaAu';
            if (password_verify((string)($_POST['password'] ?? ''), $hash) && !empty($account['admin'])) {
                session_regenerate_id(true);
                $_SESSION['admin_token'] = slip_token_issue($username, 'Station admin browser');
                back();
            }
            $err = 'Wrong username or password, or this account is not a station admin.';
        }
    }
    slip_page_start('Station admin', '../');
    ?>
    <h2>Station admin</h2>
    <div class="panel">
      <?php if ($err): ?><div class="flash err"><?= h($err) ?></div><?php endif ?>
      <form method="post"><?= $form() ?><input type="hidden" name="action" value="login">
        <p>Sign in with a technician account that has Station admin permission.</p>
        <p class="dim">For the first sign-in after upgrading, use the previous station password with <b class="mono"><?= h($bootstrapUsername) ?></b>.</p>
        <label class="k">Username</label><input type="text" name="username" value="<?= h($_POST['username'] ?? '') ?>" autocomplete="username" required autofocus>
        <label class="k">Password</label><input type="password" name="password" autocomplete="current-password" required>
        <br><button>Sign in</button></form>
    </div>
    <?php
    slip_page_end();
    exit;
}

require_once __DIR__ . '/components.php';

// ── Actions ────────────────────────────────────────────────────────────────
switch ($action) {
    case 'relay_restart':
        slip_write_htaccess();
        flash(slip_relay_ensure(true, 'admin_restart') ? 'Relay restarted.' : 'The relay could not be started — see the checks below.', !slip_relay_running());
        back();
    case 'relay_stop':
        slip_relay_stop();
        flash(slip_relay_running() ? 'Relay is still running — see the relay log.' : 'Relay stopped. It starts again automatically when clients connect.', slip_relay_running());
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
    case 'group_members':
    case 'group_visibility':
        try {
            if ($inlineSave && ($_POST['complete'] ?? '') !== '1') throw new InvalidArgumentException('The selections were incomplete. Nothing was saved; reduce the selection or contact the station administrator.');
            if ($action === 'group_members') {
                slip_set_group_members($_POST['owner'] ?? null, $_POST['group'] ?? null, $_POST['ids'] ?? []);
                $message = 'Group workstations saved. Other memberships and saved passwords were kept.';
            } else {
                $usernames = $_POST['usernames'] ?? [];
                if (!is_array($usernames)) throw new InvalidArgumentException('Choose valid technicians.');
                slip_set_group_assignment($_POST['owner'] ?? null, $_POST['group'] ?? null, $usernames);
                $message = 'Group assignments saved. Selected technicians see the current group on their next list refresh. Device passwords are still required.';
            }
            if ($inlineSave) slip_admin_json(['ok' => true, 'message' => $message]);
            flash($message);
        } catch (InvalidArgumentException $e) {
            if ($inlineSave) slip_admin_json(['ok' => false, 'error' => $e->getMessage()], 400);
            flash($e->getMessage(), true);
        } catch (Throwable $e) {
            if (!$inlineSave) throw $e;
            error_log('station group save: ' . $e->getMessage());
            slip_admin_json(['ok' => false, 'error' => 'Could not save the changes. Please try again.'], 500);
        }
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
            slip_relay_ensure(true, 'station_settings_changed'); // the relay enforces the key, so restart it
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
        slip_relay_ensure(true, 'access_key_rotated');
        flash('New access key created. Existing apps stop working until they are downloaded again.', true);
        back('settings');
    case 'tech_add':
        try {
            slip_user_create((string)($_POST['username'] ?? ''), trim((string)($_POST['name'] ?? '')), (string)($_POST['password'] ?? ''), !empty($_POST['admin']));
            flash('Technician added. Station admins can also sign in to this web console. App sign-in: ' . slip_server_address() . ', their username and password.');
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
        try {
            slip_user_set_admin($u, $on);
            flash($on ? "$u can now administer this station and its shared devices." : "$u is no longer a station admin.");
        } catch (InvalidArgumentException $e) { flash($e->getMessage(), true); }
        back('techs');
    case 'tech_signout':
        slip_user_signout_all(slip_user_norm($_POST['username'] ?? ''));
        flash('Signed out on every computer.');
        back('techs');
    case 'tech_delete':
        try {
            slip_user_delete(slip_user_norm($_POST['username'] ?? ''));
            flash('Technician removed, with their saved devices.');
        } catch (InvalidArgumentException $e) { flash($e->getMessage(), true); }
        back('techs');
    case 'shared_add':
        try {
            $input = ['id' => (int)preg_replace('/\D/', '', (string)($_POST['id'] ?? '')), 'name' => $_POST['name'] ?? '', 'notes' => $_POST['notes'] ?? ''];
            if (isset($_POST['group'])) $input['group'] = $_POST['group'];
            if (isset($_POST['groups_present'])) {
                $input['groups'] = $_POST['groups'] ?? [];
                if (!is_array($input['groups'])) throw new InvalidArgumentException('Choose valid workstation groups.');
                if (($_POST['new_group'] ?? '') !== '') $input['groups'][] = $_POST['new_group'];
            }
            $d = slip_clean_device($input, false);
            if (!$d) throw new InvalidArgumentException('Enter the device’s 9-digit ID.');
            slip_shared_update(function (array $list) use ($d) {
                $old = array_values(array_filter($list, fn($x) => ($x['id'] ?? 0) === $d['id']))[0] ?? [];
                $list = array_filter($list, fn($x) => ($x['id'] ?? 0) !== $d['id']);
                $d = slip_merge_device_groups($d, $old);
                $list[] = $d + ['added_by' => $old['added_by'] ?? 'admin page', 'added' => $old['added'] ?? time()];
                return $list;
            });
            flash('Station device saved. Group assignments control which technicians see it.');
        } catch (InvalidArgumentException $e) { flash($e->getMessage(), true); }
        back(($_POST['return_tab'] ?? '') === 'devices' ? 'devices' : 'techs');
    case 'shared_delete':
        $id = (int)($_POST['id'] ?? 0);
        slip_shared_update(fn(array $list) => array_filter($list, fn($x) => ($x['id'] ?? 0) !== $id));
        flash('Removed from the shared list.');
        back('techs');

}

// Make sure the relay is up whenever an admin looks.
slip_write_htaccess();
$relayUp = slip_relay_ensure();
$station = slip_station();
$tab = (string)($_GET['tab'] ?? 'status');
$selectedDevice = null;
if ($tab === 'devices' && isset($_GET['device'])) {
    $requestedId = filter_var($_GET['device'], FILTER_VALIDATE_INT);
    if (slip_access_key() !== null) foreach (slip_store()->all() as $device) {
        if ((int)$device['id'] === $requestedId) { $selectedDevice = $device; break; }
    }
    if (!$selectedDevice) http_response_code(404);
}

slip_page_start('Station admin', '../', 'admin-console');
?>
<nav class="tabs">
  <a href="./" class="<?= $tab === 'status' ? 'on' : '' ?>">Status</a>
  <a href="?tab=devices" class="<?= $tab === 'devices' ? 'on' : '' ?>">Devices</a>
  <a href="?tab=groups" class="<?= $tab === 'groups' ? 'on' : '' ?>">Groups</a>
  <a href="?tab=techs" class="<?= $tab === 'techs' ? 'on' : '' ?>">Technicians</a>
  <a href="?tab=settings" class="<?= $tab === 'settings' ? 'on' : '' ?>">Settings</a>
  <a href="../" target="_blank">Download page ↗</a>
  <span class="signed-in dim">Signed in as <b><?= h($admin['username']) ?></b></span>
  <form method="post"><?= $form() ?><input type="hidden" name="action" value="logout"><button class="small lav">Sign out</button></form>
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
  <p class="dim">Connection records show the device when known, peer address, duration, transport byte counts and observed close reason. Client stream endings are INFO; rejected connections and transport failures are WARN. Process events identify starts, stops and restart requests. Timestamps are UTC.</p>
  <?php slip_relay_report($log) ?>
  <?php endif ?>

<?php elseif ($tab === 'devices'):
    $devices = slip_store()->all();
    $sharedById = array_column(slip_shared(), null, 'id');
    $technicians = slip_users();
    $private = slip_access_key() !== null;
    $stationGroups = slip_shared_groups();
    $groupOptions = array_map(fn($g) => ['value' => $g, 'label' => $g], $stationGroups);
    $technicianOptions = [];
    foreach ($technicians as $username => $technician) $technicianOptions[] = ['value' => (string)$username, 'label' => $technician['name'] ?? $username, 'detail' => (string)$username];
    usort($devices, fn($a, $b) => ($b['last_seen'] ?? 0) <=> ($a['last_seen'] ?? 0));
    $cut = time() - SLIP_ONLINE_WINDOW;
    $ago = function ($t) {
        if (!$t) return 'signed off';
        $d = time() - $t;
        return $d < 90 ? 'just now' : ($d < 5400 ? round($d / 60) . ' min ago' : ($d < 172800 ? round($d / 3600) . ' h ago' : round($d / 86400) . ' days ago'));
    };
    ?>
  <h2>Devices</h2>
  <?php if ($selectedDevice):
      $inventory = slip_device_inventory($selectedDevice['endpoint']);
      $saved = $sharedById[$selectedDevice['id']] ?? [];
      $name = $saved['name'] ?? $selectedDevice['name'] ?? 'Workstation';
  ?>
  <p><a href="?tab=devices">← All devices</a></p>
  <div class="panel device-overview">
    <h3><?= h($name) ?> <span class="mono dim"><?= h($selectedDevice['id']) ?></span></h3>
    <p class="dim">Last reported information, available even while the workstation is offline.
      Updated <?= !empty($inventory['report_at']) ? h(gmdate('Y-m-d H:i', $inventory['report_at'])) . ' UTC' : 'when the workstation next reports' ?>.
      Updated clients refresh this report every 15 minutes while connected to a private station.</p>
    <dl class="inventory-summary">
    <?php foreach (['computer_name' => 'Computer', 'model' => 'Model', 'serial_number' => 'System serial', 'motherboard' => 'Motherboard', 'motherboard_serial' => 'Motherboard serial', 'os_version' => 'Operating system', 'processor' => 'Processor'] as $key => $label): ?>
      <dt><?= h($label) ?></dt><dd><?= h($inventory[$key] ?? 'Unavailable (not reported or access denied)') ?></dd>
    <?php endforeach ?>
    </dl>
  </div>
  <?php if (empty($inventory['report'])): ?>
    <div class="panel"><p>The full System information report will appear after this workstation runs an updated Slipstream client and reports to this station.</p></div>
  <?php else: ?>
    <div class="report-sections">
    <?php foreach ($inventory['report'] as $section): ?>
      <details class="panel system-report" open><summary><?= h($section['title']) ?></summary>
        <table><tbody><?php foreach ($section['rows'] as [$label, $value]): ?>
          <tr><th scope="row"><?= h($label) ?></th><td><?= h($value) ?></td></tr>
        <?php endforeach ?></tbody></table>
      </details>
    <?php endforeach ?>
    </div>
  <?php endif ?>
  <?php else: ?>
  <div class="panel device-register">
    <?php if (isset($_GET['device'])): ?><p class="bad">Device report unavailable.</p><?php endif ?>
    <p class="dim"><?= $private ? 'Workstations automatically enroll in the station under Ungrouped. Assign their group to technicians to show them in those apps. Names and groups you set stay as you left them. Connecting still requires the device password.' : 'Public directory: automatic shared enrollment and hardware inventory are disabled.' ?> <a href="?tab=groups">Organize groups and assign them to technicians</a></p>

    <?php if (!$devices): ?><p class="dim">No devices yet. They appear here once someone runs the app from your download page.</p><?php else: ?>
    <div class="device-list-toolbar" data-device-controls hidden>
      <label class="search-control"><span class="k">Find a device</span><input type="search" data-device-search placeholder="Name, group, ID or serial…" autocomplete="off"></label>
      <div class="device-sort-control"><span class="k" id="device-sort-label">Sort by</span>
        <details class="device-sort-menu" data-device-sort data-value="recent">
          <summary aria-labelledby="device-sort-label device-sort-current" aria-haspopup="menu" aria-controls="device-sort-options" aria-expanded="false"><span id="device-sort-current" data-device-sort-label>Last seen</span><span class="picker-chevron" aria-hidden="true"></span></summary>
          <div class="device-sort-options" id="device-sort-options" role="menu" aria-labelledby="device-sort-label">
            <?php foreach (['recent' => 'Last seen', 'group' => 'Group name', 'name' => 'Device name', 'id' => 'Device ID', 'system' => 'System'] + ($private ? ['hardware' => 'Hardware'] : []) as $key => $label): ?>
              <button type="button" role="menuitemradio" aria-checked="<?= $key === 'recent' ? 'true' : 'false' ?>" tabindex="<?= $key === 'recent' ? '0' : '-1' ?>" data-device-sort-option="<?= h($key) ?>"><span data-sort-option-label><?= h($label) ?></span><span class="sort-choice-mark" aria-hidden="true">✓</span></button>
            <?php endforeach ?>
          </div>
        </details>
      </div>
      <button type="button" class="small blue" data-device-direction>Newest first</button>
      <button type="button" class="small lav" data-device-clear disabled>Clear search</button>
      <p class="dim" data-device-count role="status" aria-live="polite"><?= count($devices) ?> devices</p>
    </div>
    <table class="devices-table">
      <thead><tr>
        <th scope="col" aria-label="Status" class="device-status-heading"></th>
        <th scope="col" data-device-sort-column="id"><button type="button" data-device-column-sort="id" disabled>ID <span data-sort-arrow aria-hidden="true">↕</span></button></th>
        <th scope="col" data-device-sort-column="name"><button type="button" data-device-column-sort="name" disabled>Name <span data-sort-arrow aria-hidden="true">↕</span></button></th>
        <th scope="col" data-device-sort-column="group"><button type="button" data-device-column-sort="group" disabled>Group <span data-sort-arrow aria-hidden="true">↕</span></button></th>
        <th scope="col" data-device-sort-column="system"><button type="button" data-device-column-sort="system" disabled>System <span data-sort-arrow aria-hidden="true">↕</span></button></th>
        <?php if ($private): ?><th scope="col" data-device-sort-column="hardware"><button type="button" data-device-column-sort="hardware" disabled>Hardware <span data-sort-arrow aria-hidden="true">↕</span></button></th><?php endif ?>
        <th scope="col" data-device-sort-column="recent" aria-sort="descending"><button type="button" data-device-column-sort="recent" disabled>Last seen <span data-sort-arrow aria-hidden="true">↓</span></button></th>
        <th scope="col" class="device-actions-heading">Actions</th>
      </tr></thead>
      <tbody data-device-rows>
      <?php foreach ($devices as $d):
          $on = ($d['last_seen'] ?? 0) >= $cut; $id = (string)$d['id'];
          $saved = $sharedById[$d['id']] ?? null;
          $inventory = $private ? slip_device_inventory($d['endpoint']) : [];
          $deviceName = $saved['name'] ?? $d['name'] ?? '';
          $deviceGroups = slip_device_groups($saved ?? []) ?: ['Ungrouped'];
          $searchText = implode(' ', array_merge([$id, $deviceName, $d['name'] ?? ''], $deviceGroups,
              array_values(array_intersect_key($d, array_flip(['os', 'app_version']))),
              array_values(array_filter(array_intersect_key($inventory, array_flip(['os_version', 'architecture', 'model', 'processor', 'serial_number', 'motherboard', 'motherboard_serial'])), 'is_scalar'))));
      ?>
        <tr data-device-row data-device-id="<?= h($id) ?>" data-device-seen="<?= (int)($d['last_seen'] ?? 0) ?>" data-device-search-text="<?= h($searchText) ?>">
          <td class="<?= $on ? 'ok' : 'dim' ?>">●</td>
          <td class="mono device-id"><?php if ($private): ?><a href="?tab=devices&amp;device=<?= (int)$d['id'] ?>" title="View full system report"><?php endif ?><?= h(substr($id, 0, 3) . ' ' . substr($id, 3, 3) . ' ' . substr($id, 6)) ?><?php if ($private): ?></a><?php endif ?></td>
          <td data-label="Name"><b data-device-name><?= h($deviceName) ?></b>
            <?php if ($saved && ($saved['name'] ?? '') !== ($d['name'] ?? '')): ?><div class="dim">Computer: <?= h($d['name'] ?? '') ?></div><?php endif ?>
          </td>
          <td data-label="Group"><div class="group-chips" data-device-groups><?php foreach ($deviceGroups as $group): ?><span class="group-chip"><?= h($group) ?></span><?php endforeach ?></div></td>
          <td data-label="System" data-device-system="<?= h($inventory['os_version'] ?? $d['os'] ?? '') ?>"><?= h($inventory['os_version'] ?? $d['os'] ?? '') ?><?php if (!empty($inventory['architecture'])): ?><div class="dim"><?= h($inventory['architecture']) ?></div><?php endif ?><div class="dim">Slipstream <?= h($d['app_version'] ?? '') ?></div></td>
          <?php if ($private): ?><td data-label="Hardware" data-device-hardware="<?= h($inventory['model'] ?? '') ?>">
            <?php if ($inventory): ?>
              <div><?= h($inventory['model'] ?? 'Model unavailable') ?></div>
              <?php if (!empty($inventory['processor'])): ?><div class="dim"><?= h($inventory['processor']) ?></div><?php endif ?>
              <?php if (!empty($inventory['memory_bytes'])): ?><div><?= h(round($inventory['memory_bytes'] / 1073741824, 1)) ?> GiB RAM</div><?php endif ?>
              <div class="mono"><?= !empty($inventory['serial_number']) ? 'Serial: ' . h($inventory['serial_number']) : 'Motherboard serial: ' . h($inventory['motherboard_serial'] ?? 'Unavailable') ?></div>
              <a href="?tab=devices&amp;device=<?= (int)$d['id'] ?>">View full system report →</a>
              <div class="dim">Reported <?= h(gmdate('Y-m-d H:i', $inventory['reported_at'])) ?> UTC</div>
            <?php else: ?><span class="dim">Awaiting host inventory</span><?php endif ?>
          </td><?php endif ?>
          <td data-label="Last seen" class="<?= $on ? 'ok' : 'dim' ?>"><?= $on ? 'online' : h($ago($d['last_seen'] ?? 0)) ?></td>
          <td class="device-actions">
            <div class="device-action-stack">
            <div class="device-management-actions">
            <?php if ($private): ?>
              <button type="button" class="small lav" data-dialog-open="edit-device-<?= (int)$d['id'] ?>">Edit workstation</button>
              <dialog id="edit-device-<?= (int)$d['id'] ?>" class="device-dialog">
                <div class="dialog-heading"><h3>Edit workstation</h3><button type="button" class="small lav" data-dialog-close aria-label="Close editor">Close</button></div>
                <p class="dim mono"><?= h($id) ?></p>
                <form method="post"><?= $form() ?><input type="hidden" name="action" value="shared_add"><input type="hidden" name="return_tab" value="devices"><input type="hidden" name="id" value="<?= (int)$d['id'] ?>"><input type="hidden" name="groups_present" value="1">
                  <div class="device-form-grid">
                    <label><span class="k">Name</span><input type="text" name="name" maxlength="64" required value="<?= h($saved['name'] ?? $d['name'] ?? '') ?>"></label>
                    <?php slip_picker('groups', 'Groups', $groupOptions, slip_device_groups($saved ?? []), ['create' => true, 'placeholder' => 'Ungrouped']) ?>
                    <div class="form-wide"><label class="k" for="notes-<?= (int)$d['id'] ?>">Notes</label><?php slip_notes($saved['notes'] ?? '', 'notes-' . (int)$d['id']) ?></div>
                  </div>
                  <div class="dialog-actions"><button class="small blue">Save workstation</button><button type="button" class="small lav" data-dialog-close>Cancel</button></div>
                </form>
              </dialog>
            <?php endif ?>
            <form class="device-remove" method="post" onsubmit="return confirm('Remove this device?')"><?= $form() ?>
              <input type="hidden" name="action" value="delete_device"><input type="hidden" name="id" value="<?= (int)$d['id'] ?>">
              <button class="small red">Remove</button>
            </form>
            </div>
            <?php if ($technicians): ?>
              <form method="post" class="device-assign-form"><?= $form() ?><input type="hidden" name="action" value="device_assign"><input type="hidden" name="id" value="<?= (int)$d['id'] ?>">
                <?php slip_picker('usernames', 'Assign to technicians', $technicianOptions, [], ['placeholder' => 'Choose technicians']) ?>
                <button class="small blue">Assign</button>
              </form>
            <?php else: ?><a href="?tab=techs">Add a technician first</a><?php endif ?>
            </div>
          </td>
        </tr>
      <?php endforeach ?>
      </tbody>
    </table>
    <p class="dim" data-device-empty hidden>No devices match this search.</p>
    <?php endif ?>
  </div>

  <?php endif ?>

<?php elseif ($tab === 'groups'):
    $state = slip_shared_state(); $groups = slip_shared_groups($state); $shared = $state['devices']; $technicians = slip_users();
    $catalog = slip_group_catalog($state, $technicians);
    $technicianLabel = fn($u) => ($technicians[$u]['name'] ?? $u) . ' (' . $u . ')';
    ?>
  <h2>Groups &amp; technician access</h2>
  <div class="panel group-toolbar">
    <p>A workstation can belong to several groups. Choose the technicians who should see a group and press <b>Done</b> in that picker to save without leaving the page. Personal passwords stay private.</p>
    <form method="post" class="inline-form"><?= $form() ?><input type="hidden" name="action" value="station_group_create"><label><span class="k">Create station group</span><input type="text" name="group" maxlength="64" required placeholder="New group name"></label><button class="small blue">Create group</button></form>
  </div>
  <div class="panel">
    <div class="group-list-toolbar"><label class="search-control"><span class="k">Find a group</span><input type="search" data-group-search placeholder="Group or source…"></label><p class="dim">Station groups are hidden until assigned. Personal groups always remain visible to their owner.</p></div>
    <?php if (!$catalog): ?><p class="dim">No groups yet.</p><?php else: ?>
    <div class="table-scroll"><table class="group-access">
      <tr><th>Group</th><th>Source</th><th>Workstations</th><th>Visible to</th><th>Manage</th></tr>
      <?php foreach ($catalog as $entry):
          $owner = $entry['owner']; $group = $entry['group']; $count = count($entry['devices']);
          $recipients = array_values(array_filter($entry['usernames'], fn($u) => isset($technicians[$u])));
          $choices = $owner === '' ? array_column($shared, null, 'id') : array_column($technicians[$owner]['devices'] ?? [], null, 'id') + array_column($shared, null, 'id');
          $workstationOptions = [];
          foreach ($choices as $device) $workstationOptions[] = ['value' => (string)$device['id'], 'label' => ($device['name'] ?? '') ?: (string)$device['id'], 'detail' => (string)$device['id'], 'disabled' => $group === ''];
          $userOptions = [];
          foreach ($technicians as $username => $technician) $userOptions[] = ['value' => (string)$username, 'label' => $technician['name'] ?? $username, 'detail' => (string)$username . ((string)$username === $owner ? ' · owner' : ''), 'disabled' => (string)$username === $owner];
          ?>
      <tr data-group-row data-group-search-text="<?= h(($group === '' ? 'Ungrouped' : $group) . ' ' . ($owner === '' ? 'Station' : $technicianLabel($owner))) ?>">
        <td data-label="Group"><b><?= h($group === '' ? 'Ungrouped' : $group) ?></b><?php if ($group === ''): ?><div class="dim">Automatic: no named groups</div><?php endif ?></td>
        <td data-label="Source"><?= $owner === '' ? 'Station' : h($technicianLabel($owner)) ?><div class="dim"><?= $owner === '' ? 'Managed by station admins' : 'Personal technician group' ?></div></td>
        <td data-label="Workstations">
          <?php if ($group !== ''): ?><form method="post"><?= $form() ?><input type="hidden" name="action" value="group_members"><input type="hidden" name="owner" value="<?= h($owner) ?>"><input type="hidden" name="group" value="<?= h($group) ?>"><?php endif ?>
            <?php slip_picker('ids', 'Workstations', $workstationOptions, array_column($entry['devices'], 'id'), ['placeholder' => 'Choose workstations']) ?>
            <?php if ($group !== ''): ?><button class="small blue">Save workstations</button></form><?php endif ?>
        </td>
        <td data-label="Visible to">
          <form method="post" data-picker-save><?= $form() ?><input type="hidden" name="action" value="group_visibility"><input type="hidden" name="owner" value="<?= h($owner) ?>"><input type="hidden" name="group" value="<?= h($group) ?>">
            <?php slip_picker('usernames', 'Visible to these technicians', $userOptions, array_merge($recipients, $owner !== '' ? [$owner] : []), ['placeholder' => $owner === '' ? 'No technicians assigned' : 'Owner only']) ?>
            <p class="picker-save-status dim" data-picker-save-status role="status" aria-live="polite" hidden></p>
            <noscript><button class="small blue">Save visibility</button></noscript>
          </form>
        </td>
        <td data-label="Manage">
        <?php if ($owner === '' && $group !== ''): ?>
          <details class="group-rename"><summary>Rename</summary><form method="post"><?= $form() ?><input type="hidden" name="action" value="station_group_rename"><input type="hidden" name="group" value="<?= h($group) ?>">
            <label class="k">New name</label><input type="text" name="new_name" maxlength="64" required value="<?= h($group) ?>"><button class="small blue">Rename group</button>
          </form></details>
          <?php if (!$count): ?><form method="post"><?= $form() ?><input type="hidden" name="action" value="station_group_delete"><input type="hidden" name="group" value="<?= h($group) ?>"><button class="small red">Remove empty group</button></form><?php endif ?>
        <?php endif ?>
        </td>
      </tr>
      <?php endforeach ?>
    </table></div><?php endif ?>
    <p class="dim" data-group-empty hidden>No matching groups.</p>
  </div>

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
    organisation's station. Accounts with <b>Station admin</b> permission also sign in to this web console
    with the same username and password. Devices they save a password for keep it encrypted with a key only their app can
    make from their password: this station can't read it.</p>
    <?php if (!$users): ?><p class="dim">No technicians yet.</p><?php else: ?>
    <div class="table-scroll"><table class="technician-table">
      <tr><th>Username</th><th>Name</th><th>Role</th><th>Personal / assigned groups</th><th>Devices</th><th>Signed in</th><th></th></tr>
      <?php foreach ($users as $u => $a): $sessions = (array)($a['sessions'] ?? []); $last = $sessions ? max(array_column($sessions, 'used')) : 0; ?>
        <tr>
          <td data-label="Username" class="mono"><?= h($u) ?></td>
          <td data-label="Name"><?= h($a['name'] ?? '') ?></td>
          <td data-label="Role"><?= !empty($a['admin']) ? '<span class="ok">admin</span>' : 'technician' ?></td>
          <td data-label="Groups"><?php foreach ($catalog as $entry): if ($entry['owner'] !== (string)$u && !in_array((string)$u, $entry['usernames'], true)) continue; ?>
            <div><?= h($entry['group'] === '' ? 'Ungrouped' : $entry['group']) ?> <span class="dim">· <?= $entry['owner'] === (string)$u ? 'personal' : ($entry['owner'] === '' ? 'station' : h($entry['owner'])) ?></span></div>
          <?php endforeach ?><a href="?tab=groups">Manage group assignments</a></td>
          <td data-label="Devices"><?= count($a['devices'] ?? []) ?></td>
          <td data-label="Signed in" class="dim"><?= count($sessions) ?> app<?= count($sessions) === 1 ? '' : 's' ?><?= $last ? ' · ' . h($when($last)) : '' ?></td>
          <td class="technician-actions">
            <form method="post" style="display:inline"><?= $form() ?><input type="hidden" name="action" value="tech_admin">
              <input type="hidden" name="username" value="<?= h($u) ?>"><input type="hidden" name="admin" value="<?= empty($a['admin']) ? '1' : '' ?>">
              <button class="small lav"><?= empty($a['admin']) ? 'Make admin' : 'Remove admin' ?></button></form>
            <form method="post" style="display:inline"><?= $form() ?><input type="hidden" name="action" value="tech_signout">
              <input type="hidden" name="username" value="<?= h($u) ?>"><button class="small blue">Sign out everywhere</button></form>
            <form method="post" style="display:inline" onsubmit="return confirm('Remove <?= h($u) ?> and their saved devices?')"><?= $form() ?>
              <input type="hidden" name="action" value="tech_delete"><input type="hidden" name="username" value="<?= h($u) ?>">
              <button class="small red">Remove</button></form>
            <form method="post" class="password-reset"><?= $form() ?><input type="hidden" name="action" value="tech_password">
              <input type="hidden" name="username" value="<?= h($u) ?>">
              <input type="password" name="password" minlength="10" required placeholder="New password" aria-label="New password for <?= h($u) ?>" autocomplete="new-password">
              <button class="small">Set password</button></form>
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
      <label><input type="checkbox" name="admin" value="1"> Station admin: may sign in to this web console, manage technicians and settings, and change shared devices</label>
      <br><button>Add technician</button>
    </form>
  </div>

  <h2 class="gold">Station devices</h2>
  <div class="panel">
    <p>These are the station’s workstations. Visibility in technician apps is controlled by each group’s assignments on the <a href="?tab=groups">Groups tab</a>. Passwords aren't stored for shared devices.</p>
    <?php if ($shared): ?>
    <table class="shared-device-table">
      <tr><th>ID</th><th>Name</th><th>Group</th><th>Notes</th><th>Added by</th><th></th></tr>
      <?php foreach ($shared as $d): $id = (string)$d['id']; ?>
        <tr><td class="mono device-id"><?= h(substr($id, 0, 3) . ' ' . substr($id, 3, 3) . ' ' . substr($id, 6)) ?></td>
          <td data-label="Name"><?= h($d['name'] ?? '') ?></td><td data-label="Group"><?= h(implode(' · ', slip_device_groups($d)) ?: 'Ungrouped') ?></td><td data-label="Notes" class="dim"><?= h($d['notes'] ?? '') ?></td><td data-label="Added by" class="dim"><?= h($d['added_by'] ?? '') ?></td>
          <td><form method="post"><?= $form() ?><input type="hidden" name="action" value="shared_delete"><input type="hidden" name="id" value="<?= (int)$d['id'] ?>">
            <button class="small red">Remove</button></form></td></tr>
      <?php endforeach ?>
    </table>
    <?php endif ?>
    <form method="post"><?= $form() ?><input type="hidden" name="action" value="shared_add">
      <label class="k">Device ID</label><input type="text" name="id" required placeholder="482 913 077">
      <label class="k">Name</label><input type="text" name="name" placeholder="Front desk PC">
      <input type="hidden" name="groups_present" value="1">
      <?php slip_picker('groups', 'Groups', array_map(fn($g) => ['value' => $g, 'label' => $g], slip_shared_groups()), [], ['create' => true, 'placeholder' => 'Ungrouped']) ?>
      <label class="k">Notes (optional)</label><?php slip_notes('', 'new-device-notes') ?>
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

  <h2 class="violet">Administrator accounts</h2>
  <div class="panel"><p>Each administrator signs in with their own technician username and password.
    Manage permissions and reset passwords on the <a href="?tab=techs">Technicians page</a>.</p></div>
<?php endif ?>
<?php $scriptVersion = (string)(filemtime(SLIP_ROOT . '/assets/admin.js') ?: 0); ?>
<script src="../assets/admin.js?v=<?= h($scriptVersion) ?>" defer></script>
<?php slip_page_end();
