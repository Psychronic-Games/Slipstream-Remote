<?php
/**
 * Slipstream Station — updates itself from the project's GitHub releases.
 *
 * Every release carries `station-update.json`: a list of the station's files
 * (PHP, relay program, apps) with their sizes and SHA-256, signed with the
 * project's release key. The station checks that signature against the keys
 * in lib/release-keys.txt before downloading anything, and checks each file
 * against the list before installing it. Someone who takes over the GitHub
 * project or the download path still can't push files to stations: they'd
 * need the release key, which never leaves the maintainer's machine.
 *
 * Installing runs in small resumable steps (one download per step), so it
 * fits ordinary PHP time limits. Files are swapped in with rename, apps
 * before clients/manifest.json, so nothing is offered before it's there.
 * Settings, devices and accounts live in the data folder and aren't touched.
 *
 * MIT License.
 */

if (realpath($_SERVER['SCRIPT_FILENAME'] ?? '') === realpath(__FILE__)) {
    http_response_code(404);
    exit;
}

const SLIP_UPDATE_REPO = 'Psychronic-Games/Slipstream-Remote';
const SLIP_UPDATE_CONTEXT = "slipstream station update v1\n";
const SLIP_UPDATE_MANIFEST = 'station-update.json';
const SLIP_UPDATE_EVERY = 6 * 3600;     // seconds between automatic checks
const SLIP_UPDATE_RETRY = 120;          // continue an interrupted install after this long

/** Hex public keys this station trusts for updates. */
function slip_release_keys(): array
{
    $keys = (string)@file_get_contents(__DIR__ . '/release-keys.txt');
    $keys .= ',' . (string)(slip_config()['release_keys'] ?? '');
    return array_values(array_unique(array_filter(
        array_map('strtolower', array_map('trim', explode(',', str_replace("\n", ',', $keys)))),
        fn($k) => (bool)preg_match('/^[0-9a-f]{64}$/', $k)
    )));
}

/** The installed station release: version, build and commit. */
function slip_installed(): array
{
    $v = json_decode((string)@file_get_contents(__DIR__ . '/version.json'), true);
    if (is_array($v) && isset($v['build'])) {
        return ['version' => (string)($v['version'] ?? '?'), 'build' => (int)$v['build'], 'commit' => (string)($v['commit'] ?? '')];
    }
    // Stations from before 0.27 only know their apps' build.
    $m = slip_client_manifest();
    return ['version' => (string)($m['version'] ?? '?'), 'build' => (int)($m['build'] ?? 0), 'commit' => (string)($m['commit'] ?? '')];
}

function slip_update_repo(): string
{
    $repo = (string)(slip_config()['update_repo'] ?? '');
    return preg_match('#^[\w.-]+/[\w.-]+$#', $repo) ? $repo : SLIP_UPDATE_REPO;
}

function slip_update_dir(): string
{
    $dir = slip_data_dir() . '/update';
    if (!is_dir($dir)) {
        @mkdir($dir, 0750, true);
    }
    return $dir;
}

/** Automatic installs default to on; the admin can switch them off. */
function slip_update_auto(): bool
{
    return (bool)(slip_station()['auto_update'] ?? true);
}

function slip_update_state(): array
{
    $s = json_decode((string)@file_get_contents(slip_update_dir() . '/state.json'), true);
    return is_array($s) ? $s : [];
}

function slip_update_save(array $state): void
{
    $path = slip_update_dir() . '/state.json';
    @file_put_contents("$path.tmp", json_encode($state, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES));
    @rename("$path.tmp", $path);
}

/**
 * GET over HTTPS. With $saveTo, streams the body into that file. Returns the
 * body (or true when saved); throws on failure. Uses the curl extension when
 * there is one, otherwise PHP's own HTTPS streams.
 */
