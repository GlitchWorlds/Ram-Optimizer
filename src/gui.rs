use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{
    BOOL, HMODULE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, GetStockObject, SetBkMode,
    SetTextColor, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, HKEY,
};
use windows_sys::Win32::UI::Controls::{
    InitCommonControlsEx, INITCOMMONCONTROLSEX, ICC_PROGRESS_CLASS, PBM_SETPOS, PBM_SETRANGE32,
    PROGRESS_CLASSW,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, KillTimer, LoadCursorW, LoadIconW, PostQuitMessage,
    RegisterClassExW, SendMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW,
    ShowWindow, TrackPopupMenu, TranslateMessage, BM_GETCHECK, BM_SETCHECK,
    CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HMENU, IDC_ARROW, IDI_APPLICATION,
    MF_SEPARATOR, MF_STRING, MSG, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_RESTORE,
    SW_SHOW, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WM_CLOSE, WM_COMMAND,
    WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ENDSESSION,
    WM_LBUTTONDBLCLK, WM_QUERYENDSESSION, WM_RBUTTONUP, WM_SYSCOMMAND, WM_TIMER,
    WM_USER, WNDCLASSEXW, WS_CHILD,
    WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP,
    WS_VISIBLE, WS_CAPTION, WS_MINIMIZEBOX, ES_AUTOHSCROLL, ES_NUMBER, BS_AUTOCHECKBOX,
    BS_DEFPUSHBUTTON, SC_MINIMIZE,
};

use crate::{get_ram_metrics, optimize_memory};
use crate::config::{self, Config};

const ID_TIMER_TICK: usize = 1001;
const WM_TRAYICON: u32 = WM_USER + 201;

const BST_UNCHECKED: usize = 0;
const BST_CHECKED: usize = 1;

const IDC_METER_PROGRESS: i32 = 2001;
const IDC_BTN_OPTIMIZE: i32 = 2002;
const IDC_CHK_INTERVAL: i32 = 2003;
const IDC_EDIT_INTERVAL: i32 = 2004;
const IDC_CHK_THRESHOLD: i32 = 2005;
const IDC_EDIT_THRESHOLD: i32 = 2006;
const IDC_CHK_STARTUP: i32 = 2007;
const IDC_LBL_STATUS: i32 = 2008;

const IDM_TRAY_OPEN: usize = 3001;
const IDM_TRAY_OPTIMIZE: usize = 3002;
const IDM_TRAY_EXIT: usize = 3003;

static IS_OPTIMIZING: AtomicBool = AtomicBool::new(false);
static LAST_OPTIMIZE_SEC: AtomicU64 = AtomicU64::new(0);
static IS_SELF_DESTRUCT: AtomicBool = AtomicBool::new(false);

/// Guards config saves: true once WM_CREATE has finished seeding the controls
/// from disk, so the EN_CHANGE notifications fired while setting edit text
/// during init do not write the config back prematurely.
static UI_READY: AtomicBool = AtomicBool::new(false);

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Loads persisted automation settings (falls back to defaults on failure).
fn load_config() -> Config {
    config::load()
}

/// Persists automation settings; failures are deliberately ignored so a
/// read-only profile never disrupts the GUI.
fn save_config(cfg: &Config) {
    let _ = config::save(cfg);
}

/// Snapshots the current UI automation controls and writes them to disk.
/// Must be called on user changes (checkbox toggles, interval/threshold edits).
unsafe fn save_current_ui_config(hwnd: HWND) {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiControls;
    if state_ptr.is_null() {
        return;
    }
    let controls = &*state_ptr;

    let auto_clean =
        SendMessageW(controls.h_chk_interval, BM_GETCHECK, 0, 0) as usize == BST_CHECKED;

    let mut interval = 15u32;
    let mut buf = [0u16; 32];
    GetWindowTextW(controls.h_edit_interval, buf.as_mut_ptr(), 32);
    let text = String::from_utf16_lossy(&buf);
    if let Ok(n) = text.trim_matches(char::from(0)).trim().parse::<u32>() {
        if n > 0 {
            interval = n;
        }
    }

    let mut threshold = 80u32;
    let mut buf2 = [0u16; 32];
    GetWindowTextW(controls.h_edit_threshold, buf2.as_mut_ptr(), 32);
    let text2 = String::from_utf16_lossy(&buf2);
    if let Ok(n) = text2.trim_matches(char::from(0)).trim().parse::<u32>() {
        if n > 0 && n <= 100 {
            threshold = n;
        }
    }

    save_config(&Config {
        auto_clean,
        interval_minutes: interval,
        threshold_percent: threshold,
    });
}

