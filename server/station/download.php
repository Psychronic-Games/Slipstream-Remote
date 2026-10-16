<?php
/**
 * Serves a client build preconfigured for this station: the station's
 * address, access key and name are appended to the executable (or added to
 * the macOS app bundle), so the app works with no setup.
 */
require __DIR__ . '/lib/slipstream.php';

$station = slip_station();
if (!slip_station_ready()) {
    http_response_code(503);
    exit('Station not set up yet.');
}
// Apps updating themselves present the station's access key instead of
// the download page password.
$key = slip_access_key();
$appKey = (string)($_SERVER['HTTP_X_SLIPSTREAM_KEY'] ?? '');
$isApp = $key !== null && $appKey !== '' && hash_equals($key, $appKey);
if (!empty($station['download_password_hash']) && !$isApp) {
    slip_session_start('slipstream_dl');
    if (empty($_SESSION['dl_ok'])) {
        header('Location: ./');
        exit;
    }
}

$os = (string)($_GET['os'] ?? '');
$builds = slip_client_builds();
$b = $builds[$os] ?? null;
if (!$b || !$b['path']) {
    http_response_code(404);
    exit('That download is not available.');
}

$slug = trim(preg_replace('/[^A-Za-z0-9]+/', '-', $station['org'] ?? ''), '-');
$suffix = $slug !== '' ? "-$slug" : '';
while (ob_get_level()) {
    ob_end_clean();
}
header('Cache-Control: no-store');
header('X-Content-Type-Options: nosniff');

if ($os === 'macos') {
    // Add Contents/Resources/station.json inside the zipped app bundle.
    if (!class_exists('ZipArchive')) {
        http_response_code(500);
        exit('This server needs the PHP zip extension to prepare macOS downloads.');
    }
    $tmp = tempnam(sys_get_temp_dir(), 'slip');
    copy($b['path'], $tmp);
    $zip = new ZipArchive();
    if ($zip->open($tmp) !== true) {
        http_response_code(500);
        exit('Broken macOS build on the server.');
    }
    $app = null;
    for ($i = 0; $i < $zip->numFiles; $i++) {
        if (preg_match('#^(.*?\.app)/Contents/#', $zip->getNameIndex($i), $m)) {
            $app = $m[1];
            break;
        }
    }
    $zip->addFromString(($app ?? 'Slipstream.app') . '/Contents/Resources/station.json', json_encode(slip_client_config(), JSON_UNESCAPED_SLASHES));
    $zip->close();
    header('Content-Type: application/zip');
    header('Content-Disposition: attachment; filename="Slipstream' . $suffix . '-macOS.zip"');
    header('Content-Length: ' . filesize($tmp));
    readfile($tmp);
    unlink($tmp);
    exit;
}

$trailer = slip_trailer();
$name = $os === 'windows' ? "Slipstream$suffix.exe" : 'slipstream' . strtolower($suffix);
header('Content-Type: application/octet-stream');
header('Content-Disposition: attachment; filename="' . $name . '"');
header('Content-Length: ' . ($b['size'] + strlen($trailer)));
readfile($b['path']);
echo $trailer;