function slip_http_get(string $url, ?string $saveTo = null, int $timeout = 20)
{
    $headers = ['User-Agent: Slipstream-Station', 'Accept: application/vnd.github+json, application/octet-stream;q=0.9, */*;q=0.8'];
    if (function_exists('curl_init')) {
        $ch = curl_init($url);
        $out = $saveTo ? fopen($saveTo, 'wb') : null;
        curl_setopt_array($ch, [
            CURLOPT_FOLLOWLOCATION => true,
            CURLOPT_MAXREDIRS => 5,
            CURLOPT_PROTOCOLS => CURLPROTO_HTTPS,
            CURLOPT_REDIR_PROTOCOLS => CURLPROTO_HTTPS,
            CURLOPT_CONNECTTIMEOUT => 10,
            CURLOPT_TIMEOUT => $timeout,
            CURLOPT_HTTPHEADER => $headers,
            CURLOPT_FAILONERROR => true,
        ]);
        if ($out) {
            curl_setopt($ch, CURLOPT_FILE, $out);
        } else {
            curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
        }
        $body = curl_exec($ch);
        $err = curl_error($ch);
        curl_close($ch);
        if ($out) {
            fclose($out);
        }
        if ($body === false) {
            throw new RuntimeException("download failed: $err");
        }
        return $saveTo ? true : (string)$body;
    }
    if (!filter_var(ini_get('allow_url_fopen'), FILTER_VALIDATE_BOOLEAN)) {
        throw new RuntimeException('PHP can\'t download: enable the curl extension or allow_url_fopen');
    }
    $ctx = stream_context_create(['http' => [
        'header' => implode("\r\n", $headers),
        'timeout' => $timeout,
        'follow_location' => 1,
        'max_redirects' => 5,
        'ignore_errors' => false,
    ]]);
    $in = @fopen($url, 'rb', false, $ctx);
    if (!$in) {
        throw new RuntimeException('download failed: ' . (error_get_last()['message'] ?? $url));
    }
    if ($saveTo) {
        $out = fopen($saveTo, 'wb');
        $ok = stream_copy_to_stream($in, $out) !== false;
        fclose($out);
        fclose($in);
        if (!$ok) {
            throw new RuntimeException("download of $url was interrupted");
        }
        return true;
    }
    $body = stream_get_contents($in);
    fclose($in);
    return (string)$body;
}

/**
 * Checks a signed station-update.json. Returns its payload, or throws.
 * The signature covers the payload text exactly as published.
 */
function slip_update_verify(string $raw, array $keys): array
{
    $env = json_decode($raw, true);
    if (!is_array($env) || !is_string($env['payload'] ?? null) || !is_string($env['sig'] ?? null)) {
        throw new RuntimeException('the release\'s station-update.json is malformed');
    }
    if (!function_exists('sodium_crypto_sign_verify_detached')) {
        throw new RuntimeException('PHP\'s sodium extension is needed to check updates');
    }
    if (!$keys) {
        throw new RuntimeException('this station has no release key (lib/release-keys.txt), so it can\'t check updates');
    }
    $sig = @hex2bin($env['sig']);
    $ok = false;
    foreach ($keys as $key) {
        if ($sig !== false && strlen($sig) === SODIUM_CRYPTO_SIGN_BYTES
            && sodium_crypto_sign_verify_detached($sig, SLIP_UPDATE_CONTEXT . $env['payload'], hex2bin($key))) {
            $ok = true;
            break;
        }
    }
    if (!$ok) {
        throw new RuntimeException('the release isn\'t signed with this station\'s release key — not installing it');
    }
    $p = json_decode($env['payload'], true);
    if (!is_array($p) || ($p['format'] ?? 0) !== 1 || !isset($p['version'], $p['build'], $p['files'], $p['assets'])
        || !is_array($p['files']) || !is_array($p['assets'])) {
        throw new RuntimeException('the release\'s station-update.json has an unknown format');
    }
    foreach ($p['assets'] as $name => $a) {
        if (!preg_match('/^[A-Za-z0-9._-]+$/', (string)$name) || !preg_match('/^[0-9a-f]{64}$/', (string)($a['sha256'] ?? ''))) {
            throw new RuntimeException('the release lists an invalid download');
        }
    }
    foreach ($p['files'] as $f) {
        if (!slip_update_path_ok((string)($f['path'] ?? '')) || !preg_match('/^[0-9a-f]{64}$/', (string)($f['sha256'] ?? ''))
            || !isset($p['assets'][$f['from'] ?? ''])) {
            throw new RuntimeException('the release lists an invalid file: ' . ($f['path'] ?? '?'));
        }
    }
    return $p;
}

