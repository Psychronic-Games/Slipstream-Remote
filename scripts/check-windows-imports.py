#!/usr/bin/env python3
"""Checks that a Windows build of Slipstream can start on Windows 7 SP1.

  python3 scripts/check-windows-imports.py slipstream.exe YY_Thunks_for_Win7.obj

Windows refuses to start a program that imports a function the system
doesn't have. The build links YY-Thunks (packaging/windows/yy-thunks.txt),
which answers the imports Windows 7/8 lack. This fails if any import that
YY-Thunks knows is newer than Windows 7 is still imported directly (the
object wasn't linked), if the executable imports API-set DLLs, or if it
imports one of the newer functions listed below that YY-Thunks doesn't
cover (code must look those up at run time instead). Needs llvm-readobj and
llvm-nm (LLVM; preinstalled on GitHub's Windows runners).
"""
import re
import shutil
import subprocess
import sys

# Newer than Windows 7 and not answered by YY-Thunks: look them up with
# GetProcAddress instead (as desktop_shell.rs does).
NOT_THUNKED = {
    'SetThreadDpiAwarenessContext', 'GetThreadDpiAwarenessContext', 'SetProcessDpiAwarenessContext',
    'GetDpiForSystem', 'GetSystemMetricsForDpi', 'AdjustWindowRectExForDpi',
}


def tool(name):
    found = shutil.which(name) or shutil.which(name + '.exe')
    if not found:
        raise SystemExit(f'needs {name} (LLVM)')
    return found


def imports(exe):
    out = subprocess.run([tool('llvm-readobj'), '--coff-imports', exe], check=True, capture_output=True, text=True).stdout
    dll, found = None, []
    for line in out.splitlines():
        if m := re.search(r'Name: (\S+)', line):
            dll = m.group(1)
        elif (m := re.search(r'Symbol: (\S+)', line)) and dll:
            found.append((dll, m.group(1)))
    return found


def thunked(obj):
    out = subprocess.run([tool('llvm-nm'), '--defined-only', obj], check=True, capture_output=True, text=True).stdout
    return {m.group(1) for m in re.finditer(r' __imp_(\w+)$', out, re.M)}


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    exe, obj = sys.argv[1:]
    newer = thunked(obj)
    if len(newer) < 50:
        raise SystemExit(f'{obj} does not look like a YY-Thunks object')
    found = imports(exe)
    problems = sorted({f'{dll}!{name}' for dll, name in found
                       if name in newer or name in NOT_THUNKED or dll.lower().startswith('api-ms-win-')})
    if problems:
        print(f'{exe} may not start on Windows 7: these imports bypass YY-Thunks (is it linked?)', file=sys.stderr)
        for p in problems:
            print(f'  {p}', file=sys.stderr)
        raise SystemExit(1)
    print(f'Windows 7 imports: OK ({len(found)} imports checked, {len(newer)} newer functions answered by YY-Thunks)')


if __name__ == '__main__':
    main()