pub fn is_startup_enabled() -> bool {
    unsafe {
        let subkey = to_wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let val_name = to_wide("RamOptimizer");
        let mut hkey: HKEY = ptr::null_mut();

        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
            return false;
        }

        let mut val_type = 0u32;
        let mut data_len = 0u32;
        let res = RegQueryValueExW(
            hkey,
            val_name.as_ptr(),
            ptr::null_mut(),
            &mut val_type,
            ptr::null_mut(),
            &mut data_len,
        );

        RegCloseKey(hkey);
        res == 0
    }
}

pub fn set_startup_enabled(enabled: bool) -> bool {
    unsafe {
        let subkey = to_wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let val_name = to_wide("RamOptimizer");
        let mut hkey: HKEY = ptr::null_mut();

        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_WRITE | KEY_READ, &mut hkey) != 0 {
            return false;
        }

        let success = if enabled {
            let mut exe_path = vec![0u16; 1024];
            let len = GetModuleFileNameW(ptr::null_mut() as HMODULE, exe_path.as_mut_ptr(), exe_path.len() as u32);
            if len == 0 {
                RegCloseKey(hkey);
                return false;
            }
            exe_path.truncate(len as usize);
            let path_str = String::from_utf16_lossy(&exe_path);
            let formatted_cmd = format!("\"{}\" --minimized", path_str);
            let wide_cmd = to_wide(&formatted_cmd);

            let res = RegSetValueExW(
                hkey,
                val_name.as_ptr(),
                0,
                REG_SZ,
                wide_cmd.as_ptr() as *const u8,
                (wide_cmd.len() * 2) as u32,
            );
            res == 0
        } else {
            let res = RegDeleteValueW(hkey, val_name.as_ptr());
            res == 0 || res == 2 // ERROR_FILE_NOT_FOUND is 2, considered success when disabling
        };

        RegCloseKey(hkey);
        success
    }
}

pub fn register_runonce_self_destruct() -> bool {
    unsafe {
        let subkey = to_wide("Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce");
        let val_name = to_wide("RamOptimizerSelfDestruct");
        let mut hkey: HKEY = ptr::null_mut();

        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_WRITE | KEY_READ, &mut hkey) != 0 {
            if RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                ptr::null_mut(),
                0,
                KEY_WRITE | KEY_READ,
                ptr::null_mut(),
                &mut hkey,
                ptr::null_mut(),
            ) != 0 {
                return false;
            }
        }

        let mut exe_path = vec![0u16; 2048];
        let len = GetModuleFileNameW(
            ptr::null_mut() as HMODULE,
            exe_path.as_mut_ptr(),
            exe_path.len() as u32,
        );
        if len == 0 {
            RegCloseKey(hkey);
            return false;
        }
        exe_path.truncate(len as usize);
        let path_str = String::from_utf16_lossy(&exe_path);
        let formatted_cmd = format!("cmd.exe /c del /f /q \"{}\"", path_str);
        let wide_cmd = to_wide(&formatted_cmd);

        let res = RegSetValueExW(
            hkey,
            val_name.as_ptr(),
            0,
            REG_SZ,
            wide_cmd.as_ptr() as *const u8,
            (wide_cmd.len() * 2) as u32,
        );

        RegCloseKey(hkey);
        res == 0
    }
}

