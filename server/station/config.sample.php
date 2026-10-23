<?php
/**
 * Optional station overrides. Everything works without this file — settings
 * like the organisation name live on the admin page. Copy to config.php to
 * change where data is kept or to use a database.
 */
return [
    // Where device records and station settings are kept. By default a
    // "slipstream-data" folder next to the web root, or ./data if that
    // isn't writable.
    // 'data_dir' => '/home/example/slipstream-data',

    // Optional relay capacity controls; 0 keeps the self-hosted default unlimited.
    // 'relay_bytes_per_second' => 5242880,  // 5 MiB/s per connection
    // 'relay_max_connections' => 256,

    // Station updates come from the project's GitHub releases, verified with
    // the release keys in lib/release-keys.txt. Point them at a fork or a
    // mirror of the releases API, or trust an extra key (hex, comma separated).
    // Automatic installs are switched on or off on the admin page.
    // 'update_repo' => 'Psychronic-Games/Slipstream-Remote',
    // 'update_releases_url' => 'https://mirror.example.com/releases/latest.json',
    // 'release_keys' => '',
    // PHP's command-line program, for background installs without PHP-FPM
    // (found by itself in the usual places).
    // 'php_cli' => '/usr/bin/php8.4',

    // Use a database instead of files (for very large installs).
    // 'db' => [
    //     'dsn'  => 'mysql:host=localhost;dbname=slipstream;charset=utf8mb4',
    //     'user' => 'slipstream',
    //     'pass' => 'change-me',
    // ],
];
