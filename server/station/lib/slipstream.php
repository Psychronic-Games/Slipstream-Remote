<?php
/**
 * Slipstream Station — shared library.
 *
 * A station is one self-contained web folder that gives an organisation its
 * own private Slipstream service: a directory of its devices (this PHP), a
 * relay (a small program the PHP starts and supervises), preconfigured
 * client downloads and an admin page.
 *
 * Speaks the directory protocol in crates/slipstream-core/src/directory.rs.
 * MIT License.
 */

if (realpath($_SERVER['SCRIPT_FILENAME'] ?? '') === realpath(__FILE__)) {
    http_response_code(404);
    exit;
}

const SLIP_VERSION = 2;                 // directory protocol version
const SLIP_SIGNING_CONTEXT = "slipstream-directory-v2\n";
const SLIP_ONLINE_WINDOW = 75;          // seconds without heartbeat => offline
const SLIP_MAX_SKEW = 600;              // signed request freshness (seconds)
const SLIP_ID_MIN = 100000000;
const SLIP_ID_MAX = 999999999;
const SLIP_ROOT = __DIR__ . '/..';

// ── Configuration ──────────────────────────────────────────────────────────

/** Static overrides from config.php (all optional). */
function slip_config(): array
{
    static $config = null;
    if ($config !== null) {
        return $config;
    }
    $defaults = [
        'data_dir' => slip_default_data_dir(),
        'db' => null,
        'lookups_per_minute' => 60,
        'registers_per_minute' => 30,
        'relay_bytes_per_second' => 0,
        'relay_max_connections' => 0,
    ];
    $file = SLIP_ROOT . '/config.php';
    $user = is_file($file) ? (include $file) : [];
    $config = array_merge($defaults, is_array($user) ? $user : []);
    return $config;
}

/**
 * Prefer a folder *outside* the web root (e.g. /home/site/slipstream-data
 * next to public_html) so records can never be downloaded; fall back to
 * ./data protected by .htaccess.
 */
function slip_default_data_dir(): string
{
    $docRoot = realpath($_SERVER['DOCUMENT_ROOT'] ?? '') ?: realpath(SLIP_ROOT);
    if ($docRoot) {
        $outside = dirname($docRoot) . '/slipstream-data';
        if (is_dir($outside) || (is_writable(dirname($docRoot)) && @mkdir($outside, 0750))) {
            return $outside;
        }
    }
    return realpath(SLIP_ROOT) . '/data';
}

function slip_data_dir(): string
{
    $dir = rtrim(slip_config()['data_dir'], '/');
    if (!is_dir($dir)) {
        @mkdir($dir, 0750, true);
    }
    if (is_dir($dir) && !is_file("$dir/.htaccess")) {
        @file_put_contents("$dir/.htaccess", slip_deny_htaccess());
        @file_put_contents("$dir/index.html", '');
    }
    return $dir;
}

function slip_deny_htaccess(): string
{
    return "<IfModule mod_authz_core.c>\n  Require all denied\n</IfModule>\n<IfModule !mod_authz_core.c>\n  Deny from all\n</IfModule>\n";
}

/**
 * Station settings, managed from the admin page and kept in the data
 * folder (station.json). Empty until first-run setup.
 */
function slip_station(): array
{
    if (!isset($GLOBALS['slip_station_cache'])) {
        $raw = @file_get_contents(slip_data_dir() . '/station.json');
        $GLOBALS['slip_station_cache'] = $raw ? (json_decode($raw, true) ?: []) : [];
    }
    return $GLOBALS['slip_station_cache'];
}

function slip_station_ready(): bool
{
    return !empty(slip_station()['admin_hash']);
}

function slip_station_save(array $changes): array
{
    $path = slip_data_dir() . '/station.json';
    $fh = fopen("$path.lock", 'c');
    flock($fh, LOCK_EX);
    $cur = json_decode((string)@file_get_contents($path), true) ?: [];
    $new = array_merge($cur, $changes);
    $tmp = $path . '.' . bin2hex(random_bytes(4));
    file_put_contents($tmp, json_encode($new, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES));
    @chmod($tmp, 0640);
    rename($tmp, $path);
    flock($fh, LOCK_UN);
    fclose($fh);
    $GLOBALS['slip_station_cache'] = $new;
    return $new;
}

/** Defaults for a brand-new station. */
function slip_new_station(string $org, string $adminPassword): array
{
    return [
        'org' => $org,
        'admin_hash' => password_hash($adminPassword, PASSWORD_DEFAULT),
        'access_key' => bin2hex(random_bytes(16)),
        'private' => true,
        'public_relays' => true,
        'motd' => '',
        'relay_port' => slip_pick_port(),
        'download_password_hash' => null,
        'created' => time(),
    ];
}

function slip_pick_port(): int
{
    for ($i = 0; $i < 30; $i++) {
        $port = random_int(20000, 45000);
        $s = @fsockopen('127.0.0.1', $port, $errno, $errstr, 0.2);
        if (!$s) {
            return $port;
        }
        fclose($s);
    }
    return 21117;
}

