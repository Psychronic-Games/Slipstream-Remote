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
$guess = stripos($ua, 'Windows') !== false ? 'windows' : (stripos($ua, 'Mac OS') !== false ? 'macos' : (stripos($ua, 'Linux') !== false && stripos($ua, 'Android') === false ? 'linux' : null));
$fmt = fn($b) => round($b / 1048576) . ' MB';

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
          <a class="btn <?= $os === $guess ? '' : 'blue' ?>" href="download.php?os=<?= h($os) ?>">Download</a>
        <?php else: ?>
          <span class="btn off">Not available</span>
        <?php endif ?>
      </div>
    <?php endforeach ?>
  </div>

  <h2 class="violet">Getting started</h2>
  <div class="panel">
    <ol class="steps">
      <li>Download the app for your computer above.</li>
      <li><b>Windows:</b> open the downloaded file (if SmartScreen appears, choose <i>More info → Run anyway</i>).
        <b>Mac:</b> unzip, move <i>Slipstream</i> to Applications, then right-click → <i>Open</i>.
        <b>Linux:</b> make it executable (<code>chmod +x slipstream-*</code>) and run it.</li>
      <li>The app opens already connected to <?= h($org) ?>. Read the <b>ID</b> and <b>password</b> to your technician.</li>
      <li>Close the app any time to end access. A new password is made after every session.</li>
    </ol>
  </div>
<?php endif ?>
<?php slip_page_end();
