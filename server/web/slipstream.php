<?php
/**
 * Slipstream Remote — directory server library (PHP edition).
 *
 * Speaks the same JSON protocol as the `slipstream-server` binary
 * (see crates/slipstream-core/src/directory.rs). Storage is plain files by
 * default (no database needed); set 'db' in config.php to use MariaDB/MySQL
 * or SQLite instead.
 *
 * MIT License — https://github.com/slipstream-remote/slipstream
 */

if (basename($_SERVER['SCRIPT_FILENAME'] ?? '') === basename(__FILE__)) {
    http_response_code(404);
    exit;
}

const SLIP_VERSION = 2;                 // directory protocol version
const SLIP_SIGNING_CONTEXT = "slipstream-directory-v2\n";
const SLIP_ONLINE_WINDOW = 75;          // seconds without heartbeat => offline
const SLIP_MAX_SKEW = 600;              // signed request freshness (seconds)
const SLIP_ID_MIN = 100000000;
const SLIP_ID_MAX = 999999999;

function slip_config(): array
{
    static $config = null;
    if ($config !== null) {
        return $config;
    }
    $defaults = [
        'data_dir' => slip_default_data_dir(),
        'db' => null,                   // e.g. ['dsn' => 'mysql:host=localhost;dbname=slipstream', 'user' => '…', 'pass' => '…']
        'relays' => 'auto',             // 'auto', or a list like ['https://relay.example.com/']
        'motd' => null,
        'lookups_per_minute' => 60,
        'registers_per_minute' => 30,
    ];
    $file = __DIR__ . '/config.php';
    $user = is_file($file) ? (include $file) : [];
    $config = array_merge($defaults, is_array($user) ? $user : []);
    return $config;
}

/**
 * Prefer a folder *outside* the web root (e.g. /home/site/slipstream-data
 * next to public_html) so device records can never be downloaded; fall back
 * to ./data protected by .htaccess.
 */
function slip_default_data_dir(): string
{
    $docRoot = realpath($_SERVER['DOCUMENT_ROOT'] ?? '');
    if ($docRoot) {
        $outside = dirname($docRoot) . '/slipstream-data';
        if (is_dir($outside) || (is_writable(dirname($docRoot)) && @mkdir($outside, 0750))) {
            return $outside;
        }
    }
    return __DIR__ . '/data';
}