function slip_access_key(): ?string
{
    $s = slip_station();
    return !empty($s['private']) && !empty($s['access_key']) ? $s['access_key'] : null;
}

function slip_same_key(string $a, string $b): bool
{
    return hash_equals($b, $a);
}

// ── Storage ────────────────────────────────────────────────────────────────

interface SlipStore
{
    /** Returns the device's ID, assigning a new one if needed. */
    public function register(string $endpoint, array $info, string $addrJson): int;
    public function unregister(string $endpoint): void;
    /** ['status' => 'online'|'offline'|'not_found', 'addr' => json?, 'info' => array?] */
    public function lookup(int $id): array;
    /** [total, online] */
    public function counts(): array;
    /** All devices: [['id','endpoint','name','os','app_version','last_seen','created'], …] */
    public function all(): array;
    public function delete(int $id): void;
    public function name(): string;
}

/** Zero-setup storage: one small JSON file per device. */
class SlipFileStore implements SlipStore
{
    private string $dir;

    public function __construct(string $dir)
    {
        $this->dir = $dir;
        foreach (['devices', 'ids'] as $sub) {
            if (!is_dir("$dir/$sub")) {
                @mkdir("$dir/$sub", 0750, true);
            }
        }
        if (!is_writable("$dir/devices")) {
            throw new RuntimeException("data folder is not writable: $dir");
        }
    }

    public function name(): string
    {
        return 'files';
    }

    private function devicePath(string $endpoint): string
    {
        return "{$this->dir}/devices/$endpoint.json";
    }

    private function load(string $endpoint): ?array
    {
        $raw = @file_get_contents($this->devicePath($endpoint));
        $d = $raw === false ? null : json_decode($raw, true);
        return is_array($d) ? $d : null;
    }

    private function save(string $endpoint, array $d): void
    {
        $path = $this->devicePath($endpoint);
        $tmp = $path . '.' . bin2hex(random_bytes(4)) . '.tmp';
        file_put_contents($tmp, json_encode($d, JSON_UNESCAPED_SLASHES));
        rename($tmp, $path);
    }

    public function register(string $endpoint, array $info, string $addrJson): int
    {
        $d = $this->load($endpoint);
        if ($d === null) {
            $id = null;
            // Claim a free ID atomically: creating ids/<id> with 'x' fails if taken.
            for ($i = 0; $i < 50 && !$id; $i++) {
                $try = random_int(SLIP_ID_MIN, SLIP_ID_MAX);
                $fh = @fopen("{$this->dir}/ids/$try", 'x');
                if ($fh) {
                    fwrite($fh, $endpoint);
                    fclose($fh);
                    $id = $try;
                }
            }
            if (!$id) {
                throw new RuntimeException('could not allocate an ID');
            }
            $d = ['id' => $id, 'endpoint' => $endpoint, 'created' => time()];
        }
        $d = array_merge($d, $info, ['addr' => $addrJson, 'last_seen' => time()]);
        $this->save($endpoint, $d);
        return (int)$d['id'];
    }

    public function unregister(string $endpoint): void
    {
        $d = $this->load($endpoint);
        if ($d !== null) {
            $d['last_seen'] = 0;
            $this->save($endpoint, $d);
        }
    }

    private function byId(int $id): ?array
    {
        $endpoint = @file_get_contents("{$this->dir}/ids/$id");
        return ($endpoint !== false && preg_match('/^[0-9a-f]{64}$/', $endpoint)) ? $this->load($endpoint) : null;
    }

    public function lookup(int $id): array
    {
        $d = $this->byId($id);
        if ($d === null) {
            return ['status' => 'not_found'];
        }
        $info = ['name' => $d['name'] ?? '', 'os' => $d['os'] ?? '', 'app_version' => $d['app_version'] ?? ''];
        if (!empty($d['addr']) && time() - ($d['last_seen'] ?? 0) <= SLIP_ONLINE_WINDOW) {
            return ['status' => 'online', 'addr' => $d['addr'], 'info' => $info];
        }
        return ['status' => 'offline', 'info' => $info];
    }

    public function all(): array
    {
        $out = [];
        foreach (glob("{$this->dir}/devices/*.json") ?: [] as $f) {
            $d = json_decode((string)@file_get_contents($f), true);
            if (is_array($d)) {
                $d['endpoint'] = $d['endpoint'] ?? basename($f, '.json');
                $out[] = $d;
            }
        }
        return $out;
    }

    public function counts(): array
    {
        $all = $this->all();
        $cut = time() - SLIP_ONLINE_WINDOW;
        return [count($all), count(array_filter($all, fn($d) => ($d['last_seen'] ?? 0) >= $cut))];
    }

    public function delete(int $id): void
    {
        $d = $this->byId($id);
        if ($d) {
            @unlink($this->devicePath($d['endpoint'] ?? ''));
        }
        @unlink("{$this->dir}/ids/$id");
    }
}

/** MariaDB/MySQL or SQLite via PDO, for bigger installs. */
class SlipPdoStore implements SlipStore
{
    private PDO $db;
    private string $driver;

