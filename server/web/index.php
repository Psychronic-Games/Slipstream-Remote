<?php
/**
 * Slipstream Remote directory — status and setup page.
 */
require __DIR__ . '/slipstream.php';

header('Cache-Control: no-store');
$h = fn($s) => htmlspecialchars((string)$s, ENT_QUOTES);

// Health checks.
$checks = [];
$checks[] = ['PHP ' . PHP_VERSION, version_compare(PHP_VERSION, '8.0', '>='), 'PHP 8.0 or newer is required'];
$sodium = function_exists('sodium_crypto_sign_verify_detached');
$checks[] = ['Signature verification (sodium)', $sodium, 'Enable the PHP "sodium" extension'];

$store = null;
$storeError = null;
try {
    $store = slip_store();
} catch (Throwable $e) {
    $storeError = $e->getMessage();
}
$where = $store && $store->name() === 'files' ? ' in ' . realpath(slip_data_dir()) : '';
$checks[] = ['Storage: ' . ($store ? $store->name() . $where : 'unavailable'), $store !== null, $storeError ?? ''];

$dataPrivate = null;
if ($store && $store->name() === 'files') {
    // Make sure the data folder can't be downloaded.
    $probe = slip_data_dir() . '/probe.txt';
    @file_put_contents($probe, 'x');
    $dataDir = realpath(slip_data_dir());
    $docRoot = realpath($_SERVER['DOCUMENT_ROOT'] ?? '') ?: '';
    if ($docRoot && $dataDir && strpos($dataDir, $docRoot) === 0) {
        $url = slip_scheme() . '://' . slip_host() . str_replace('\\', '/', substr($dataDir, strlen($docRoot))) . '/probe.txt';
        $ctx = stream_context_create(['http' => ['timeout' => 2, 'ignore_errors' => true], 'ssl' => ['verify_peer' => false, 'verify_peer_name' => false]]);
        $body = @file_get_contents($url, false, $ctx);
        $dataPrivate = $body !== 'x';
        @unlink($probe);
    } else {
        $dataPrivate = true; // outside the web root
        @unlink($probe);
    }
    $checks[] = ['Data folder is private', $dataPrivate, 'Your web server serves the data folder publicly. On Nginx add: location ~ /data/ { deny all; } — or set data_dir in config.php to a folder outside the web root.'];
}

