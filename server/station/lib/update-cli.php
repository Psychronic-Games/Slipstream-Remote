<?php
/**
 * Slipstream Station — installs updates in the background on hosts without
 * PHP-FPM (started by slip_update_background() in update.php). Runs only from
 * PHP's command line; lib/ is closed to web visitors anyway.
 *
 * MIT License.
 */
if (PHP_SAPI !== 'cli') {
    http_response_code(404);
    exit;
}
require __DIR__ . '/slipstream.php';
echo date('c'), " checking for station updates\n";
slip_update_run();
$state = slip_update_state();
echo date('c'), ' ', $state['error'] ?? (isset($state['installed']) ? 'installed build ' . $state['installed']['build'] : 'done'), "\n";