    public function __construct(array $cfg)
    {
        $this->db = new PDO($cfg['dsn'], $cfg['user'] ?? null, $cfg['pass'] ?? null, [
            PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        ]);
        $this->driver = $this->db->getAttribute(PDO::ATTR_DRIVER_NAME);
        $text = $this->driver === 'mysql' ? 'VARCHAR(255)' : 'TEXT';
        $this->db->exec("CREATE TABLE IF NOT EXISTS slipstream_devices (
            endpoint CHAR(64) PRIMARY KEY,
            device_id INTEGER NOT NULL UNIQUE,
            name $text NOT NULL DEFAULT '',
            os $text NOT NULL DEFAULT '',
            app_version $text NOT NULL DEFAULT '',
            addr TEXT,
            created BIGINT NOT NULL,
            last_seen BIGINT NOT NULL
        )");
    }

    public function name(): string
    {
        return $this->driver;
    }

    public function register(string $endpoint, array $info, string $addrJson): int
    {
        $q = $this->db->prepare('SELECT device_id FROM slipstream_devices WHERE endpoint = ?');
        $q->execute([$endpoint]);
        $id = $q->fetchColumn();
        if ($id !== false) {
            $this->db->prepare('UPDATE slipstream_devices SET name=?, os=?, app_version=?, addr=?, last_seen=? WHERE endpoint=?')
                ->execute([$info['name'], $info['os'], $info['app_version'], $addrJson, time(), $endpoint]);
            return (int)$id;
        }
        for ($i = 0; $i < 50; $i++) {
            $id = random_int(SLIP_ID_MIN, SLIP_ID_MAX);
            try {
                $this->db->prepare('INSERT INTO slipstream_devices (endpoint, device_id, name, os, app_version, addr, created, last_seen) VALUES (?,?,?,?,?,?,?,?)')
                    ->execute([$endpoint, $id, $info['name'], $info['os'], $info['app_version'], $addrJson, time(), time()]);
                return $id;
            } catch (PDOException $e) {
                $q->execute([$endpoint]);
                $existing = $q->fetchColumn();
                if ($existing !== false) {
                    return (int)$existing;
                }
            }
        }
        throw new RuntimeException('could not allocate an ID');
    }

    public function unregister(string $endpoint): void
    {
        $this->db->prepare('UPDATE slipstream_devices SET last_seen = 0 WHERE endpoint = ?')->execute([$endpoint]);
    }

    public function lookup(int $id): array
    {
        $q = $this->db->prepare('SELECT name, os, app_version, addr, last_seen FROM slipstream_devices WHERE device_id = ?');
        $q->execute([$id]);
        $d = $q->fetch(PDO::FETCH_ASSOC);
        if (!$d) {
            return ['status' => 'not_found'];
        }
        $info = ['name' => $d['name'], 'os' => $d['os'], 'app_version' => $d['app_version']];
        if (!empty($d['addr']) && time() - (int)$d['last_seen'] <= SLIP_ONLINE_WINDOW) {
            return ['status' => 'online', 'addr' => $d['addr'], 'info' => $info];
        }
        return ['status' => 'offline', 'info' => $info];
    }

    public function all(): array
    {
        $rows = $this->db->query('SELECT device_id AS id, endpoint, name, os, app_version, last_seen, created FROM slipstream_devices')->fetchAll(PDO::FETCH_ASSOC);
        return array_map(fn($r) => array_merge($r, ['id' => (int)$r['id'], 'last_seen' => (int)$r['last_seen']]), $rows);
    }

    public function counts(): array
    {
        $total = (int)$this->db->query('SELECT COUNT(*) FROM slipstream_devices')->fetchColumn();
        $q = $this->db->prepare('SELECT COUNT(*) FROM slipstream_devices WHERE last_seen >= ?');
        $q->execute([time() - SLIP_ONLINE_WINDOW]);
        return [$total, (int)$q->fetchColumn()];
    }

    public function delete(int $id): void
    {
        $this->db->prepare('DELETE FROM slipstream_devices WHERE device_id = ?')->execute([$id]);
    }
}

function slip_store(): SlipStore
{
    static $store = null;
    if ($store === null) {
        $db = slip_config()['db'];
        $store = is_array($db) && !empty($db['dsn']) ? new SlipPdoStore($db) : new SlipFileStore(slip_data_dir());
    }
    return $store;
}

// ── Technician accounts ────────────────────────────────────────────────────
//
// Technicians sign in from the app, including to other organisations'
// stations (whose access key their app doesn't carry), and keep their list
// of devices here so it follows them to any computer. One JSON file per
// account in <data>/accounts/; sign-in tokens are kept only as hashes, with
// an index in <data>/tokens/. Saved device passwords arrive already
// encrypted by the app, with a key derived from the technician's password
// (crates/slipstream-core/src/vault.rs): this station stores them but has
// no key to read them. Devices in <data>/shared.json are everyone's.

const SLIP_USER_RE = '/^[a-z0-9][a-z0-9._-]{1,31}$/';
const SLIP_MAX_DEVICES = 500;

