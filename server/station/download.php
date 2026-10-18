<?php
/**
 * Serves a client build for this station. Unsigned builds get the station's
 * address, access key and name appended, so they work with no setup; signed
 * ones are served untouched and link up with a setup code instead.
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

// "Slipstream Remote - Acme IT". Characters that are illegal in file names
// on any OS (Windows forbids <>:"/\|?*) are dropped from the org name.
$org = preg_replace('/[\x00-\x1F<>:"\/\\\\|?*]+/u', ' ', (string)($station['org'] ?? ''));
$org = trim(preg_replace('/\s+/u', ' ', $org), " .");
$base = 'Slipstream Remote' . ($org !== '' && strcasecmp($org, 'Slipstream Remote') !== 0 ? " - $org" : '');

function slip_disposition(string $name): string
{
    // Plain ASCII name for every browser, plus the exact UTF-8 name (RFC 6266)
    // for org names with accents etc.
    $ascii = preg_replace('/[^\x20-\x7E]/', '_', $name);
    return 'Content-Disposition: attachment; filename="' . $ascii . "\"; filename*=UTF-8''" . rawurlencode($name);
}

while (ob_get_level()) {
    ob_end_clean();
}
header('Cache-Control: no-store');
header('X-Content-Type-Options: nosniff');

// Signed apps (and the Mac bundle and Android's APK, whose seal or
// signature any added byte breaks) go out byte-for-byte as released. They
// link to this station with the setup code that the download URL carries
// (see index.php). Unsigned builds get the station's settings appended, so
// they need no setup at all.
if ($b['signed'] || $os === 'macos') {
    $name = ['windows' => "$base.exe", 'macos' => "$base (macOS).zip", 'android' => "$base.apk"][$os] ?? $base;
    $type = ['macos' => 'application/zip', 'android' => 'application/vnd.android.package-archive'][$os] ?? 'application/octet-stream';
    header("Content-Type: $type");
    header(slip_disposition($name));
    header('Content-Length: ' . $b['size']);
    readfile($b['path']);
    exit;
}

$trailer = slip_trailer();
$name = $os === 'windows' ? "$base.exe" : $base;
header('Content-Type: application/octet-stream');
header(slip_disposition($name));
header('Content-Length: ' . ($b['size'] + strlen($trailer)));
readfile($b['path']);
echo $trailer;
