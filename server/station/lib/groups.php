<?php
/** Multiple workstation memberships, with single-group projections for older apps. */
function slip_device_groups(array $device): array
{
    $values = is_array($device['groups'] ?? null) ? $device['groups'] : [$device['group'] ?? ''];
    return array_values(array_unique(array_filter($values, fn($g) => is_string($g) && $g !== '')));
}

function slip_device_in_group(array $device, string $group): bool
{
    $groups = slip_device_groups($device);
    return $group === '' ? !$groups : in_array($group, $groups, true);
}

function slip_clean_device_groups($groups): array
{
    if (!is_array($groups) || count($groups) > 32) throw new InvalidArgumentException('Choose at most 32 groups for a workstation.');
    $out = [];
    foreach ($groups as $group) {
        $group = slip_group_name($group);
        if (preg_match('/[\x00-\x1f\x7f]/u', $group)) throw new InvalidArgumentException('Group names must be single-line text.');
        if (!in_array($group, $out, true)) $out[] = $group;
    }
    return $out;
}

function slip_set_device_groups(array $device, array $groups): array
{
    $device['groups'] = array_values(array_unique($groups));
    $device['group'] = $device['groups'][0] ?? '';
    return $device;
}

/** Old clients may edit a row from any existing group without losing the others.
 * An explicit move to a new legacy group retains its original single-folder meaning. */
function slip_merge_device_groups(array $device, ?array $old): array
{
    if (array_key_exists('groups', $device)) return slip_set_device_groups($device, $device['groups']);
    $previous = $old ? slip_device_groups($old) : [];
    if (!array_key_exists('group', $device) || in_array($device['group'], $previous, true)) {
        $groups = $previous;
    } else {
        $groups = $device['group'] === '' ? [] : [$device['group']];
    }
    $device['group'] = $groups[0] ?? '';
    return array_key_exists('groups', $old ?? []) ? slip_set_device_groups($device, $groups) : $device;
}

/** A compatible row for each visible folder; never disclose other memberships. */
function slip_client_devices(array $devices): array
{
    $out = [];
    foreach ($devices as $device) {
        foreach (slip_device_groups($device) ?: [''] as $group) {
            $copy = $device;
            unset($copy['groups']);
            $copy['group'] = $group;
            $out[] = $copy;
        }
    }
    return $out;
}

/** Replace one group's membership, preserving other groups and personal vaults. */
function slip_set_group_members($owner, $group, $ids): void
{
    if (!is_string($owner) || !is_string($group) || $group === '') throw new InvalidArgumentException('Choose a named group. Ungrouped is automatic.');
    if (!is_array($ids) || count($ids) > SLIP_MAX_DEVICES) throw new InvalidArgumentException('Choose valid workstations.');
    $selected = [];
    foreach ($ids as $id) {
        if ((!is_string($id) && !is_int($id)) || !preg_match('/^[1-9][0-9]{8}$/D', (string)$id)) throw new InvalidArgumentException('Choose valid workstations.');
        $selected[(int)$id] = true;
    }
    $group = slip_group_name($group);
    if ($owner !== '' && (!slip_user_valid($owner) || !slip_user_load($owner))) throw new InvalidArgumentException('The group owner no longer exists.');
    slip_shared_state_update(function ($state) use ($owner, $group, $selected) {
        $catalog = slip_group_catalog($state, $owner === '' ? [] : [$owner => slip_user_load($owner)]);
        if (!array_filter($catalog, fn($g) => $g['owner'] === $owner && $g['group'] === $group)) throw new InvalidArgumentException('That group no longer exists.');
        $update = function ($devices) use ($state, $group, $selected, $owner) {
            $current = array_column($devices, null, 'id');
            $available = $current + array_column($state['devices'], null, 'id');
            if (array_diff_key($selected, $available)) throw new InvalidArgumentException('A selected workstation is no longer available.');
            foreach ($selected as $id => $_) {
                if (!isset($current[$id])) {
                    if ($owner === '') throw new InvalidArgumentException('A selected workstation is no longer shared.');
                    $copy = slip_clean_device($available[$id], false);
                    $copy = slip_set_device_groups($copy, []);
                    $current[$id] = $copy + ['secret' => null, 'added' => time()];
                }
            }
            if (count($current) > SLIP_MAX_DEVICES) throw new InvalidArgumentException('This technician’s device list is full.');
            foreach ($current as $id => &$device) {
                $groups = slip_device_groups($device);
                if (isset($selected[$id])) {
                    if (!in_array($group, $groups, true)) $groups[] = $group;
                } else {
                    $groups = array_values(array_filter($groups, fn($g) => $g !== $group));
                }
                if (count($groups) > 32) throw new InvalidArgumentException('Choose at most 32 groups for a workstation.');
                if ($groups !== slip_device_groups($device)) $device = slip_set_device_groups($device, $groups);
            }
            unset($device);
            return array_values($current);
        };
        if ($owner === '') {
            $state['groups'] = slip_shared_groups($state);
            $state['devices'] = $update($state['devices']);
        } else {
            $changed = slip_user_update($owner, function ($account) use ($update, $group) {
                $account['groups'] = array_values(array_unique(array_merge($account['groups'] ?? [], [$group])));
                if (count($account['groups']) > 100) throw new InvalidArgumentException('This technician’s group list is full.');
                $account['devices'] = $update($account['devices'] ?? []);
                return $account;
            });
            if ($changed === null) throw new InvalidArgumentException('The group owner no longer exists.');
        }
        return $state;
    });
}