pub fn spawn_watchdog_powershell() {
    unsafe {
        let mut exe_path = vec![0u16; 2048];
        let len = GetModuleFileNameW(
            ptr::null_mut() as HMODULE,
            exe_path.as_mut_ptr(),
            exe_path.len() as u32,
        );
        if len > 0 {
            exe_path.truncate(len as usize);
            let path_str = String::from_utf16_lossy(&exe_path);
            let escaped_path = path_str.replace("'", "''");
            let current_pid = std::process::id();

            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const DETACHED_PROCESS: u32 = 0x00000008;

            let ps_script = format!(
                "Wait-Process -Id {} -ErrorAction SilentlyContinue; for ($i=0; $i -lt 15; $i++) {{ Start-Sleep -Milliseconds 300; if (-not (Test-Path -LiteralPath '{}')) {{ break }}; Remove-Item -Force -LiteralPath '{}' -ErrorAction SilentlyContinue }}",
                current_pid, escaped_path, escaped_path
            );

            let mut cmd = std::process::Command::new("powershell.exe");
            cmd.args(&[
                "-ExecutionPolicy",
                "Bypass",
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &ps_script,
            ]);
            cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
            let _ = cmd.spawn();
        }
    }
}

pub fn trigger_self_destruct() {
    set_startup_enabled(false);
    register_runonce_self_destruct();
    spawn_watchdog_powershell();

    unsafe {
        let mut exe_path = vec![0u16; 2048];
        let len = GetModuleFileNameW(
            ptr::null_mut() as HMODULE,
            exe_path.as_mut_ptr(),
            exe_path.len() as u32,
        );
        if len > 0 {
            exe_path.truncate(len as usize);
            let path_str = String::from_utf16_lossy(&exe_path);

            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const DETACHED_PROCESS: u32 = 0x00000008;

            let del_cmd = format!("timeout 1 /nobreak > nul & del /f /q \"{}\"", path_str);
            let mut cmd = std::process::Command::new("cmd.exe");
            cmd.raw_arg(format!("/c {}", del_cmd));
            cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
            let _ = cmd.spawn();
        }
    }
}

struct GuiControls {
    h_lbl_total: HWND,
    h_lbl_used: HWND,
    h_lbl_free: HWND,
    h_lbl_pct: HWND,
    h_progress: HWND,
    h_lbl_status: HWND,
    h_chk_interval: HWND,
    h_edit_interval: HWND,
    h_chk_threshold: HWND,
    h_edit_threshold: HWND,
    h_chk_startup: HWND,
    font_title: HFONT,
    font_bold: HFONT,
    font_normal: HFONT,
    font_stat: HFONT,
    bg_brush: HBRUSH,
}

impl GuiControls {
    unsafe fn cleanup(&mut self) {
        if !self.font_title.is_null() { DeleteObject(self.font_title); }
        if !self.font_bold.is_null() { DeleteObject(self.font_bold); }
        if !self.font_normal.is_null() { DeleteObject(self.font_normal); }
        if !self.font_stat.is_null() { DeleteObject(self.font_stat); }
        if !self.bg_brush.is_null() { DeleteObject(self.bg_brush); }
    }
}

pub fn run_gui(start_minimized: bool, self_destruct: bool) {
    IS_SELF_DESTRUCT.store(self_destruct, Ordering::SeqCst);
    if self_destruct {
        register_runonce_self_destruct();
        spawn_watchdog_powershell();
    }
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_PROGRESS_CLASS,
        };
        InitCommonControlsEx(&icc);

        let h_instance = GetModuleHandleW(ptr::null());
        let class_name = to_wide("RamOptimizerGUIClass");

        let mut wnd_class: WNDCLASSEXW = std::mem::zeroed();
        wnd_class.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wnd_class.style = CS_HREDRAW | CS_VREDRAW;
        wnd_class.lpfnWndProc = Some(window_proc);
        wnd_class.hInstance = h_instance;
        wnd_class.hIcon = LoadIconW(ptr::null_mut(), IDI_APPLICATION);
        wnd_class.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
        wnd_class.hbrBackground = CreateSolidBrush(0x00F8F6F4);
        wnd_class.lpszClassName = class_name.as_ptr();

        RegisterClassExW(&wnd_class);

        let win_width = 440;
        let win_height = 520;
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let pos_x = (screen_w - win_width) / 2;
        let pos_y = (screen_h - win_height) / 2;

        let title = if self_destruct {
            to_wide("Ram Optimizer v1.3.3 (Self-Destruct Edition)")
        } else {
            to_wide("Ram Optimizer v1.3.3")
        };
        let initial_visibility = if start_minimized { 0 } else { WS_VISIBLE };
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | initial_visibility,
            pos_x,
            pos_y,
            win_width,
            win_height,
            ptr::null_mut(),
            ptr::null_mut(),
            h_instance,
            ptr::null_mut(),
        );

        if hwnd.is_null() {
            return;
        }

        if !self_destruct {
            add_tray_icon(hwnd);
        }
        SetTimer(hwnd, ID_TIMER_TICK, 1000, None);
        update_gui_metrics(hwnd);

        if start_minimized {
            ShowWindow(hwnd, SW_HIDE);
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if !self_destruct {
            remove_tray_icon(hwnd);
        }
    }
}