function slip_acct_dir(string $sub): string
{
    $dir = slip_data_dir() . "/$sub";
    if (!is_dir($dir)) {
        @mkdir($dir, 0750, true);
    }
    return $dir;
}

function slip_user_norm($u): string
{
    return strtolower(trim(is_string($u) ? $u : ''));
}

function slip_user_valid(string $u): bool
{
    return (bool)preg_match(SLIP_USER_RE, $u);
}

function slip_user_load(string $u): ?array
{
    if (!slip_user_valid($u)) {
        return null;
    }
    $d = json_decode((string)@file_get_contents(slip_acct_dir('accounts') . "/$u.json"), true);
    return is_array($d) ? $d : null;
}

/** Runs $fn on the account under a lock and saves what it returns. */
function slip_user_update(string $u, callable $fn): ?array
{
    $path = slip_acct_dir('accounts') . "/$u.json";
    $fh = fopen("$path.lock", 'c');
    flock($fh, LOCK_EX);
    try {
        $cur = slip_user_load($u);
        if ($cur === null) {
            return null;
        }
        $new = $fn($cur);
        $tmp = $path . '.' . bin2hex(random_bytes(4)) . '.tmp';
        file_put_contents($tmp, json_encode($new, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES));
        @chmod($tmp, 0640);
        rename($tmp, $path);
        return $new;
    } finally {
        flock($fh, LOCK_UN);
        fclose($fh);
    }
}

/** All accounts, by username. */
function slip_users(): array
{
    $out = [];
    foreach (glob(slip_acct_dir('accounts') . '/*.json') ?: [] as $f) {
        $d = json_decode((string)@file_get_contents($f), true);
        if (is_array($d) && isset($d['username'])) {
            $out[$d['username']] = $d;
        }
    }
    ksort($out);
    return $out;
}

function slip_user_create(string $u, string $name, string $pw, bool $admin): void
{
    $u = slip_user_norm($u);
    if (!slip_user_valid($u)) {
        throw new InvalidArgumentException('Usernames are 2–32 characters: letters, digits, dot, dash or underscore.');
    }
    if (strlen($pw) < 10) {
        throw new InvalidArgumentException('Use a password of at least 10 characters.');
    }
    $path = slip_acct_dir('accounts') . "/$u.json";
    $fh = @fopen($path, 'x'); // fails if the name is taken
    if (!$fh) {
        throw new InvalidArgumentException("There is already a technician called $u.");
    }
    fwrite($fh, json_encode([
        'username' => $u,
        'name' => slip_clip($name !== '' ? $name : $u, 64),
        'hash' => password_hash($pw, PASSWORD_DEFAULT),
        'admin' => $admin,
        'created' => time(),
        'vault_salt' => bin2hex(random_bytes(16)),
        'devices' => [],
        'sessions' => (object)[],
    ], JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES));
    fclose($fh);
    @chmod($path, 0640);
}

/** A new password from the admin: saved device passwords can't be opened
 *  with it, so they're dropped (with a new vault salt), and every sign-in
 *  ends. */
function slip_user_set_password(string $u, string $pw): void
{
    if (strlen($pw) < 10) {
        throw new InvalidArgumentException('Use a password of at least 10 characters.');
    }
    slip_user_signout_all($u);
    slip_user_update($u, function (array $a) use ($pw) {
        $a['hash'] = password_hash($pw, PASSWORD_DEFAULT);
        $a['vault_salt'] = bin2hex(random_bytes(16));
        foreach ($a['devices'] as &$d) {
            $d['secret'] = null;
        }
        return $a;
    });
}

function slip_user_signout_all(string $u): void
{
    $a = slip_user_load($u);
    foreach (array_keys((array)($a['sessions'] ?? [])) as $hash) {
        @unlink(slip_acct_dir('tokens') . "/$hash");
    }
    slip_user_update($u, function (array $a) {
        $a['sessions'] = (object)[];
        return $a;
    });
}

function slip_user_delete(string $u): void
{
    if (!slip_user_valid($u)) {
        return;
    }
    slip_user_signout_all($u);
    @unlink(slip_acct_dir('accounts') . "/$u.json");
    @unlink(slip_acct_dir('accounts') . "/$u.json.lock");
}

/** Signs a technician in from an app: a new token (returned once). */
function slip_token_issue(string $u, string $device): string
{
    $token = bin2hex(random_bytes(32));
    $hash = hash('sha256', $token);
    file_put_contents(slip_acct_dir('tokens') . "/$hash", $u);
    slip_user_update($u, function (array $a) use ($hash, $device) {
        $sessions = (array)($a['sessions'] ?? []);
        $sessions[$hash] = ['created' => time(), 'used' => time(), 'device' => $device];
        // The oldest sign-ins go past 20 (each app keeps its own).
        uasort($sessions, fn($x, $y) => $y['used'] <=> $x['used']);
        foreach (array_slice(array_keys($sessions), 20) as $old) {
            unset($sessions[$old]);
            @unlink(slip_acct_dir('tokens') . "/$old");
        }
        $a['sessions'] = $sessions;
        return $a;
    });
    return $token;
}