/** Only the station's own files: never settings, data or the managed .htaccess. */
function slip_update_path_ok(string $path): bool
{
    return (bool)preg_match('#^(?:[A-Za-z0-9_-][A-Za-z0-9._-]*/)*[A-Za-z0-9_.-]+$#', $path)
        && !preg_match('#(^|/)\.\.?(/|$)#', $path)
        && !in_array($path, ['.htaccess', 'config.php'], true)
        && !preg_match('#^data(/|$)#', $path);
}

/**
 * Looks for a newer release on GitHub (at most every few minutes unless
 * $force). Records the result in the update state and returns it.
 */
function slip_update_check(bool $force = false): array
{
    $state = slip_update_state();
    if (!$force && time() - (int)($state['checked'] ?? 0) < 300) {
        return $state;
    }
    $state['checked'] = time();
    $state['error'] = null;
    try {
        // config.php may point at a mirror of the GitHub releases API instead.
        $api = (string)(slip_config()['update_releases_url'] ?? '') ?: 'https://api.github.com/repos/' . slip_update_repo() . '/releases/latest';
        $rel = json_decode((string)slip_http_get($api), true);
        $url = null;
        $assets = [];
        foreach ((array)($rel['assets'] ?? []) as $a) {
            $assets[(string)($a['name'] ?? '')] = (string)($a['browser_download_url'] ?? '');
        }
        $url = $assets[SLIP_UPDATE_MANIFEST] ?? null;
        if (!$url) {
            throw new RuntimeException('the latest release has no ' . SLIP_UPDATE_MANIFEST);
        }
        $raw = (string)slip_http_get($url);
        $p = slip_update_verify($raw, slip_release_keys());
        $state['latest'] = [
            'version' => (string)$p['version'],
            'build' => (int)$p['build'],
            'commit' => (string)($p['commit'] ?? ''),
            'page' => (string)($rel['html_url'] ?? ''),
            'assets' => array_intersect_key($assets, $p['assets']),
            'manifest' => $raw,
        ];
    } catch (Throwable $e) {
        // Forget what an earlier check found: never install on stale trust.
        unset($state['latest']);
        $state['error'] = $e->getMessage();
    }
    slip_update_save($state);
    return $state;
}

/** A verified newer release, if the last check found one. */
function slip_update_available(?array $state = null): ?array
{
    $state ??= slip_update_state();
    $latest = $state['latest'] ?? null;
    return $latest && (int)$latest['build'] > slip_installed()['build'] ? $latest : null;
}

/**
 * Does the next piece of the install: downloads one file, or unpacks and
 * installs once everything is here. Returns ['done' => bool, 'message' => …].
 * Holds a lock, so overlapping calls just report that one is running.
 */
