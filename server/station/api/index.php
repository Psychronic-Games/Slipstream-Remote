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

/** Invalid signatures use their own IP bucket; valid devices never share it. */
function verified_device_request(array $req, string $op): object
{
    try {
        return slip_verify($req, $op);
    } catch (InvalidArgumentException | DomainException $e) {
        if (!slip_rate_ok('bad_signature', 30)) fail('too many invalid signed requests', 429);
        throw $e;
    }
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
// After answering, look for a newer station release now and then (see ../lib/update.php).
register_shutdown_function('slip_update_background');
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

// Technician accounts: a sign-in gives the app this station's config
// (access key included) and a token for the rest, so these work without
// the key too. See the accounts section of ../lib/slipstream.php.
$op = (string)($req['op'] ?? '');
if ($op === 'login') {
    if (!slip_rate_ok('login', 20)) {
        fail('too many sign-in attempts, wait a minute', 429);
    }
    $u = slip_user_norm($req['username'] ?? '');
    $pw = (string)($req['password'] ?? '');
    if (!slip_rate_ok("login|$u", 10)) {
        fail('too many attempts for that account, wait a minute', 429);
    }
    $acct = slip_user_load($u);
    // The same work for unknown names, so timing doesn't tell them apart.
    $hash = $acct['hash'] ?? '$2y$12$qbJtzQ04aH6DWxr8jJh0g.VgXGS8MRZznxb3JTdipy/6KCYInoaAu';
    if (!password_verify($pw, $hash) || !$acct) {
        fail('wrong username or password', 403);
    }
    $token = slip_token_issue($u, slip_clip(trim((string)($req['device'] ?? '')), 64));
    reply(['ok' => true, 'token' => $token, 'user' => slip_user_public($acct), 'station' => slip_client_config(), 'vault_salt' => $acct['vault_salt']]);
}
if (in_array($op, ['account', 'logout', 'device_save', 'device_delete', 'group_save', 'group_delete', 'shared_save', 'shared_delete'], true)) try {
    if (!slip_rate_ok('account', 120)) {
        fail('too many requests, slow down', 429);
    }
    $acct = slip_token_user($req['token'] ?? null);
    if (!$acct) {
        fail('signed out: sign in to this station again', 401);
    }
    $u = $acct['username'];
    $isAdmin = !empty($acct['admin']);
    switch ($op) {
        case 'logout':
            slip_token_revoke($req['token']);
            reply(['ok' => true]);
        case 'device_save':
            $d = slip_clean_device($req['device'] ?? null);
            if (!$d) {
                fail('a device needs a 9-digit ID');
            }
            $acct = slip_user_update($u, function (array $a) use ($d) {
                $list = array_values(array_filter($a['devices'] ?? [], fn($x) => ($x['id'] ?? 0) !== $d['id']));
                if (count($list) >= SLIP_MAX_DEVICES) {
                    throw new InvalidArgumentException('your list is full (' . SLIP_MAX_DEVICES . ' devices)');
                }
                $old = array_values(array_filter($a['devices'] ?? [], fn($x) => ($x['id'] ?? 0) === $d['id']))[0] ?? null;
                $d = slip_merge_device_groups($d, $old);
                $d['added'] = (int)($old['added'] ?? time());
                $list[] = $d;
                $a['devices'] = $list;
                return $a;
            });
            break;
        case 'device_delete':
            $id = $req['id'] ?? null;
            $acct = slip_user_update($u, function (array $a) use ($id) {
                $a['devices'] = array_values(array_filter($a['devices'] ?? [], fn($x) => ($x['id'] ?? 0) !== $id));
                return $a;
            });
            break;
        case 'group_save':
        case 'group_delete':
            $group = $req['group'] ?? null;
            if (!is_string($group) || trim($group) === '') {
                fail('enter a group name');
            }
            $group = slip_clip(trim($group), 64);
            $acct = slip_user_update($u, function (array $a) use ($group, $op) {
                $groups = array_values(array_filter($a['groups'] ?? [], 'is_string'));
                if ($op === 'group_save') {
                    if (!in_array($group, $groups, true)) {
                        if (count($groups) >= 100) {
                            throw new InvalidArgumentException('your list is full (100 groups)');
                        }
                        $groups[] = $group;
                    }
                } else {
                    foreach ($a['devices'] ?? [] as $device) {
                        if (slip_device_in_group($device, $group)) {
                            throw new InvalidArgumentException('move devices out of this group before removing it');
                        }
                    }
                    $groups = array_values(array_filter($groups, fn($name) => $name !== $group));
                }
                natcasesort($groups);
                $a['groups'] = array_values($groups);
                return $a;
            });
            if ($op === 'group_delete') slip_forget_group_assignments($u, $group);
            break;
        case 'shared_save':
        case 'shared_delete':
            if (!$isAdmin) {
                fail('only station admins can change the shared list', 403);
            }
            $d = $op === 'shared_save' ? slip_clean_device($req['device'] ?? null, false) : null;
            if ($op === 'shared_save' && !$d) {
                fail('a device needs a 9-digit ID');
            }
            $id = $d['id'] ?? ($req['id'] ?? null);
            slip_shared_update(function (array $list) use ($d, $id, $u) {
                $old = array_values(array_filter($list, fn($x) => ($x['id'] ?? 0) === $id))[0] ?? null;
                $list = array_filter($list, fn($x) => ($x['id'] ?? 0) !== $id);
                if ($d) {
                    $d = slip_merge_device_groups($d, $old);
                    $list[] = $d + ['added_by' => $u, 'added' => time()];
                }
                return $list;
            });
            break;
    }
    // Shared visibility is enforced here, including for older clients.
    $visible = slip_shared_for_user($u);
    // Every account call answers with the whole picture.
    reply([
        'ok' => true,
        'user' => slip_user_public($acct),
        'devices' => slip_client_devices($acct['devices'] ?? []),
        'groups' => slip_personal_groups($acct),
        'shared' => $visible['devices'],
        'shared_groups' => $visible['groups'],
        'vault_salt' => $acct['vault_salt'],
        'station' => slip_client_config(),
    ]);
} catch (InvalidArgumentException $e) {
    fail($e->getMessage(), 400);
} catch (Throwable $e) {
    error_log('slipstream: ' . $e->getMessage());
    fail('server error', 500);
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
        case 'meeting_relay':
            if (!slip_rate_ok('meeting_relay', 20)) {
                fail('too many meeting requests, wait a minute', 429);
            }
            $relayKey = slip_access_key();
            $body = 'meeting1.' . (time() + 4 * 3600) . '.' . bin2hex(random_bytes(16));
            reply(['ok' => true, 'token' => $relayKey === null ? null :
                $body . '.' . hash_hmac('sha256', $body, $relayKey)]);

        case 'hello':
            reply([
                'ok' => true,
                'version' => SLIP_VERSION,
                'time' => time(),
                'relays' => slip_relays(),
                'motd' => ($station['motd'] ?? '') !== '' ? $station['motd'] : null,
                'org' => ($station['org'] ?? '') !== '' ? $station['org'] : null,
                'public_relays' => (bool)($station['public_relays'] ?? true),
                'approve_links' => (bool)($station['approve_links'] ?? false),
                'inventory' => slip_access_key() !== null,
                'inventory_report' => slip_access_key() !== null,
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

        case 'presence':
            $ids = $req['ids'] ?? null;
            if (!is_array($ids) || !array_is_list($ids) || count($ids) > 500) {
                fail('presence needs a list of at most 500 device IDs');
            }
            foreach ($ids as $id) {
                if (!is_int($id) || $id < SLIP_ID_MIN || $id > SLIP_ID_MAX) fail('invalid presence device ID');
            }
            if (!slip_rate_ok('presence', (int)$cfg['presence_per_minute'])) {
                fail('online-status refresh limit reached — wait a minute', 429);
            }
            reply(['ok' => true, 'devices' => slip_store()->presence(array_values(array_unique($ids)))]);

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

        case 'inventory':
            if (slip_access_key() === null) fail('inventory is only available on private stations', 403);
            $p = verified_device_request($req, 'inventory');
            $inventory = slip_clean_inventory($p->inventory ?? null);
            if (!slip_rate_ok('inventory', (int)$cfg['inventory_per_minute'], 'device:' . $p->endpoint)) fail('too many inventory requests for this device', 429);
            slip_inventory_save($p->endpoint, $inventory);
            reply(['ok' => true]);

        case 'register':
        case 'unregister':
            $p = verified_device_request($req, $req['op']);
            if (!slip_rate_ok('register', (int)$cfg['registers_per_minute'], 'device:' . $p->endpoint)) {
                fail('too many heartbeats for this device — wait a minute', 429);
            }
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
            // Discovery is retried on later heartbeats if shared storage is
            // temporarily unavailable; it must not break the device's ID.
            try {
                slip_enroll_device($p->endpoint, $id, $info['name']);
            } catch (Throwable $e) {
                error_log('slipstream enrollment: ' . $e->getMessage());
            }
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