/** The account a token belongs to, or null if it's unknown or revoked. */
function slip_token_user($token): ?array
{
    if (!is_string($token) || !preg_match('/^[0-9a-f]{64}$/', $token)) {
        return null;
    }
    $hash = hash('sha256', $token);
    $u = (string)@file_get_contents(slip_acct_dir('tokens') . "/$hash");
    $a = slip_user_load($u);
    $s = ((array)($a['sessions'] ?? []))[$hash] ?? null;
    if (!$a || !$s) {
        return null;
    }
    // Note use now and then, for the admin page (not on every request).
    if (time() - (int)($s['used'] ?? 0) > 600) {
        $a = slip_user_update($u, function (array $a) use ($hash) {
            $sessions = (array)$a['sessions'];
            if (isset($sessions[$hash])) {
                $sessions[$hash]['used'] = time();
            }
            $a['sessions'] = $sessions;
            return $a;
        }) ?? $a;
    }
    return $a;
}

function slip_token_revoke($token): void
{
    $a = slip_token_user($token);
    if (!$a) {
        return;
    }
    $hash = hash('sha256', $token);
    @unlink(slip_acct_dir('tokens') . "/$hash");
    slip_user_update($a['username'], function (array $a) use ($hash) {
        $sessions = (array)$a['sessions'];
        unset($sessions[$hash]);
        $a['sessions'] = $sessions;
        return $a;
    });
}

/** What the app may know about an account. */
function slip_user_public(array $a): array
{
    return ['username' => $a['username'], 'name' => $a['name'] ?? $a['username'], 'admin' => !empty($a['admin'])];
}

/** A device entry as sent by the app, checked and trimmed; null if unusable. */
function slip_clean_device($d, bool $withSecret = true): ?array
{
    if (!is_array($d)) {
        return null;
    }
    $id = $d['id'] ?? null;
    if (!is_int($id) || $id < SLIP_ID_MIN || $id > SLIP_ID_MAX) {
        return null;
    }
    $out = ['id' => $id, 'name' => slip_clip(trim((string)($d['name'] ?? '')), 64), 'notes' => slip_clip(trim((string)($d['notes'] ?? '')), 500)];
    // Absence means an older client: retain the existing group on updates.
    // An explicit empty string moves the device back to Ungrouped.
    if (array_key_exists('group', $d)) {
        if (!is_string($d['group'])) {
            throw new InvalidArgumentException('The device group must be text.');
        }
        $out['group'] = slip_clip(trim($d['group']), 64);
    }
    $secret = $d['secret'] ?? null;
    if ($withSecret) {
        $ok = is_array($secret) && is_string($secret['n'] ?? null) && is_string($secret['c'] ?? null)
            && preg_match('/^[0-9a-f]{48}$/', $secret['n']) && preg_match('/^[0-9a-f]{32,1024}$/', $secret['c']);
        $out['secret'] = $ok ? ['n' => $secret['n'], 'c' => $secret['c']] : null;
    }
    return $out;
}

function slip_shared(): array
{
    $d = json_decode((string)@file_get_contents(slip_data_dir() . '/shared.json'), true);
    return is_array($d) ? array_values($d) : [];
}

/** Changes the shared list under a lock: $fn gets and returns it. */
function slip_shared_update(callable $fn): array
{
    $path = slip_data_dir() . '/shared.json';
    $fh = fopen("$path.lock", 'c');
    flock($fh, LOCK_EX);
    try {
        $new = array_values($fn(slip_shared()));
        $tmp = $path . '.' . bin2hex(random_bytes(4));
        file_put_contents($tmp, json_encode($new, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES));
        @chmod($tmp, 0640);
        rename($tmp, $path);
        return $new;
    } finally {
        flock($fh, LOCK_UN);
        fclose($fh);
    }
}

// ── Relay supervisor ───────────────────────────────────────────────────────
//
// The relay is the slipstream-server program in bin/, run as this site's
// own user on a private loopback port. Apache forwards /relay to it via the
// managed block in .htaccess. It is started on demand and restarted
// automatically whenever it isn't answering (e.g. after a reboot).

function slip_relay_binary(): ?string
{
    $arch = php_uname('m');
    $arch = ['amd64' => 'x86_64', 'arm64' => 'aarch64'][$arch] ?? $arch;
    $bin = realpath(SLIP_ROOT) . "/bin/slipstream-server-linux-$arch";
    return is_file($bin) ? $bin : null;
}

function slip_can_exec(): bool
{
    $disabled = array_map('trim', explode(',', (string)ini_get('disable_functions')));
    return function_exists('proc_open') && !in_array('proc_open', $disabled, true) && PHP_OS_FAMILY === 'Linux';
}

function slip_relay_running(): bool
{
    $port = (int)(slip_station()['relay_port'] ?? 0);
    if (!$port) {
        return false;
    }
    $s = @fsockopen('127.0.0.1', $port, $errno, $errstr, 0.3);
    if ($s) {
        fclose($s);
        return true;
    }
    return false;
}

