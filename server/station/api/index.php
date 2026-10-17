<?php
/**
 * Slipstream Station directory API: POST {"op": …} as JSON.
 * See ../lib/slipstream.php and crates/slipstream-core/src/directory.rs.
 */
require __DIR__ . '/../lib/slipstream.php';

header('Content-Type: application/json');
header('Cache-Control: no-store');

function reply(array $data, int $status = 200): void
{
    http_response_code($status);
    echo json_encode($data, JSON_UNESCAPED_SLASHES);
    exit;
}

function fail(string $msg, int $status = 400): void
{
    reply(['ok' => false, 'error' => $msg], $status);
}

if (($_SERVER['REQUEST_METHOD'] ?? 'GET') !== 'POST') {
    fail('Slipstream directory API — POST JSON here', 405);
}
if (!function_exists('sodium_crypto_sign_verify_detached')) {
    fail('server misconfigured: PHP sodium extension missing', 500);
}
if (!slip_station_ready()) {
    fail('this station has not been set up yet — open its /admin/ page', 503);
}
$req = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($req)) {
    fail('expected a JSON body');
}

$cfg = slip_config();
$station = slip_station();

// Setup codes (from the download page) buy an app this station's config,
// access key included, so they come before the key check.
if (($req['op'] ?? '') === 'link') {
    if (!slip_rate_ok('link', 20)) {
        fail('too many attempts, slow down', 429);
    }
    if (!slip_link_ok((string)($req['code'] ?? ''))) {
        fail('that setup code is not valid — codes last ' . SLIP_CODE_DAYS . ' days; get a new one from ' . slip_base_url(), 403);
    }
    reply(['ok' => true] + slip_client_config());
}

// Private stations only serve apps carrying the access key.
if ($key = slip_access_key()) {
    $given = $req['key'] ?? '';
    if (!is_string($given) || !slip_same_key($given, $key)) {
        if (!slip_rate_ok('badkey', 20)) {
            fail('too many requests, slow down', 429);
        }
        fail('this is a private Slipstream station — download the app from ' . slip_base_url(), 403);
    }
}

try {
    switch ($req['op'] ?? '') {
        case 'hello':
            reply([
                'ok' => true,
                'version' => SLIP_VERSION,
                'time' => time(),
                'relays' => slip_relays(),
                'motd' => ($station['motd'] ?? '') !== '' ? $station['motd'] : null,
                'org' => ($station['org'] ?? '') !== '' ? $station['org'] : null,
                'public_relays' => (bool)($station['public_relays'] ?? true),
            ]);

        case 'update_check':
            $m = slip_client_manifest();
            $f = $m['platforms'][(string)($req['platform'] ?? '')] ?? null;
            if (!$m || !$f || !is_file(SLIP_ROOT . '/clients/' . basename((string)$f['file']))) {
                reply(['ok' => true, 'update' => null]);
            }
            reply(['ok' => true, 'update' => [
                'version' => (string)$m['version'],
                'build' => (int)$m['build'],
                'commit' => (string)($m['commit'] ?? ''),
                'size' => (int)$f['size'],
                'sha256' => (string)$f['sha256'],
                'os' => (string)$f['os'],
                'sig' => (string)($f['sig'] ?? ''),
            ]]);

        case 'lookup':
            if (!slip_rate_ok('lookup', (int)$cfg['lookups_per_minute'])) {
                fail('too many lookups, slow down', 429);
            }
            $id = $req['id'] ?? null;
            if (!is_int($id) || $id < SLIP_ID_MIN || $id > SLIP_ID_MAX) {
                fail('missing or invalid id');
            }
            $r = slip_store()->lookup($id);
            $out = ['ok' => true, 'status' => $r['status']];
            if (isset($r['info'])) {
                $out['info'] = $r['info'];
            }
            if (isset($r['addr'])) {
                // Stored as JSON text; splice it in untouched.
                $json = json_encode($out, JSON_UNESCAPED_SLASHES);
                http_response_code(200);
                echo substr($json, 0, -1) . ',"addr":' . $r['addr'] . '}';
                exit;
            }
            reply($out);

        case 'register':
        case 'unregister':
            if (!slip_rate_ok('register', (int)$cfg['registers_per_minute'])) {
                fail('too many requests, slow down', 429);
            }
            $p = slip_verify($req, $req['op']);
            if ($req['op'] === 'unregister') {
                slip_store()->unregister($p->endpoint);
                reply(['ok' => true]);
            }
            if (!isset($p->addr)) {
                fail('missing addr');
            }
            $i = $p->info ?? new stdClass();
            $info = [
                'name' => slip_clip($i->name ?? '', 64),
                'os' => slip_clip($i->os ?? '', 32),
                'app_version' => slip_clip($i->app_version ?? '', 32),
            ];
            $id = slip_store()->register($p->endpoint, $info, json_encode($p->addr, JSON_UNESCAPED_SLASHES));
            reply(['ok' => true, 'id' => $id]);

        default:
            fail('unknown op');
    }
} catch (InvalidArgumentException $e) {
    fail($e->getMessage(), 400);
} catch (DomainException $e) {
    fail($e->getMessage(), 403);
} catch (Throwable $e) {
    error_log('slipstream: ' . $e->getMessage());
    fail('server error', 500);
}