function slip_update_step(): array
{
    $lock = fopen(slip_update_dir() . '/install.lock', 'c');
    if (!$lock || !flock($lock, LOCK_EX | LOCK_NB)) {
        return ['done' => false, 'busy' => true, 'message' => 'Another update step is running…'];
    }
    @set_time_limit(600);
    try {
        $state = slip_update_state();
        $latest = slip_update_available($state);
        if (!$latest) {
            return ['done' => true, 'message' => 'The station is up to date.'];
        }
        // Check the stored list again: the state file is only as trusted as the data folder.
        $p = slip_update_verify((string)$latest['manifest'], slip_release_keys());
        if ((int)$p['build'] !== (int)$latest['build']) {
            throw new RuntimeException('the stored update doesn\'t match its signature');
        }
        $stage = slip_update_dir() . '/build-' . (int)$p['build'];
        slip_update_cleanup((int)$p['build']);
        @mkdir($stage, 0750, true);
        foreach ($p['assets'] as $name => $a) {
            $file = "$stage/$name";
            if (is_file($file) && filesize($file) === (int)$a['size'] && hash_file('sha256', $file) === $a['sha256']) {
                continue;
            }
            $url = (string)($latest['assets'][$name] ?? '');
            // Plain HTTP only for a mirror the admin configured (e.g. on a LAN).
            $mirror = (string)(slip_config()['update_releases_url'] ?? '') !== '';
            if ($url === '') {
                throw new RuntimeException("the release has no download for $name");
            }
            if (!preg_match($mirror ? '#^https?://#' : '#^https://#', $url)) {
                throw new RuntimeException("the download for $name isn't HTTPS");
            }
            slip_http_get($url, "$file.part", 900);
            if (filesize("$file.part") !== (int)$a['size'] || hash_file('sha256', "$file.part") !== $a['sha256']) {
                @unlink("$file.part");
                throw new RuntimeException("$name didn't match the signed checksum — try again");
            }
            rename("$file.part", $file);
            $have = count(array_filter(array_keys($p['assets']), fn($n) => is_file("$stage/$n")));
            return ['done' => false, 'message' => "Downloaded $name ($have of " . count($p['assets']) . ')'];
        }
        $changed = slip_update_install($p, $stage);
        $state = slip_update_state();
        $state['installed'] = ['version' => $p['version'], 'build' => (int)$p['build'], 'at' => time()];
        $state['error'] = null;
        slip_update_save($state);
        slip_update_cleanup();
        if (in_array('bin', $changed, true)) {
            slip_relay_ensure(true);
        }
        return ['done' => true, 'message' => 'Installed version ' . $p['version'] . ' (build ' . (int)$p['build'] . ').'];
    } catch (Throwable $e) {
        $state = slip_update_state();
        $state['error'] = $e->getMessage();
        slip_update_save($state);
        return ['done' => true, 'error' => true, 'message' => 'Update stopped: ' . $e->getMessage()];
    } finally {
        flock($lock, LOCK_UN);
        fclose($lock);
    }
}

/**
 * Unpacks and checks every file, then swaps them in: station files, the
 * relay, the apps, and the apps' manifest last. The replaced files are kept
 * in the data folder (update/previous) until the next update. Returns the
 * top-level folders that changed.
 */
function slip_update_install(array $p, string $stage): array
{
    $root = realpath(SLIP_ROOT);
    $tars = [];
    $files = [];
    // Check everything before replacing anything. Apps are copied from the
    // staged downloads; only the small station files are held in memory.
    foreach ($p['files'] as $f) {
        $from = "$stage/" . $f['from'];
        if (empty($f['in_tar'])) {
            if (hash_file('sha256', $from) !== $f['sha256']) {
                throw new RuntimeException("{$f['path']} didn't match the signed checksum");
            }
            $files[$f['path']] = ['file' => $from, 'exec' => !empty($f['exec']), 'sha256' => $f['sha256']];
            continue;
        }
        $tars[$from] ??= slip_tar_read($from);
        $bytes = $tars[$from][$f['path']] ?? null;
        if ($bytes === null || hash('sha256', $bytes) !== $f['sha256']) {
            throw new RuntimeException("{$f['path']} is missing from {$f['from']} or didn't match the signed checksum");
        }
        $files[$f['path']] = ['bytes' => $bytes, 'exec' => !empty($f['exec']), 'sha256' => $f['sha256']];
    }
    // Apps' manifest after the apps, and our own version stamp at the very end.
    $last = ['clients/manifest.json' => 1, 'lib/version.json' => 2];
    uksort($files, fn($a, $b) => ($last[$a] ?? 0) <=> ($last[$b] ?? 0) ?: strcmp($a, $b));
    $previous = slip_update_dir() . '/previous';
    slip_rmtree($previous);
    $changed = [];
    foreach ($files as $path => $f) {
        $dest = "$root/$path";
        if (is_file($dest) && hash_file('sha256', $dest) === $f['sha256']) {
            continue;
        }
        $dir = dirname($dest);
        if (!is_dir($dir) && !@mkdir($dir, 0755, true)) {
            throw new RuntimeException("can't create $dir");
        }
        if (is_file($dest)) {
            @mkdir(dirname("$previous/$path"), 0750, true);
            @copy($dest, "$previous/$path");
        }
        $tmp = "$dest.update-" . bin2hex(random_bytes(4));
        $ok = isset($f['file']) ? @copy($f['file'], $tmp) : @file_put_contents($tmp, $f['bytes']) === strlen($f['bytes']);
        if (!$ok || hash_file('sha256', $tmp) !== $f['sha256']) {
            @unlink($tmp);
            throw new RuntimeException("can't write $path: is the site folder writable by PHP, with space left?");
        }
        @chmod($tmp, $f['exec'] ? 0755 : 0644);
        if (!rename($tmp, $dest)) {
            @unlink($tmp);
            throw new RuntimeException("can't replace $path");
        }
        $changed[strtok($path, '/')] = true;
    }
    if (function_exists('opcache_reset')) {
        @opcache_reset();
    }
    return array_keys($changed);
}