/** Starts the relay if it isn't running. Returns true if it's up. */
function slip_relay_ensure(bool $force = false): bool
{
    if (!$force && slip_relay_running()) {
        return true;
    }
    $bin = slip_relay_binary();
    $port = (int)(slip_station()['relay_port'] ?? 0);
    if (!$bin || !$port || !slip_can_exec()) {
        return false;
    }
    $data = slip_data_dir();
    $lock = fopen("$data/relay.lock", 'c');
    if (!$lock || !flock($lock, LOCK_EX | LOCK_NB)) {
        return false; // someone else is starting it right now
    }
    try {
        if ($force) {
            slip_relay_stop();
        } elseif (slip_relay_running()) {
            return true;
        }
        if (!is_executable($bin)) {
            @chmod($bin, 0755);
        }
        @mkdir("$data/relay", 0750, true);
        $args = [$bin, '--relay-only', '--listen', "127.0.0.1:$port", '--data-dir', "$data/relay"];
        if (is_executable('/usr/bin/setsid')) {
            array_unshift($args, '/usr/bin/setsid');   // own session: survives PHP worker recycling
        }
        $env = ['PATH' => '/usr/bin:/bin', 'RUST_LOG' => 'warn'];
        if ($key = slip_access_key()) {
            $env['SLIPSTREAM_ACCESS_KEY'] = $key;    // env, not argv: invisible to `ps`
        }
        $cfg = slip_config();
        $env['SLIPSTREAM_RELAY_BYTES_PER_SECOND'] = (string)max(0, (int)$cfg['relay_bytes_per_second']);
        $env['SLIPSTREAM_RELAY_MAX_CONNECTIONS'] = (string)max(0, (int)$cfg['relay_max_connections']);
        $spec = [0 => ['file', '/dev/null', 'r'], 1 => ['file', "$data/relay.log", 'a'], 2 => ['file', "$data/relay.log", 'a']];
        $proc = @proc_open($args, $spec, $pipes, $data, $env);
        if (!is_resource($proc)) {
            return false;
        }
        $status = proc_get_status($proc);
        @file_put_contents("$data/relay.pid", (string)$status['pid']);
        for ($i = 0; $i < 30; $i++) {
            usleep(100000);
            if (slip_relay_running()) {
                return true;
            }
        }
        return false;
    } finally {
        flock($lock, LOCK_UN);
        fclose($lock);
    }
}

function slip_relay_stop(): void
{
    $pidFile = slip_data_dir() . '/relay.pid';
    $pid = (int)@file_get_contents($pidFile);
    if ($pid > 1 && function_exists('posix_kill')) {
        @posix_kill($pid, 15);
    } elseif ($pid > 1 && slip_can_exec()) {
        @proc_close(@proc_open(['kill', (string)$pid], [], $p));
    }
    // setsid may have forked: also stop whatever holds our port.
    for ($i = 0; $i < 20 && slip_relay_running(); $i++) {
        usleep(100000);
    }
    @unlink($pidFile);
}

/**
 * Keeps the managed block in the site's .htaccess in sync: it forwards the
 * relay URLs to the relay's port and protects the internal folders.
 */
function slip_write_htaccess(): bool
{
    $port = (int)(slip_station()['relay_port'] ?? 0);
    $path = realpath(SLIP_ROOT) . '/.htaccess';
    $block = "# BEGIN Slipstream (managed by the admin page — do not edit)\n"
        . "<IfModule mod_rewrite.c>\n"
        . "RewriteEngine On\n"
        // ws:// makes Apache (2.4.47+, mod_proxy_http) tunnel the WebSocket upgrade.
        . ($port ? "RewriteRule ^relay$ ws://127.0.0.1:$port/relay [P,L]\n"
            . "RewriteRule ^(ping|generate_204)$ http://127.0.0.1:$port/$1 [P,L]\n" : '')
        . "RewriteRule ^(lib|bin|clients|data)(/|$) - [F,L]\n"
        . "</IfModule>\n"
        . "# END Slipstream\n";
    $cur = (string)@file_get_contents($path);
    $new = preg_match('/# BEGIN Slipstream.*?# END Slipstream\n?/s', $cur)
        ? preg_replace('/# BEGIN Slipstream.*?# END Slipstream\n?/s', str_replace('$', '\$', $block), $cur)
        : $block . ($cur !== '' ? "\n" . $cur : '');
    if ($new === $cur) {
        return true;
    }
    return @file_put_contents($path, $new) !== false;
}

// ── Helpers ────────────────────────────────────────────────────────────────

/** Truncates to $max characters (works without the mbstring extension). */
function slip_clip($s, int $max): string
{
    $s = is_scalar($s) ? (string)$s : '';
    if (function_exists('mb_substr')) {
        return mb_substr($s, 0, $max);
    }
    preg_match('/^.{0,' . $max . '}/us', $s, $m);
    return $m[0] ?? '';
}

function slip_client_ip(): string
{
    return $_SERVER['REMOTE_ADDR'] ?? '0.0.0.0';
}

