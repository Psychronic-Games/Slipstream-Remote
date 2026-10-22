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

    // Use a database instead of files (for very large installs).
    // 'db' => [
    //     'dsn'  => 'mysql:host=localhost;dbname=slipstream;charset=utf8mb4',
    //     'user' => 'slipstream',
    //     'pass' => 'change-me',
    // ],
];
