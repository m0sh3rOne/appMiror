#![cfg(windows)]

use std::sync::Mutex;
use windows::core::w;
use windows::Win32::Foundation::GENERIC_ALL;
use windows::Win32::System::StationsAndDesktops::{
    CreateDesktopW, GetThreadDesktop, HDESK, OpenDesktopW, SetThreadDesktop, DESKTOP_CONTROL_FLAGS,
};
use windows::Win32::System::Threading::GetCurrentThreadId;

static VIRTUAL_DESKTOP: Mutex<Option<isize>> = Mutex::new(None);

pub fn get_or_create_virtual_desktop() -> Option<isize> {
    let mut lock = VIRTUAL_DESKTOP.lock().unwrap();
    if let Some(h) = *lock {
        return Some(h);
    }
    
    unsafe {
        let desktop_name = w!("appMirror_Desktop");
        // Tente d'ouvrir le bureau virtuel s'il existe déjà
        let mut hdesk = OpenDesktopW(
            desktop_name,
            DESKTOP_CONTROL_FLAGS(0),
            false,
            GENERIC_ALL.0,
        ).ok();
        
        if hdesk.is_none() {
            // Sinon on le crée
            hdesk = CreateDesktopW(
                desktop_name,
                None,
                None,
                DESKTOP_CONTROL_FLAGS(0),
                GENERIC_ALL.0,
                None,
            ).ok();
        }
        
        if let Some(h) = hdesk {
            *lock = Some(h.0 as isize);
            Some(h.0 as isize)
        } else {
            None
        }
    }
}

pub fn run_on_desktop<F, R>(hdesk_val: isize, f: F) -> R
where
    F: FnOnce() -> R,
{
    unsafe {
        let cur_tid = GetCurrentThreadId();
        let old_desk = GetThreadDesktop(cur_tid).ok();
        let hdesk = HDESK(hdesk_val as *mut core::ffi::c_void);
        
        let attached = if let Some(old) = old_desk {
            if !hdesk.is_invalid() && hdesk != old {
                SetThreadDesktop(hdesk).is_ok()
            } else {
                false
            }
        } else {
            false
        };
        
        let result = f();
        
        if attached {
            if let Some(old) = old_desk {
                let _ = SetThreadDesktop(old);
            }
        }
        result
    }
}

pub fn spawn_on_desktop(command: &str, arguments: &[String], desktop_name: &str) -> Result<u32, String> {
    use windows::Win32::System::Threading::{
        CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW, STARTF_USESHOWWINDOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    use windows::core::{HSTRING, PWSTR};
    
    unsafe {
        let cmd_line = if arguments.is_empty() {
            command.to_string()
        } else {
            let escaped_args: Vec<String> = arguments
                .iter()
                .map(|arg| {
                    if arg.contains(' ') || arg.contains('\t') || arg.is_empty() {
                        format!("\"{}\"", arg.replace('\"', "\\\""))
                    } else {
                        arg.clone()
                    }
                })
                .collect();
            format!("{} {}", command, escaped_args.join(" "))
        };
        
        let mut cmd_utf16 = cmd_line.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let desktop_utf16 = HSTRING::from(desktop_name);
        
        let mut si = STARTUPINFOW::default();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.lpDesktop = PWSTR(desktop_utf16.as_ptr() as *mut u16);
        si.dwFlags = STARTF_USESHOWWINDOW;
        si.wShowWindow = SW_SHOW.0 as u16;
        
        let mut pi = PROCESS_INFORMATION::default();
        
        let success = CreateProcessW(
            None,
            PWSTR(cmd_utf16.as_mut_ptr()),
            None,
            None,
            false,
            windows::Win32::System::Threading::PROCESS_CREATION_FLAGS(0x08000000),
            None,
            None,
            &si,
            &mut pi,
        );
        
        if success.is_ok() {
            let _ = windows::Win32::Foundation::CloseHandle(pi.hProcess);
            let _ = windows::Win32::Foundation::CloseHandle(pi.hThread);
            Ok(pi.dwProcessId)
        } else {
            let err = success.err().unwrap();
            Err(format!("CreateProcessW failed with error {:?}", err))
        }
    }
}

pub fn get_process_image_path(hwnd: isize) -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT};
    use windows::Win32::Foundation::{CloseHandle, HWND};

    unsafe {
        let mut pid = 0;
        let hwnd_raw = HWND(hwnd as *mut core::ffi::c_void);
        GetWindowThreadProcessId(hwnd_raw, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut len = 2048u32;
        let mut name = vec![0u16; len as usize];
        let success = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(name.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        if success.is_ok() && len > 0 {
            Some(String::from_utf16_lossy(&name[..len as usize]))
        } else {
            None
        }
    }
}