/** Simple per-IP rate limit using files in the data folder. */
function slip_rate_ok(string $kind, int $perMinute): bool
{
    $dir = slip_data_dir() . '/rate';
    if (!is_dir($dir)) {
        @mkdir($dir, 0750, true);
    }
    $file = $dir . '/' . sha1($kind . '|' . slip_client_ip());
    $fh = @fopen($file, 'c+');
    if (!$fh) {
        return true;
    }
    flock($fh, LOCK_EX);
    $parts = explode(' ', trim((string)stream_get_contents($fh)) ?: '0 0');
    $start = (int)($parts[0] ?? 0);
    $count = (int)($parts[1] ?? 0);
    if (time() - $start >= 60) {
        [$start, $count] = [time(), 0];
    }
    $count++;
    ftruncate($fh, 0);
    rewind($fh);
    fwrite($fh, "$start $count");
    flock($fh, LOCK_UN);
    fclose($fh);
    if (random_int(1, 200) === 1) {
        foreach (glob("$dir/*") ?: [] as $f) {
            if (filemtime($f) < time() - 120) {
                @unlink($f);
            }
        }
    }
    return $count <= $perMinute;
}

function slip_scheme(): string
{
    if (!empty($_SERVER['HTTP_X_FORWARDED_PROTO'])) {
        return explode(',', $_SERVER['HTTP_X_FORWARDED_PROTO'])[0] === 'https' ? 'https' : 'http';
    }
    return (!empty($_SERVER['HTTPS']) && $_SERVER['HTTPS'] !== 'off') ? 'https' : 'http';
}

function slip_host(): string
{
    return preg_replace('/[^A-Za-z0-9.\-:\[\]]/', '', $_SERVER['HTTP_HOST'] ?? 'localhost');
}

/** The station's public root URL, e.g. https://relay.example.com/ */
function slip_base_url(): string
{
    return slip_scheme() . '://' . slip_host() . '/';
}

/** What the apps should type/store as the server address. */
function slip_server_address(): string
{
    $url = slip_base_url();
    return slip_scheme() === 'https' ? rtrim(substr($url, 8), '/') : rtrim($url, '/');
}

/** Relays to advertise: ours (with the access key) if it's running. */
function slip_relays(): array
{
    if (!slip_relay_ensure()) {
        return [];
    }
    $url = slip_base_url();
    $key = slip_access_key();
    return [$key ? $url . '?token=' . $key : $url];
}

/** Checks a signed register/unregister request; returns the decoded payload. */
function slip_verify(array $req, string $op): object
{
    $payload = $req['payload'] ?? null;
    $sig = $req['sig'] ?? null;
    if (!is_string($payload) || !is_string($sig) || !preg_match('/^[0-9a-f]{128}$/', $sig)) {
        throw new InvalidArgumentException('missing payload or signature');
    }
    $p = json_decode($payload);
    if (!is_object($p) || !isset($p->endpoint, $p->time, $p->op) || !preg_match('/^[0-9a-f]{64}$/', (string)$p->endpoint)) {
        throw new InvalidArgumentException('bad payload');
    }
    if ($p->op !== $op) {
        throw new InvalidArgumentException('op mismatch');
    }
    if (!sodium_crypto_sign_verify_detached(hex2bin($sig), SLIP_SIGNING_CONTEXT . $payload, hex2bin($p->endpoint))) {
        throw new DomainException('signature does not match');
    }
    if (abs((int)$p->time - time()) > SLIP_MAX_SKEW) {
        throw new DomainException("request expired — check this computer's clock");
    }
    if (isset($p->addr) && (!is_object($p->addr) || ($p->addr->id ?? '') !== $p->endpoint)) {
        throw new DomainException('address belongs to a different key');
    }
    return $p;
}

/** Starts a PHP session, falling back to our data folder for storage if
 *  the host's session directory isn't usable. */
function slip_session_start(string $name): void
{
    $path = session_save_path();
    $dir = $path !== '' ? (strpos($path, ';') !== false ? substr($path, strrpos($path, ';') + 1) : $path) : sys_get_temp_dir();
    if (!is_dir($dir) || !is_writable($dir)) {
        $own = slip_data_dir() . '/sessions';
        if (!is_dir($own)) {
            @mkdir($own, 0700, true);
        }
        session_save_path($own);
    }
    session_name($name);
    session_set_cookie_params(['httponly' => true, 'samesite' => 'Lax', 'secure' => slip_scheme() === 'https']);
    session_start();
}

function h($s): string
{
    return htmlspecialchars((string)$s, ENT_QUOTES);
}

/** One filename for the HTTP attachment and the browser's download hint. */
function slip_download_name(string $os): string
{
    $org = preg_replace('/[\x00-\x1F<>:"\/\\\\|?*]+/u', ' ', (string)(slip_station()['org'] ?? ''));
    $org = trim(preg_replace('/\s+/u', ' ', $org), " .");
    $base = 'Slipstream Remote' . ($org !== '' && strcasecmp($org, 'Slipstream Remote') !== 0 ? " - $org" : '');
    return match ($os) {
        'windows' => "$base.exe",
        'macos' => "$base (macOS).zip",
        'android' => "$base.apk",
        default => $base,
    };
}

