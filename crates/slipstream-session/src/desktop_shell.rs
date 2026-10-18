//! Desktop actions run on the host, using the signed-in user's shell when
//! hosted by the Windows service. Drop coordinates freeze a destination;
//! browser targets are restricted to typed IPv4 HTTP/HTTPS addresses.

#[cfg(not(windows))]
pub fn drop_folder(_x: i32, _y: i32) -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::fsops::default_upload_dir())
}

#[cfg(not(windows))]
pub fn folder_under_cursor() -> Option<std::path::PathBuf> {
    None
}

#[cfg(windows)]
pub use native::{drop_folder, open_web};

#[cfg(windows)]
pub fn folder_under_cursor() -> Option<std::path::PathBuf> {
    let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    // Legacy uploads still work, but new viewers resolve once using the
    // actual drop point, then send the whole batch to that frozen folder.
    unsafe {
        if windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point) == 0 {
            return None;
        }
    }
    drop_folder(point.x, point.y).ok()
}

#[cfg(not(windows))]
pub fn open_web(address: std::net::Ipv4Addr, port: u16) -> anyhow::Result<()> {
    let url = slipstream_core::network::web_url(address, port)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use anyhow::Context;
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let mut child = Command::new(opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("starting the host's default browser")?;
        // Some openers stay alive with the browser. Catch immediate errors,
        // then reap in the background rather than waiting for its window to close.
        let deadline = Instant::now() + Duration::from_millis(400);
        loop {
            if let Some(status) = child.try_wait()? {
                anyhow::ensure!(
                    status.success(),
                    "the host's browser launcher failed ({status}); check its default browser"
                );
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = url;
        anyhow::bail!("Opening a browser is unavailable on this host");
    }
}

#[cfg(windows)]
mod native {
    use anyhow::{Context, Result, bail};
    use std::{
        io::Read,
        os::windows::io::{FromRawHandle, OwnedHandle},
        path::PathBuf,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, POINT, SetHandleInformation, WAIT_OBJECT_0,
        },
        Security::SECURITY_ATTRIBUTES,
        System::{
            Com::CoTaskMemFree,
            Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock},
            Pipes::CreatePipe,
            RemoteDesktop::{ProcessIdToSessionId, WTSQueryUserToken},
            Threading::{
                CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
                DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcessId,
                GetExitCodeProcess, InitializeProcThreadAttributeList,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
                STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
            },
        },
        UI::{
            Shell::{FOLDERID_Desktop, SHGetKnownFolderPath},
            WindowsAndMessaging::{GA_ROOT, GetAncestor, GetClassNameW, WindowFromPoint},
        },
    };

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain([0]).collect()
    }
    fn owned(handle: HANDLE) -> OwnedHandle {
        // SAFETY: caller transfers a newly created, non-null handle.
        unsafe { OwnedHandle::from_raw_handle(handle.cast()) }
    }
    fn raw(handle: &OwnedHandle) -> HANDLE {
        use std::os::windows::io::AsRawHandle;
        handle.as_raw_handle().cast()
    }

    fn interactive_user() -> Result<Option<OwnedHandle>> {
        Ok(if slipstream_platform::windesk::following() {
            unsafe {
                let mut session = 0;
                let mut token = std::ptr::null_mut();
                if ProcessIdToSessionId(GetCurrentProcessId(), &mut session) == 0
                    || WTSQueryUserToken(session, &mut token) == 0
                {
                    return Err(std::io::Error::last_os_error())
                        .context("no signed-in user in the controlled session");
                }
                Some(owned(token))
            }
        } else {
            None
        })
    }

    pub fn open_web(address: std::net::Ipv4Addr, port: u16) -> Result<()> {
        let url = slipstream_core::network::web_url(address, port)?;
        if let Some(user) = interactive_user()? {
            // The URL consists only of a scheme, numeric IPv4 and a slash.
            // ShellExecute under the user's token selects that user's browser.
            let script = format!(
                "$ErrorActionPreference='Stop';$p=New-Object System.Diagnostics.ProcessStartInfo;\
                 $p.FileName='{url}';$p.UseShellExecute=$true;$p.Verb='open';\
                 [System.Diagnostics.Process]::Start($p) | Out-Null"
            );
            user_query(&powershell(), &script, &user, false)?;
            return Ok(());
        }
        use windows_sys::Win32::{
            System::Com::{
                COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
            },
            UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
        };
        unsafe {
            let status = CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            );
            if status < 0 {
                bail!(
                    "cannot initialize the host's browser launcher (0x{:08x})",
                    status as u32
                );
            }
            let result = ShellExecuteW(
                std::ptr::null_mut(),
                wide("open").as_ptr(),
                wide(&url).as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            ) as isize;
            CoUninitialize();
            if result <= 32 {
                bail!("the host's default browser could not open the URL (shell error {result})");
            }
        }
        Ok(())
    }

    pub fn drop_folder(x: i32, y: i32) -> Result<PathBuf> {
        use slipstream_platform::windesk;
        use windows_sys::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
            SetThreadDpiAwarenessContext,
        };
        // Viewer coordinates describe capture pixels, even when the host's
        // headless service process otherwise has DPI virtualization enabled.
        struct DpiGuard(DPI_AWARENESS_CONTEXT);
        impl Drop for DpiGuard {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    unsafe {
                        SetThreadDpiAwarenessContext(self.0);
                    }
                }
            }
        }
        let _dpi = DpiGuard(unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE)
        });
        windesk::follow();
        if windesk::following() && !windesk::on_default_desktop() {
            bail!("unlock the remote computer before dropping files");
        }
        // The service host lives in the controlled user's session, as SYSTEM.
        // Its own Desktop is not the user's Desktop, and Shell.Application
        // must run with the user's token to see their Explorer windows.
        let user = interactive_user()?;
        let root = unsafe { GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT) };
        let mut class = [0u16; 64];
        let len =
            unsafe { GetClassNameW(root, class.as_mut_ptr(), class.len() as i32) }.max(0) as usize;
        let class = String::from_utf16_lossy(&class[..len]);
        let path = if class == "CabinetWClass" || class == "ExploreWClass" {
            let script = format!(
                "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false);$ErrorActionPreference='Stop';\
                 $p=(New-Object -ComObject Shell.Application).Windows() | Where-Object {{ $_.HWND -eq {} }} | \
                 ForEach-Object {{ $_.Document.Folder.Self.Path }} | Select-Object -First 1;\
                 if($p -and $p.Length -lt 8000){{[Console]::Write($p)}}",
                root as isize
            );
            let path = shell_query(&script, user.as_ref())?;
            if path.is_empty() {
                bail!(
                    "Explorer does not expose a filesystem folder at that spot; drop onto the Desktop or use Files"
                );
            }
            PathBuf::from(path)
        } else {
            // Known folders include redirection (e.g. OneDrive) and use the
            // signed-in user's token rather than SYSTEM's profile.
            unsafe {
                let mut path = std::ptr::null_mut();
                let status = SHGetKnownFolderPath(
                    &FOLDERID_Desktop,
                    0,
                    user.as_ref().map(raw).unwrap_or(std::ptr::null_mut()),
                    &mut path,
                );
                if status < 0 || path.is_null() {
                    if !path.is_null() {
                        CoTaskMemFree(path.cast());
                    }
                    bail!(
                        "cannot locate the signed-in user's Desktop (0x{:08x})",
                        status as u32
                    );
                }
                let mut len = 0;
                while *path.add(len) != 0 {
                    len += 1;
                }
                let result = PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(
                    path, len,
                )));
                CoTaskMemFree(path.cast());
                result
            }
        };
        if !path.is_dir() {
            bail!("{} is not an accessible folder", path.display());
        }
        Ok(path)
    }

    fn powershell() -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe")
    }

    fn shell_query(script: &str, user: Option<&OwnedHandle>) -> Result<String> {
        let exe = powershell();
        let output = if let Some(user) = user {
            user_query(&exe, script, user, true)?
        } else {
            use std::{
                os::windows::process::CommandExt,
                process::{Command, Stdio},
            };
            let mut child = Command::new(exe)
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?;
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                if let Some(status) = child.try_wait()? {
                    if !status.success() {
                        bail!("Explorer folder query failed");
                    }
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("Explorer folder query timed out");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let mut output = Vec::new();
            if let Some(stdout) = child.stdout.take() {
                stdout.take(64 * 1024).read_to_end(&mut output)?;
            }
            output
        };
        Ok(String::from_utf8(output)
            .context("Explorer returned a non-UTF-8 path")?
            .trim()
            .trim_start_matches('\u{feff}')
            .to_owned())
    }

    fn user_query(
        exe: &std::path::Path,
        script: &str,
        user: &OwnedHandle,
        capture: bool,
    ) -> Result<Vec<u8>> {
        // Bounded output (< 8000 chars) fits the pipe, so waiting before
        // reading cannot deadlock. No result or user path is written to disk.
        unsafe {
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: std::ptr::null_mut(),
                bInheritHandle: 1,
            };
            let (mut read, mut write) = (std::ptr::null_mut(), std::ptr::null_mut());
            if CreatePipe(&mut read, &mut write, &attributes, 64 * 1024) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let (read, write) = (owned(read), owned(write));
            if SetHandleInformation(raw(&read), HANDLE_FLAG_INHERIT, 0) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let input: OwnedHandle = std::fs::File::open("NUL")?.into();
            if SetHandleInformation(raw(&input), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let output: OwnedHandle = std::fs::OpenOptions::new().write(true).open("NUL")?.into();
            if SetHandleInformation(raw(&output), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let stdout = if capture { raw(&write) } else { raw(&output) };
            let mut environment = std::ptr::null_mut();
            if CreateEnvironmentBlock(&mut environment, raw(user), 0) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let application = wide(&exe.to_string_lossy());
            let mut command = wide(&format!(
                "\"{}\" -NoProfile -NonInteractive -Command \"{}\"",
                exe.display(),
                script
            ));
            let desktop = wide("winsta0\\default");
            // Limit inheritance to this output pipe, never other service
            // handles or another simultaneous drop's private pipe.
            let mut bytes = 0;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut bytes);
            let mut attributes_storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
            let list = attributes_storage.as_mut_ptr().cast();
            if InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) == 0 {
                DestroyEnvironmentBlock(environment);
                return Err(std::io::Error::last_os_error().into());
            }
            let handles = [stdout, raw(&input)];
            if UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(&handles),
                std::ptr::null_mut(),
                std::ptr::null(),
            ) == 0
            {
                let error = std::io::Error::last_os_error();
                DeleteProcThreadAttributeList(list);
                DestroyEnvironmentBlock(environment);
                return Err(error.into());
            }
            let mut startup: STARTUPINFOEXW = std::mem::zeroed();
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.lpDesktop = desktop.as_ptr().cast_mut();
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = raw(&input);
            startup.StartupInfo.hStdOutput = stdout;
            startup.StartupInfo.hStdError = stdout;
            startup.lpAttributeList = list;
            let mut process: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessAsUserW(
                raw(user),
                application.as_ptr(),
                command.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                environment,
                std::ptr::null(),
                &startup.StartupInfo,
                &mut process,
            );
            let error = std::io::Error::last_os_error();
            DestroyEnvironmentBlock(environment);
            DeleteProcThreadAttributeList(list);
            if ok == 0 {
                return Err(error).context("starting the user's desktop shell");
            }
            let process_handle = owned(process.hProcess);
            CloseHandle(process.hThread);
            drop(write);
            if WaitForSingleObject(raw(&process_handle), 8000) != WAIT_OBJECT_0 {
                TerminateProcess(raw(&process_handle), 1);
                WaitForSingleObject(raw(&process_handle), 1000);
                bail!("The user desktop shell timed out");
            }
            let mut exit = 0;
            if GetExitCodeProcess(raw(&process_handle), &mut exit) == 0 || exit != 0 {
                bail!("The user desktop shell failed");
            }
            if !capture {
                return Ok(Vec::new());
            }
            let mut output = Vec::new();
            std::fs::File::from(read)
                .take(64 * 1024)
                .read_to_end(&mut output)?;
            Ok(output)
        }
    }
}