/** Reads the regular files of an uncompressed ustar archive into [path => bytes]. */
function slip_tar_read(string $file): array
{
    $data = (string)file_get_contents($file);
    $out = [];
    for ($at = 0; $at + 512 <= strlen($data);) {
        $h = substr($data, $at, 512);
        if (trim($h, "\0") === '') {
            break;
        }
        $name = rtrim(substr($h, 0, 100), "\0");
        $prefix = rtrim(substr($h, 345, 155), "\0");
        $size = octdec(trim(substr($h, 124, 12), "\0 "));
        $type = $h[156];
        $path = $prefix !== '' ? "$prefix/$name" : $name;
        if ($type === '0' || $type === "\0") {
            $out[preg_replace('#^\./#', '', $path)] = substr($data, $at + 512, $size);
        }
        $at += 512 + (int)ceil($size / 512) * 512;
    }
    return $out;
}

function slip_rmtree(string $dir): void
{
    if (!is_dir($dir) || is_link($dir)) {
        return;
    }
    foreach (new RecursiveIteratorIterator(new RecursiveDirectoryIterator($dir, FilesystemIterator::SKIP_DOTS), RecursiveIteratorIterator::CHILD_FIRST) as $f) {
        $f->isDir() && !$f->isLink() ? @rmdir($f->getPathname()) : @unlink($f->getPathname());
    }
    @rmdir($dir);
}

/** Drops staged downloads, except those for build $keep. */
function slip_update_cleanup(int $keep = 0): void
{
    foreach (glob(slip_update_dir() . '/build-*') ?: [] as $dir) {
        if (basename($dir) !== "build-$keep") {
            slip_rmtree($dir);
        }
    }
}

/**
 * Called after an API request has been answered: checks GitHub every few
 * hours and, if automatic updates are on, installs a newer release in the
 * background. Needs PHP-FPM (or LiteSpeed) to finish the request first;
 * otherwise the admin page offers the update instead.
 */
function slip_update_background(): void
{
    if (!slip_station_ready() || !slip_release_keys()) {
        return;
    }
    $state = slip_update_state();
    $installing = slip_update_available($state) && slip_update_auto();
    $due = time() - (int)($state['checked'] ?? 0) >= SLIP_UPDATE_EVERY
        || ($installing && time() - (int)($state['step_at'] ?? 0) >= SLIP_UPDATE_RETRY);
    if (!$due) {
        return;
    }
    $finish = function_exists('fastcgi_finish_request') ? 'fastcgi_finish_request'
        : (function_exists('litespeed_finish_request') ? 'litespeed_finish_request' : null);
    if (!$finish) {
        return;
    }
    $finish();
    ignore_user_abort(true);
    @set_time_limit(1800);
    $state = slip_update_check(true);
    if (!slip_update_auto() || !slip_update_available($state)) {
        return;
    }
    for ($i = 0; $i < 20; $i++) {
        $s = slip_update_state();
        $s['step_at'] = time();
        slip_update_save($s);
        $r = slip_update_step();
        if (!empty($r['done']) || !empty($r['busy'])) {
            break;
        }
    }
}
