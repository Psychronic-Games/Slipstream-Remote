<?php
/**
 * Slipstream directory configuration. Optional — everything works without
 * it. Copy to config.php and uncomment what you need.
 */
return [
    // Where device records are kept. The default "data" folder next to this
    // file is protected by an .htaccess file on Apache. On Nginx, prefer a
    // folder outside the web root:
    // 'data_dir' => '/home/example/slipstream-data',

    // Use a database instead of files (recommended above ~10,000 devices).
    // 'db' => [
    //     'dsn'  => 'mysql:host=localhost;dbname=slipstream;charset=utf8mb4',
    //     'user' => 'slipstream',
    //     'pass' => 'change-me',
    // ],
    // 'db' => ['dsn' => 'sqlite:/home/example/slipstream.sqlite'],

    // Relays to tell clients about. 'auto' advertises a relay running at this
    // domain's /relay (see the status page); otherwise clients fall back to
    // the free public relays. Or list your own:
    // 'relays' => ['https://relay.example.com/', 'https://relay-eu.example.com/'],
    'relays' => 'auto',

    // Message shown on every client's Bridge page.
    // 'motd' => 'Welcome to the Example Corp support server',
];