[$total, $online] = $store ? $store->counts() : [0, 0];
$relays = $store ? slip_relays() : [];
$allOk = !in_array(false, array_column($checks, 1), true);
$address = preg_replace('#^https://#', '', rtrim(slip_base_url(), '/'));
$motd = slip_config()['motd'];
?><!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Slipstream Server</title>
<style>
  :root { --bg:#020308; --panel:#080b14; --orange:#ff9900; --peach:#ffaa78; --tan:#ffd6aa; --lav:#cc99cc;
          --violet:#9977dd; --blue:#99ccff; --green:#66ffaa; --red:#ff5555; --dim:#6e7896; }
  * { box-sizing: border-box; }
  body { margin:0; background:var(--bg); color:var(--tan); font:17px/1.5 "Barlow Semi Condensed", system-ui, sans-serif; }
  .frame { display:grid; grid-template-columns: 140px 1fr; grid-template-rows: 32px 1fr 26px; gap:8px; min-height:100vh; padding:12px; }
  .elbow-top { grid-column:1/3; display:flex; gap:8px; }
  .elbow-top .e { width:340px; background:var(--orange); border-radius:40px 0 0 0; }
  .elbow-top .s { flex:1; background:var(--peach); } .elbow-top .s2 { width:90px; background:var(--violet); }
  .elbow-top h1 { margin:0; color:var(--orange); font:700 30px/32px "Antonio", "Arial Narrow", sans-serif; letter-spacing:.04em; white-space:nowrap; text-transform:uppercase; }
  .side { display:flex; flex-direction:column; gap:8px; }
  .side div { background:var(--lav); color:#000; padding:28px 10px 4px; text-align:right; font:16px "Antonio","Arial Narrow",sans-serif; text-transform:uppercase; }
  .side div:nth-child(2){background:var(--blue)} .side div:nth-child(3){background:var(--orange);flex:1}
  .bottom { grid-column:1/3; display:flex; gap:8px; } .bottom .e { width:340px; background:var(--lav); border-radius:0 0 0 40px; }
  .bottom .s { flex:1; background:var(--blue); border-radius:0 13px 13px 0; }
  main { padding:8px 12px; overflow:auto; }
  h2 { font:700 20px "Antonio","Arial Narrow",sans-serif; text-transform:uppercase; letter-spacing:.05em; display:flex; align-items:center; gap:10px; margin:20px 0 10px; }
  h2::before { content:""; width:26px; height:26px; border-radius:13px 0 0 13px; background:currentColor; }
  h2::after { content:""; flex:1; height:12px; border-radius:0 6px 6px 0; background:currentColor; opacity:.85; }
  .panel { background:var(--panel); padding:14px 18px; border-radius:4px; }
  .addr { font:32px "Share Tech Mono", ui-monospace, monospace; color:var(--tan); word-break:break-all; }
  .k { color:var(--lav); font:14px "Antonio","Arial Narrow",sans-serif; text-transform:uppercase; letter-spacing:.08em; }
  .ok { color:var(--green); } .bad { color:var(--red); } .dim { color:var(--dim); }
  table { border-collapse:collapse; } td { padding:4px 14px 4px 0; vertical-align:top; }
  code, pre { font-family:"Share Tech Mono", ui-monospace, monospace; color:var(--blue); }
  pre { background:#04060c; padding:12px; border-left:4px solid var(--violet); overflow:auto; }
  .stats { display:flex; gap:40px; } .stats b { display:block; font:40px "Share Tech Mono", monospace; color:var(--orange); }
  @media (max-width:700px) { .frame { grid-template-columns:1fr; } .side { display:none; } .elbow-top .e, .bottom .e { width:80px; } }
</style>
<div class="frame">
  <div class="elbow-top"><div class="e"></div><div class="s"></div><div class="s2"></div><h1>Slipstream Server</h1></div>
  <div class="side"><div>Directory</div><div>Relay</div><div><?= $h(date('H-i')) ?></div></div>
  <main>
    <p class="<?= $allOk ? 'ok' : 'bad' ?>">● <?= $allOk ? 'ONLINE — READY FOR CONNECTIONS' : 'NEEDS ATTENTION — SEE CHECKS BELOW' ?></p>
    <?php if ($motd): ?><p class="dim"><?= $h($motd) ?></p><?php endif ?>

    <h2 style="color:var(--orange)">Connect your apps</h2>
    <div class="panel">
      <div class="k">In Slipstream: Config → Server address</div>
      <div class="addr"><?= $h($address) ?></div>
    </div>

    <h2 style="color:var(--blue)">Status</h2>
    <div class="panel stats">
      <div><span class="k">Devices known</span><b><?= (int)$total ?></b></div>
      <div><span class="k">Online now</span><b><?= (int)$online ?></b></div>
      <div><span class="k">Relay</span><b style="font-size:20px;line-height:48px"><?= $relays ? '<span class="ok">' . $h(implode(', ', $relays)) . '</span>' : '<span class="dim">public relays</span>' ?></b></div>
    </div>

    <h2 style="color:var(--lav)">Checks</h2>
    <div class="panel"><table>
      <?php foreach ($checks as [$label, $ok, $hint]): ?>
        <tr><td class="<?= $ok ? 'ok' : 'bad' ?>"><?= $ok ? '✔' : '✘' ?></td><td><?= $h($label) ?><?php if (!$ok && $hint): ?><br><span class="dim"><?= $h($hint) ?></span><?php endif ?></td></tr>
      <?php endforeach ?>
    </table></div>

    <?php if (!$relays): ?>
    <h2 style="color:var(--violet)">Optional: your own relay</h2>
    <div class="panel">
      <p>Most connections go directly between devices. When they can't, traffic goes through a relay —
      right now the free public ones. To use your own, run <code>slipstream-server --relay-only --listen 127.0.0.1:21117</code>
      on this machine and add to this site's Apache config:</p>
<pre>ProxyPass        /relay        http://127.0.0.1:21117/relay upgrade=websocket
ProxyPass        /ping         http://127.0.0.1:21117/ping
ProxyPass        /generate_204 http://127.0.0.1:21117/generate_204
ProxyPreserveHost On</pre>
      <p class="dim">This page detects it automatically within 10 minutes. See docs/SERVER.md for Nginx and Caddy.</p>
    </div>
    <?php endif ?>
  </main>
  <div class="bottom"><div class="e"></div><div class="s"></div></div>
</div>
</html>