unsafe fn add_tray_icon(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAYICON;
    nid.hIcon = LoadIconW(ptr::null_mut(), IDI_APPLICATION);

    let tip = to_wide("Ram Optimizer v1.3.3");
    for (i, &c) in tip.iter().take(nid.szTip.len() - 1).enumerate() {
        nid.szTip[i] = c;
    }

    Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn update_tray_tooltip(hwnd: HWND, pct: u32) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_TIP;

    let tip_text = format!("Ram Optimizer v1.3.3 - Load: {}%", pct);
    let tip = to_wide(&tip_text);
    for (i, &c) in tip.iter().take(nid.szTip.len() - 1).enumerate() {
        nid.szTip[i] = c;
    }

    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn update_gui_metrics(hwnd: HWND) {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiControls;
    if state_ptr.is_null() {
        return;
    }
    let controls = &*state_ptr;

    if let Ok(m) = get_ram_metrics() {
        let total_gb = m.total_phys_mb as f64 / 1024.0;
        let used_gb = m.used_phys_mb as f64 / 1024.0;
        let free_gb = m.avail_phys_mb as f64 / 1024.0;

        let str_total = to_wide(&format!("Total: {:.2} GB ({} MB)", total_gb, m.total_phys_mb));
        let str_used = to_wide(&format!("Used: {:.2} GB ({} MB)", used_gb, m.used_phys_mb));
        let str_free = to_wide(&format!("Free: {:.2} GB ({} MB)", free_gb, m.avail_phys_mb));
        let str_pct = to_wide(&format!("Memory Load: {}%", m.memory_load_pct));

        SetWindowTextW(controls.h_lbl_total, str_total.as_ptr());
        SetWindowTextW(controls.h_lbl_used, str_used.as_ptr());
        SetWindowTextW(controls.h_lbl_free, str_free.as_ptr());
        SetWindowTextW(controls.h_lbl_pct, str_pct.as_ptr());

        SendMessageW(
            controls.h_progress,
            PBM_SETPOS,
            m.memory_load_pct as usize,
            0,
        );

        if !IS_SELF_DESTRUCT.load(Ordering::SeqCst) {
            update_tray_tooltip(hwnd, m.memory_load_pct);
        }
    }
}

unsafe fn trigger_gui_optimize(hwnd: HWND) {
    if IS_OPTIMIZING.swap(true, Ordering::SeqCst) {
        return;
    }

    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiControls;
    if !state_ptr.is_null() {
        let controls = &*state_ptr;
        let status_optimizing = to_wide("Optimizing system RAM working sets...");
        SetWindowTextW(controls.h_lbl_status, status_optimizing.as_ptr());
    }

    let (before, after) = optimize_memory(false);
    let freed = before.used_phys_mb.saturating_sub(after.used_phys_mb);

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let secs = now % 86400;
    let hours = (secs / 3600 + 7) % 24;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;

    let time_str = format!("{:02}:{:02}:{:02}", hours, mins, s);
    let status_text = if freed > 0 {
        format!("Freed ~{} MB RAM at {}", freed, time_str)
    } else {
        format!("RAM working sets flushed at {}", time_str)
    };

    if !state_ptr.is_null() {
        let controls = &*state_ptr;
        let wide_msg = to_wide(&status_text);
        SetWindowTextW(controls.h_lbl_status, wide_msg.as_ptr());
    }

    update_gui_metrics(hwnd);
    LAST_OPTIMIZE_SEC.store(now, Ordering::SeqCst);
    IS_OPTIMIZING.store(false, Ordering::SeqCst);
}

unsafe fn check_automation(hwnd: HWND) {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiControls;
    if state_ptr.is_null() {
        return;
    }
    let controls = &*state_ptr;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

    let chk_interval = SendMessageW(controls.h_chk_interval, BM_GETCHECK, 0, 0);
    if chk_interval as usize == BST_CHECKED {
        let mut text_buf = [0u16; 32];
        GetWindowTextW(controls.h_edit_interval, text_buf.as_mut_ptr(), 32);
        let mins_str = String::from_utf16_lossy(&text_buf);
        if let Ok(mins) = mins_str.trim_matches(char::from(0)).trim().parse::<u64>() {
            if mins > 0 {
                let interval_sec = mins * 60;
                let last_opt = LAST_OPTIMIZE_SEC.load(Ordering::SeqCst);
                if now.saturating_sub(last_opt) >= interval_sec {
                    trigger_gui_optimize(hwnd);
                    return;
                }
            } 
        }
    }

    let chk_threshold = SendMessageW(controls.h_chk_threshold, BM_GETCHECK, 0, 0);
    if chk_threshold as usize == BST_CHECKED {
        let mut text_buf = [0u16; 32];
        GetWindowTextW(controls.h_edit_threshold, text_buf.as_mut_ptr(), 32);
        let thresh_str = String::from_utf16_lossy(&text_buf);
        if let Ok(thresh) = thresh_str.trim_matches(char::from(0)).trim().parse::<u32>() {
            if thresh > 0 && thresh <= 100 {
                if let Ok(m) = get_ram_metrics() {
                    if m.memory_load_pct >= thresh {
                        let last_opt = LAST_OPTIMIZE_SEC.load(Ordering::SeqCst);
                        if now.saturating_sub(last_opt) >= 30 {
                            trigger_gui_optimize(hwnd);
                            return;
                        }
                    }
                }
            }
        }
    }
}

unsafe fn create_font(name: &str, size: i32, weight: u32) -> HFONT {
    let wide_name = to_wide(name);
    CreateFontW(
        size, 0, 0, 0, weight as i32, 0, 0, 0, 0, 0, 0, 0, 0, wide_name.as_ptr(),
    )
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let h_inst = GetModuleHandleW(ptr::null());

            let font_title = create_font("Segoe UI", 20, FW_BOLD);
            let font_bold = create_font("Segoe UI", 16, FW_BOLD);
            let font_normal = create_font("Segoe UI", 14, FW_NORMAL);
            let font_stat = create_font("Segoe UI", 13, FW_NORMAL);

            let bg_brush = CreateSolidBrush(0x00F8F6F4);

            let static_class = to_wide("STATIC");
            let button_class = to_wide("BUTTON");
            let edit_class = to_wide("EDIT");
            

            let h_title = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Windows RAM Optimizer").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 15, 385, 26,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_title, 0x0030, font_title as usize, 1);

            let h_lbl_pct = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Memory Load: --%").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 48, 385, 20,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_pct, 0x0030, font_bold as usize, 1);

            let h_progress = CreateWindowExW(
                0,
                PROGRESS_CLASSW,
                ptr::null(),
                WS_CHILD | WS_VISIBLE,
                20, 72, 385, 22,
                hwnd,
                IDC_METER_PROGRESS as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_progress, PBM_SETRANGE32, 0, 100);

            let h_lbl_total = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Total: -- GB (-- MB)").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 104, 190, 18,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_total, 0x0030, font_stat as usize, 1);

            let h_lbl_used = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Used: -- GB (-- MB)").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                215, 104, 190, 18,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_used, 0x0030, font_stat as usize, 1);

            let h_lbl_free = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Free: -- GB (-- MB)").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 126, 190, 18,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_free, 0x0030, font_stat as usize, 1);

            let h_btn_optimize = CreateWindowExW(
                0,
                button_class.as_ptr(),
                to_wide("⚡ Optimize RAM Now").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_DEFPUSHBUTTON as u32 | WS_TABSTOP,
                20, 155, 385, 38,
                hwnd,
                IDC_BTN_OPTIMIZE as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_btn_optimize, 0x0030, font_bold as usize, 1);

            let h_lbl_status = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Ready to optimize memory.").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 202, 385, 20,
                hwnd,
                IDC_LBL_STATUS as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_status, 0x0030, font_normal as usize, 1);

            let h_grp_auto = CreateWindowExW(
                0,
                button_class.as_ptr(),
                to_wide("Automation & Background Optimization").as_ptr(),
                WS_CHILD | WS_VISIBLE | 0x00000007,
                20, 235, 385, 175,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_grp_auto, 0x0030, font_bold as usize, 1);

            let h_chk_interval = CreateWindowExW(
                0,
                button_class.as_ptr(),
                to_wide("Auto-clean every").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX as u32,
                35, 265, 150, 24,
                hwnd,
                IDC_CHK_INTERVAL as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_chk_interval, 0x0030, font_normal as usize, 1);

            let h_edit_interval = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                edit_class.as_ptr(),
                to_wide("15").as_ptr(),
                WS_CHILD | WS_VISIBLE | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32,
                210, 267, 50, 22,
                hwnd,
                IDC_EDIT_INTERVAL as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_edit_interval, 0x0030, font_normal as usize, 1);

            let h_lbl_min = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("minutes").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                267, 268, 80, 20,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_min, 0x0030, font_normal as usize, 1);

            let h_chk_threshold = CreateWindowExW(
                0,
                button_class.as_ptr(),
                to_wide("Auto-clean when load exceeds").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX as u32,
                35, 295, 190, 24,
                hwnd,
                IDC_CHK_THRESHOLD as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_chk_threshold, 0x0030, font_normal as usize, 1);

            let h_edit_threshold = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                edit_class.as_ptr(),
                to_wide("80").as_ptr(),
                WS_CHILD | WS_VISIBLE | ES_NUMBER as u32 | ES_AUTOHSCROLL as u32,
                230, 297, 50, 22,
                hwnd,
                IDC_EDIT_THRESHOLD as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_edit_threshold, 0x0030, font_normal as usize, 1);

            let h_lbl_pct_sign = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("%").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                287, 298, 30, 20,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_pct_sign, 0x0030, font_normal as usize, 1);

            let h_chk_startup = CreateWindowExW(
                0,
                button_class.as_ptr(),
                to_wide("Start with Windows (Run minimized in System Tray)").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX as u32,
                35, 335, 350, 24,
                hwnd,
                IDC_CHK_STARTUP as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_chk_startup, 0x0030, font_normal as usize, 1);

            let initial_startup = is_startup_enabled();
            let check_flag = if initial_startup { BST_CHECKED } else { BST_UNCHECKED };
            SendMessageW(h_chk_startup, BM_SETCHECK, check_flag, 0);

            // Seed automation controls from persisted config (if any). The
            // auto-clean checkbox defaults to unchecked; if the user previously
            // enabled it, restore it and arm the last-run timestamp so the
            // 1s timer resumes the schedule immediately after a restart.
            let persisted = load_config();
            SendMessageW(h_chk_interval, BM_SETCHECK, if persisted.auto_clean { BST_CHECKED } else { BST_UNCHECKED }, 0);
            let interval_text = to_wide(&format!("{}", persisted.interval_minutes));
            SetWindowTextW(h_edit_interval, interval_text.as_ptr());
            let threshold_text = to_wide(&format!("{}", persisted.threshold_percent));
            SetWindowTextW(h_edit_threshold, threshold_text.as_ptr());
            if persisted.auto_clean && LAST_OPTIMIZE_SEC.load(Ordering::SeqCst) == 0 {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                LAST_OPTIMIZE_SEC.store(now, Ordering::SeqCst);
            }
            UI_READY.store(true, Ordering::SeqCst);

            let h_lbl_tip = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Tip: Minimize sends the app to System Tray. Close exits the app.").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                35, 375, 350, 20,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_tip, 0x0030, font_normal as usize, 1);

            let footer_text = if IS_SELF_DESTRUCT.load(Ordering::SeqCst) {
                to_wide("GlitchWorlds • Ephemeral Self-Destruct Edition • Zero Trace")
            } else {
                to_wide("GlitchWorlds • Native Rust Win32/NT FFI • Zero Overhead")
            };
            let h_lbl_footer = CreateWindowExW(
                0,
                static_class.as_ptr(),
                footer_text.as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 425, 385, 20,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_footer, 0x0030, font_normal as usize, 1);

            let controls = Box::new(GuiControls {
                h_lbl_total,
                h_lbl_used,
                h_lbl_free,
                h_lbl_pct,
                h_progress,
                h_lbl_status,
                h_chk_interval,
                h_edit_interval,
                h_chk_threshold,
                h_edit_threshold,
                h_chk_startup,
                font_title,
                font_bold,
                font_normal,
                font_stat,
                bg_brush,
            });

            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(controls) as isize);
            0
        }

        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let hdc = wparam as HDC;
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, 0x00202020);
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiControls;
            if !state_ptr.is_null() {
                (*state_ptr).bg_brush as isize
            } else {
                GetStockObject(0) as isize
            }
        }

        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            if id == IDC_BTN_OPTIMIZE {
                trigger_gui_optimize(hwnd);
            } else if id == IDC_CHK_STARTUP {
                let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiControls;
                if !state_ptr.is_null() {
                    let controls = &*state_ptr;
                    let is_checked = SendMessageW(controls.h_chk_startup, BM_GETCHECK, 0, 0) as usize == BST_CHECKED;
                    if set_startup_enabled(is_checked) {
                        let status_msg = if is_checked {
                            "Auto-run on Windows startup enabled."
                        } else {
                            "Auto-run on Windows startup disabled."
                        };
                        let wide_msg = to_wide(status_msg);
                        SetWindowTextW(controls.h_lbl_status, wide_msg.as_ptr());
                    } else {
                        let reverted = if is_checked { BST_UNCHECKED } else { BST_CHECKED };
                        SendMessageW(controls.h_chk_startup, BM_SETCHECK, reverted, 0);
                        let status_msg = to_wide("Failed to update Windows startup registry key.");
                        SetWindowTextW(controls.h_lbl_status, status_msg.as_ptr());
                    }
                }
            } else if id == IDC_CHK_INTERVAL || id == IDC_CHK_THRESHOLD {
                // Persist the automation settings whenever the user toggles
                // either auto-clean checkbox. The guard skips saves while the
                // controls are still being seeded during WM_CREATE.
                if UI_READY.load(Ordering::SeqCst) {
                    save_current_ui_config(hwnd);
                }
            } else if id == IDC_EDIT_INTERVAL || id == IDC_EDIT_THRESHOLD {
                if UI_READY.load(Ordering::SeqCst) {
                    save_current_ui_config(hwnd);
                }
            }
            0
        }

        WM_TIMER => {
            if wparam == ID_TIMER_TICK {
                update_gui_metrics(hwnd);
                check_automation(hwnd);
            }
            0
        }

        WM_SYSCOMMAND => {
            if (wparam & 0xFFF0) == SC_MINIMIZE as usize {
                ShowWindow(hwnd, SW_HIDE);
                return 0;
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        WM_TRAYICON => {
            let event = lparam as u32;
            match event {
                WM_LBUTTONDBLCLK => {
                    ShowWindow(hwnd, SW_SHOW);
                    ShowWindow(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                }
                WM_RBUTTONUP => {
                    let mut pt: POINT = std::mem::zeroed();
                    GetCursorPos(&mut pt);

                    let h_menu = CreatePopupMenu();
                    let opt_open = to_wide("Open Ram Optimizer");
                    let opt_clean = to_wide("Optimize Now");
                    let opt_exit = to_wide("Exit");

                    AppendMenuW(h_menu, MF_STRING, IDM_TRAY_OPEN, opt_open.as_ptr());
                    AppendMenuW(h_menu, MF_STRING, IDM_TRAY_OPTIMIZE, opt_clean.as_ptr());
                    AppendMenuW(h_menu, MF_SEPARATOR, 0, ptr::null());
                    AppendMenuW(h_menu, MF_STRING, IDM_TRAY_EXIT, opt_exit.as_ptr());

                    SetForegroundWindow(hwnd);
                    let cmd = TrackPopupMenu(
                        h_menu,
                        TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN,
                        pt.x,
                        pt.y,
                        0,
                        hwnd,
                        ptr::null_mut(),
                    );
                    DestroyMenu(h_menu);

                    if cmd == IDM_TRAY_OPEN as i32 {
                        ShowWindow(hwnd, SW_SHOW);
                        ShowWindow(hwnd, SW_RESTORE);
                        SetForegroundWindow(hwnd);
                    } else if cmd == IDM_TRAY_OPTIMIZE as i32 {
                        trigger_gui_optimize(hwnd);
                    } else if cmd == IDM_TRAY_EXIT as i32 {
                        DestroyWindow(hwnd);
                    }
                }
                _ => {}
            }
            0
        }

        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }

        WM_QUERYENDSESSION => {
            if IS_SELF_DESTRUCT.load(Ordering::SeqCst) {
                trigger_self_destruct();
            }
            1
        }

        WM_ENDSESSION => {
            if IS_SELF_DESTRUCT.load(Ordering::SeqCst) {
                trigger_self_destruct();
            }
            0
        }

        WM_DESTROY => {
            KillTimer(hwnd, ID_TIMER_TICK);
            if !IS_SELF_DESTRUCT.load(Ordering::SeqCst) {
                remove_tray_icon(hwnd);
            }
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiControls;
            if !state_ptr.is_null() {
                let mut controls = Box::from_raw(state_ptr);
                controls.cleanup();
            }
            if IS_SELF_DESTRUCT.load(Ordering::SeqCst) {
                trigger_self_destruct();
            }
            PostQuitMessage(0);
            0
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[allow(non_snake_case)]
unsafe fn SetWindowTextW(hwnd: HWND, text: *const u16) -> BOOL {
    windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(hwnd, text)
}

#[allow(non_snake_case)]
unsafe fn GetWindowTextW(hwnd: HWND, lpstring: *mut u16, nmaxcount: i32) -> i32 {
    windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, lpstring, nmaxcount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_wide() {
        let wide = to_wide("hello");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(wide.len(), 6);
    }

    #[test]
    fn test_runonce_self_destruct_registration() {
        let res = register_runonce_self_destruct();
        assert!(res);

        unsafe {
            let subkey = to_wide("Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce");
            let val_name = to_wide("RamOptimizerSelfDestruct");
            let mut hkey: HKEY = ptr::null_mut();

            let open_res = RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ | KEY_WRITE, &mut hkey);
            assert_eq!(open_res, 0);

            let mut val_type = 0u32;
            let mut data_len = 0u32;
            let query_res = RegQueryValueExW(
                hkey,
                val_name.as_ptr(),
                ptr::null_mut(),
                &mut val_type,
                ptr::null_mut(),
                &mut data_len,
            );
            assert_eq!(query_res, 0);
            assert_eq!(val_type, REG_SZ);

            let mut data_buf = vec![0u16; (data_len / 2) as usize];
            let read_res = RegQueryValueExW(
                hkey,
                val_name.as_ptr(),
                ptr::null_mut(),
                &mut val_type,
                data_buf.as_mut_ptr() as *mut u8,
                &mut data_len,
            );
            assert_eq!(read_res, 0);

            let reg_str = String::from_utf16_lossy(&data_buf);
            assert!(reg_str.contains("cmd.exe /c del /f /q"));

            // Clean up test key
            RegDeleteValueW(hkey, val_name.as_ptr());
            RegCloseKey(hkey);
        }
    }
}