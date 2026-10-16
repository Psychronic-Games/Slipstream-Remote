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
        slip_store()->delete((int)($_POST['id'] ?? 0));
        flash('Device removed. It will get a new ID if it connects again.');
        back('devices');
    case 'settings':
        $changes = [
            'org' => slip_clip(trim((string)($_POST['org'] ?? '')), 80),
            'motd' => slip_clip(trim((string)($_POST['motd'] ?? '')), 300),
            'private' => !empty($_POST['private']),
            'public_relays' => !empty($_POST['public_relays']),
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
    case 'rotate_key':
        slip_station_save(['access_key' => bin2hex(random_bytes(16))]);
        slip_relay_ensure(true);
        flash('New access key created. Existing apps stop working until they are downloaded again.', true);
        back('settings');
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
    @file_get_contents(slip_base_url() . 'ping', false, $ctx);
    $proxied = isset($http_response_header[0]) && strpos($http_response_header[0], ' 200') !== false;
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

  <h2 class="gold">Client downloads</h2>
  <div class="panel"><table>
    <?php foreach ($builds as $os => $b): ?>
      <tr><td><?= h($b['label']) ?></td><td class="mono"><?= h($b['file']) ?></td>
        <td class="<?= $b['path'] ? 'ok' : 'warn' ?>"><?= $b['path'] ? round($b['size'] / 1048576) . ' MB, ' . date('Y-m-d', filemtime($b['path'])) : 'missing — upload to clients/' ?></td></tr>
    <?php endforeach ?>
  </table></div>

  <?php $log = @file_get_contents(slip_data_dir() . '/relay.log'); if ($log): ?>
  <h2 class="violet">Relay log (latest)</h2>
  <pre><?= h(substr($log, -3000)) ?></pre>
  <?php endif ?>

<?php elseif ($tab === 'devices'):
    $devices = slip_store()->all();
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
    <?php if (!$devices): ?><p class="dim">No devices yet. They appear here once someone runs the app from your download page.</p><?php else: ?>
    <table>
      <tr><th></th><th>ID</th><th>Name</th><th>System</th><th>Version</th><th>Last seen</th><th></th></tr>
      <?php foreach ($devices as $d): $on = ($d['last_seen'] ?? 0) >= $cut; $id = (string)$d['id']; ?>
        <tr>
          <td class="<?= $on ? 'ok' : 'dim' ?>">●</td>
          <td class="mono"><?= h(substr($id, 0, 3) . ' ' . substr($id, 3, 3) . ' ' . substr($id, 6)) ?></td>
          <td><?= h($d['name'] ?? '') ?></td>
          <td><?= h($d['os'] ?? '') ?></td>
          <td class="dim"><?= h($d['app_version'] ?? '') ?></td>
          <td class="<?= $on ? 'ok' : 'dim' ?>"><?= $on ? 'online' : h($ago($d['last_seen'] ?? 0)) ?></td>
          <td><form method="post" onsubmit="return confirm('Remove this device?')"><?= $form() ?>
            <input type="hidden" name="action" value="delete_device"><input type="hidden" name="id" value="<?= (int)$d['id'] ?>">
            <button class="small red">Remove</button></form></td>
        </tr>
      <?php endforeach ?>
    </table>
    <?php endif ?>
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
        Let apps fall back to the free public relays if this station's relay is unreachable</label>
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
