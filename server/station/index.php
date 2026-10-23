<?php
/**
 * Slipstream Station — public download page.
 */
require __DIR__ . '/lib/slipstream.php';
header('Cache-Control: no-store');

if (!slip_station_ready()) {
    header('Location: admin/');
    exit;
}

$station = slip_station();
$org = ($station['org'] ?? '') !== '' ? $station['org'] : 'Slipstream Remote';
$builds = slip_client_builds();
$any = array_filter($builds, fn($b) => $b['path']);

// Optional download password.
slip_session_start('slipstream_dl');
$needPassword = !empty($station['download_password_hash']) && empty($_SESSION['dl_ok']);
$error = null;
if ($needPassword && ($_SERVER['REQUEST_METHOD'] ?? '') === 'POST') {
    if (!slip_rate_ok('dlpass', 10)) {
        $error = 'Too many attempts — wait a minute.';
    } elseif (password_verify((string)($_POST['password'] ?? ''), $station['download_password_hash'])) {
        $_SESSION['dl_ok'] = true;
        $needPassword = false;
    } else {
        $error = 'That password is not right.';
    }
}

// Guess the visitor's OS to highlight the right download.
$ua = $_SERVER['HTTP_USER_AGENT'] ?? '';
$guess = match (true) {
    stripos($ua, 'Android') !== false => 'android',
    stripos($ua, 'Windows') !== false => 'windows',
    stripos($ua, 'Mac OS') !== false => 'macos',
    stripos($ua, 'Linux') !== false => 'linux',
    default => null,
};
$fmt = fn($b) => round($b / 1048576) . ' MB';
// Today's setup code. Download links carry it too: browsers record the
// download URL on the file, and the app reads it from there.
$code = slip_link_code();

slip_page_start($org . ' · Remote Support');
?>
<p class="lead">Remote support from <strong><?= h($org) ?></strong>. Download the app below, open it,
and read your <b>ID</b> and <b>password</b> to your technician. They can only connect while the app is open
and you've shared your password.</p>

<?php if (!empty($station['motd'])): ?><p class="dim"><?= h($station['motd']) ?></p><?php endif ?>

<?php if ($needPassword): ?>
  <h2 class="lav">Enter download password</h2>
  <div class="panel">
    <?php if ($error): ?><div class="flash err"><?= h($error) ?></div><?php endif ?>
    <form method="post"><input type="password" name="password" autofocus placeholder="password from your technician">
      <br><button>Unlock downloads</button></form>
  </div>
<?php elseif (!$any): ?>
  <h2 class="red">Downloads coming soon</h2>
  <div class="panel">No app builds have been added to this station yet.</div>
<?php else: ?>
  <h2>Download</h2>
  <div class="grid">
    <?php foreach ($builds as $os => $b): ?>
      <div class="card <?= $os === $guess ? 'primary' : '' ?>">
        <h3><?= h($b['label']) ?></h3>
        <div class="dim"><?= h($b['note']) ?><?= $b['path'] ? ' · ' . $fmt($b['size']) : '' ?></div>
        <?php if ($b['path']): ?>
          <a class="btn <?= $os === $guess ? '' : 'blue' ?>" href="download.php?os=<?= h($os) ?>&amp;c=<?= h($code) ?>" download="<?= h(slip_download_name($os)) ?>">Download</a>
        <?php else: ?>
          <span class="btn off">Not available</span>
        <?php endif ?>
      </div>
    <?php endforeach ?>
  </div>

  <h2 class="violet">Getting started</h2>
  <div class="panel">
    <ol class="steps">
      <li>Download the app for your computer or phone above.</li>
      <li><b>Windows:</b> open the downloaded file (if SmartScreen appears, choose <i>More info → Run anyway</i>).
        <b>Mac:</b> open the zip and move <i>Slipstream</i> to Applications, then open it. If macOS says it can't
        check the app, go to <i>System Settings → Privacy &amp; Security</i> and choose <i>Open Anyway</i>; allow
        <i>Screen Recording</i> and <i>Accessibility</i> when asked.
        <b>Linux:</b> make it executable (right-click → Properties → <i>Allow executing</i>, or <code>chmod +x "Slipstream Remote"*</code>) and run it.
        <b>Android:</b> open the downloaded file. The first time, Android asks to let your browser install apps:
        choose <i>Settings</i>, turn on <i>Allow from this source</i>, go back and choose <i>Install</i>.
        Then come back to this page for the next step.</li>
      <li>The app connects to <?= h($org) ?> by itself (on a Mac, press the button below once). If it shows only
        <i>Slipstream Public</i> or asks for your station instead, press
        <a class="btn small" href="<?= h(slip_link_url()) ?>">Connect this app</a>
        or choose <i>Link to my organisation</i> on its Bridge page and enter <b class="mono"><?= h(slip_server_address()) ?></b> and the setup code
        <b class="mono"><?= h(slip_code_display($code)) ?></b>.</li>
      <li>Read the <b>ID</b> and <b>password</b> in the app to your technician.</li>
      <li>Close the app any time to end access. A new password is made after every session.</li>
    </ol>
  </div>
<?php endif ?>
<?php slip_page_end();
