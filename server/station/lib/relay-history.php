<?php
/** Bounded relay history reads and a streaming, oldest-first text export. */
if (!defined('SLIP_ROOT')) { http_response_code(404); exit; }

const SLIP_LOG_PAGE_BYTES = 65536;

function slip_log_plain(string $text): string
{
    $text = preg_replace('/\x1b\[[0-?]*[ -\/]*[@-~]/', '', $text) ?? '';
    return preg_replace('/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/', '', $text) ?? '';
}

/** Pages cover contiguous byte ranges, aligning to complete lines when possible.
 * Exceptionally long lines continue on another page; the export is unbroken. */
function slip_relay_log_page(string $mode = 'newest', ?int $at = null, ?string $source = null): array
{
    if (!in_array($mode, ['oldest', 'older', 'newer', 'newest'], true) || ($at !== null && $at < 0)) {
        throw new InvalidArgumentException('Choose a valid log page.');
    }
    $stream = @fopen(slip_data_dir() . '/relay.log', 'rb');
    if (!$stream) {
        if (in_array($mode, ['older', 'newer'], true)) throw new DomainException('The log changed. Choose Newest or Oldest to reload its history.');
        return ['text' => '', 'start' => 0, 'end' => 0, 'size' => 0, 'source' => '', 'continued' => false, 'available' => false];
    }
    try {
        $stat = fstat($stream);
        $size = (int)$stat['size'];
        $identity = hash('sha256', $stat['dev'] . ':' . $stat['ino']);
        if (in_array($mode, ['older', 'newer'], true)
            && ($at === null || $at > $size || $source !== $identity)) {
            throw new DomainException('The log changed. Choose Newest or Oldest to reload its history.');
        }
        $backward = in_array($mode, ['newest', 'older'], true);
        $start = $backward ? max(0, ($mode === 'newest' ? $size : $at) - SLIP_LOG_PAGE_BYTES) : ($mode === 'oldest' ? 0 : $at);
        $end = $backward ? ($mode === 'newest' ? $size : $at) : min($size, $start + SLIP_LOG_PAGE_BYTES);
        fseek($stream, $start);
        $text = '';
        while (strlen($text) < $end - $start && !feof($stream)) {
            $part = fread($stream, $end - $start - strlen($text));
            if ($part === false || $part === '') break;
            $text .= $part;
        }
        $end = $start + strlen($text);
        if ($backward && $start > 0) {
            fseek($stream, $start - 1);
            if (fread($stream, 1) !== "\n") {
                $newline = strpos($text, "\n");
                if ($newline !== false && $newline + 1 < strlen($text)) {
                    $start += $newline + 1;
                    $text = substr($text, $newline + 1);
                }
            }
        } elseif (!$backward && $end < $size) {
            $newline = strrpos($text, "\n");
            if ($newline !== false) {
                $text = substr($text, 0, $newline + 1);
                $end = $start + strlen($text);
            }
        }
        $continued = $end > $start && !str_ends_with($text, "\n");
        if ($start > 0) {
            fseek($stream, $start - 1);
            $continued = $continued || fread($stream, 1) !== "\n";
        }
        return ['text' => slip_log_plain($text), 'start' => $start, 'end' => $end, 'size' => $size,
            'source' => $identity, 'continued' => $continued, 'available' => true];
    } finally {
        fclose($stream);
    }
}

/** Stream only the bytes present when the download begins; appends never
 * prolong a download. Carry split ANSI sequences across bounded chunks. */
function slip_relay_log_export(): void
{
    $stream = @fopen(slip_data_dir() . '/relay.log', 'rb');
    if (!$stream) throw new RuntimeException('No relay log has been recorded yet.');
    try {
        $remaining = (int)fstat($stream)['size'];
        header('Content-Type: text/plain; charset=utf-8');
        header('Content-Disposition: attachment; filename="slipstream-relay-' . gmdate('Y-m-d-His') . '.txt"');
        header('X-Content-Type-Options: nosniff');
        header('Cache-Control: no-store');
        $carry = '';
        while ($remaining > 0 && !feof($stream)) {
            $chunk = fread($stream, min(65536, $remaining));
            if ($chunk === false || $chunk === '') break;
            $remaining -= strlen($chunk);
            $text = $carry . $chunk;
            $carry = '';
            $escape = strrpos($text, "\x1b");
            if ($escape !== false && strlen($text) - $escape <= 256
                && preg_match('/^\x1b(?:\[[0-?]*[ -\/]*)?$/D', substr($text, $escape))) {
                $carry = substr($text, $escape);
                $text = substr($text, 0, $escape);
            }
            echo slip_log_plain($text);
        }
        if ($carry !== '') echo slip_log_plain($carry);
    } finally {
        fclose($stream);
    }
}