/** Client builds the station can hand out, keyed by platform. */
function slip_client_builds(): array
{
    $dir = realpath(SLIP_ROOT) . '/clients';
    $map = [
        'windows' => ['file' => 'slipstream-windows-x86_64.exe', 'label' => 'Windows', 'note' => 'Windows 10 and 11'],
        'macos' => ['file' => 'slipstream-macos.zip', 'label' => 'macOS', 'note' => 'macOS 12 or newer'],
        'linux' => ['file' => 'slipstream-linux-x86_64', 'label' => 'Linux', 'note' => '64-bit, X11 or Wayland'],
        'android' => ['file' => 'slipstream-android.apk', 'label' => 'Android', 'note' => 'Android 8 or newer · controls computers (preview)'],
    ];
    $manifest = slip_client_manifest()['platforms'] ?? [];
    foreach ($map as $os => &$b) {
        $path = "$dir/{$b['file']}";
        $b['path'] = is_file($path) ? $path : null;
        $b['size'] = $b['path'] ? filesize($path) : 0;
        // Signed apps are served untouched (see download.php). An APK is
        // always signed.
        $b['signed'] = $os === 'android';
        foreach ($manifest as $entry) {
            if (($entry['file'] ?? '') === $b['file'] && !empty($entry['signed'])) {
                $b['signed'] = true;
            }
        }
    }
    return $map;
}

/*
 * Setup codes link an app to this station without changing the app (signed
 * apps must stay byte-for-byte as released). A code is derived from the
 * access key and the day, so there is nothing to store, every code lasts
 * SLIP_CODE_DAYS, and renewing the key cancels them all. See
 * crates/slipstream-core/src/provision.rs.
 */
const SLIP_CODE_ALPHABET = 'ABCDEFGHJKMNPQRSTUVWXYZ23456789';
const SLIP_CODE_DAYS = 30;

function slip_link_code(int $daysAgo = 0): string
{
    $day = intdiv(time(), 86400) - $daysAgo;
    $h = hash_hmac('sha256', "slipstream link $day", (string)(slip_station()['access_key'] ?? ''), true);
    $code = '';
    for ($i = 0; $i < 8; $i++) {
        $code .= SLIP_CODE_ALPHABET[ord($h[$i]) % strlen(SLIP_CODE_ALPHABET)];
    }
    return $code;
}

function slip_link_ok(string $code): bool
{
    $code = strtoupper((string)preg_replace('/[^A-Za-z0-9]/', '', $code));
    $ok = false;
    for ($d = 0; $d <= SLIP_CODE_DAYS; $d++) {
        $ok = hash_equals(slip_link_code($d), $code) || $ok;
    }
    return $ok && strlen($code) === 8;
}

/** "K7MQ2XPA" → "K7MQ-2XPA" */
function slip_code_display(string $code): string
{
    return substr($code, 0, 4) . '-' . substr($code, 4);
}

/** The download page's one-click link for the app. */
function slip_link_url(): string
{
    return 'slipstream://link?s=' . rawurlencode(slip_base_url()) . '&c=' . slip_link_code();
}

/** The update manifest written by scripts/build-station.sh, if any. */
function slip_client_manifest(): ?array
{
    $m = json_decode((string)@file_get_contents(realpath(SLIP_ROOT) . '/clients/manifest.json'), true);
    return is_array($m) && isset($m['version'], $m['build'], $m['platforms']) ? $m : null;
}

/** Station config embedded into downloaded clients. */
function slip_client_config(): array
{
    $s = slip_station();
    return array_filter([
        'server' => slip_server_address(),
        'key' => slip_access_key(),
        'org' => ($s['org'] ?? '') !== '' ? $s['org'] : null,
    ], fn($v) => $v !== null);
}

/** The trailer appended to executables (see crates/slipstream-core/src/provision.rs). */
function slip_trailer(): string
{
    $json = json_encode(slip_client_config(), JSON_UNESCAPED_SLASHES);
    return $json . pack('V', strlen($json)) . 'SLIPCFG1';
}

/** Shared page chrome (LCARS-style frame). */
function slip_page_start(string $title, string $root = ''): void
{
    $org = slip_station()['org'] ?? '';
    echo '<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">';
    echo '<title>' . h($title) . '</title><link rel="stylesheet" href="' . h($root) . 'assets/station.css">';
    echo '<link rel="icon" type="image/svg+xml" href="' . h($root) . 'assets/logo.svg">';
    echo '<body><div class="frame"><header><div class="elbow"></div><div class="seg s1"></div><div class="seg s2"></div>'
        . '<img class="logo" src="' . h($root) . 'assets/logo.svg" alt=""><h1>' . h($org !== '' ? $org : 'Slipstream Remote') . '</h1></header>';
    echo '<main>';
}

function slip_page_end(): void
{
    echo '</main><footer><div class="elbow"></div><div class="seg"></div><span>Powered by Slipstream Remote · open source</span></footer></div></body></html>';
}

require_once __DIR__ . '/update.php';