function slip_data_dir(): string
{
    $dir = rtrim(slip_config()['data_dir'], '/');
    if (!is_dir($dir)) {
        @mkdir($dir, 0750, true);
    }
    // Keep the data folder private on Apache even if it lives in the web root.
    if (is_dir($dir) && !is_file("$dir/.htaccess")) {
        @file_put_contents("$dir/.htaccess", "<IfModule mod_authz_core.c>\n  Require all denied\n</IfModule>\n<IfModule !mod_authz_core.c>\n  Deny from all\n</IfModule>\n");
        @file_put_contents("$dir/index.html", '');
    }
    return $dir;
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
        file_put_contents($tmp, json_encode($d));
        rename($tmp, $path);
    }

    public function register(string $endpoint, array $info, string $addrJson): int
    {
        $d = $this->load($endpoint);
        if ($d === null) {
            // Claim a free ID atomically: creating ids/<id> with 'x' fails if taken.
            for ($i = 0; $i < 50; $i++) {
                $id = random_int(SLIP_ID_MIN, SLIP_ID_MAX);
                $fh = @fopen("{$this->dir}/ids/$id", 'x');
                if ($fh) {
                    fwrite($fh, $endpoint);
                    fclose($fh);
                    break;
                }
                $id = null;
            }
            if (!$id) {
                throw new RuntimeException('could not allocate an ID');
            }
            $d = ['id' => $id, 'created' => time()];
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

    public function lookup(int $id): array
    {
        $endpoint = @file_get_contents("{$this->dir}/ids/$id");
        $d = ($endpoint !== false && preg_match('/^[0-9a-f]{64}$/', $endpoint)) ? $this->load($endpoint) : null;
        if ($d === null) {
            return ['status' => 'not_found'];
        }
        $info = ['name' => $d['name'] ?? '', 'os' => $d['os'] ?? '', 'app_version' => $d['app_version'] ?? ''];
        if (!empty($d['addr']) && time() - ($d['last_seen'] ?? 0) <= SLIP_ONLINE_WINDOW) {
            return ['status' => 'online', 'addr' => $d['addr'], 'info' => $info];
        }
        return ['status' => 'offline', 'info' => $info];
    }

    public function counts(): array
    {
        $total = 0;
        $online = 0;
        $cut = time() - SLIP_ONLINE_WINDOW;
        foreach (glob("{$this->dir}/devices/*.json") ?: [] as $f) {
            $total++;
            if (filemtime($f) >= $cut) {
                $d = json_decode((string)@file_get_contents($f), true);
                if (($d['last_seen'] ?? 0) >= $cut) {
                    $online++;
                }
            }
        }
        return [$total, $online];
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
                // ID (or endpoint, in a race) already taken — retry/reload.
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

    public function counts(): array
    {
        $total = (int)$this->db->query('SELECT COUNT(*) FROM slipstream_devices')->fetchColumn();
        $q = $this->db->prepare('SELECT COUNT(*) FROM slipstream_devices WHERE last_seen >= ?');
        $q->execute([time() - SLIP_ONLINE_WINDOW]);
        return [$total, (int)$q->fetchColumn()];
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
    [$start, $count] = array_map('intval', explode(' ', trim((string)stream_get_contents($fh)) ?: '0 0') + [0, 0]);
    if (time() - $start >= 60) {
        [$start, $count] = [time(), 0];
    }
    $count++;
    ftruncate($fh, 0);
    rewind($fh);
    fwrite($fh, "$start $count");
    flock($fh, LOCK_UN);
    fclose($fh);
    // Occasionally sweep stale buckets.
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

/** This folder's public URL, e.g. https://relay.example.com/ */
function slip_base_url(): string
{
    $path = rtrim(str_replace('\\', '/', dirname($_SERVER['SCRIPT_NAME'] ?? '/')), '/');
    if (substr($path, -4) === '/api') {
        $path = substr($path, 0, -4);
    }
    return slip_scheme() . '://' . slip_host() . $path . '/';
}

/**
 * Relays to advertise. In 'auto' mode we check whether a Slipstream relay
 * answers at this domain's /ping (i.e. the binary is proxied here) and
 * advertise it if so; otherwise clients use the public relays.
 */
function slip_relays(): array
{
    $cfg = slip_config()['relays'];
    if (is_array($cfg)) {
        return array_values($cfg);
    }
    $root = slip_scheme() . '://' . slip_host() . '/';
    $cache = slip_data_dir() . '/relay-probe.json';
    $c = json_decode((string)@file_get_contents($cache), true);
    if (is_array($c) && ($c['root'] ?? '') === $root && time() - ($c['at'] ?? 0) < 600) {
        return $c['ok'] ? [$root] : [];
    }
    $ok = slip_probe_relay($root);
    @file_put_contents($cache, json_encode(['root' => $root, 'ok' => $ok, 'at' => time()]));
    return $ok ? [$root] : [];
}

function slip_probe_relay(string $root): bool
{
    $url = $root . 'ping';
    if (function_exists('curl_init')) {
        $ch = curl_init($url);
        curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 2, CURLOPT_CONNECTTIMEOUT => 2, CURLOPT_NOBODY => false]);
        curl_exec($ch);
        $code = (int)curl_getinfo($ch, CURLINFO_RESPONSE_CODE);
        return $code === 200;
    }
    $ctx = stream_context_create(['http' => ['timeout' => 2, 'ignore_errors' => true]]);
    @file_get_contents($url, false, $ctx);
    return isset($http_response_header[0]) && strpos($http_response_header[0], ' 200') !== false;
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
