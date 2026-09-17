use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{
    BOOL, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, GetStockObject, SetBkMode,
    SetTextColor, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    InitCommonControlsEx, INITCOMMONCONTROLSEX, ICC_PROGRESS_CLASS, PBM_SETPOS, PBM_SETRANGE32,
    PROGRESS_CLASSW,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, KillTimer, LoadCursorW, LoadIconW, PostQuitMessage,
    RegisterClassExW, SendMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW,
    ShowWindow, TrackPopupMenu, TranslateMessage, BM_GETCHECK,
    CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HMENU, IDC_ARROW, IDI_APPLICATION,
    MF_SEPARATOR, MF_STRING, MSG, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_RESTORE,
    SW_SHOW, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WM_CLOSE, WM_COMMAND,
    WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLORSTATIC, WM_DESTROY, WM_LBUTTONDBLCLK,
    WM_RBUTTONUP, WM_SYSCOMMAND, WM_TIMER, WM_USER, WNDCLASSEXW, WS_CHILD,
    WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP,
    WS_VISIBLE, WS_CAPTION, WS_MINIMIZEBOX, ES_AUTOHSCROLL, ES_NUMBER, BS_AUTOCHECKBOX,
    BS_DEFPUSHBUTTON, SC_MINIMIZE,
};

use crate::{get_ram_metrics, optimize_memory};

const ID_TIMER_TICK: usize = 1001;
const WM_TRAYICON: u32 = WM_USER + 201;

const BST_CHECKED: isize = 1;

const IDC_METER_PROGRESS: i32 = 2001;
const IDC_BTN_OPTIMIZE: i32 = 2002;
const IDC_CHK_INTERVAL: i32 = 2003;
const IDC_EDIT_INTERVAL: i32 = 2004;
const IDC_CHK_THRESHOLD: i32 = 2005;
const IDC_EDIT_THRESHOLD: i32 = 2006;
const IDC_LBL_STATUS: i32 = 2007;

const IDM_TRAY_OPEN: usize = 3001;
const IDM_TRAY_OPTIMIZE: usize = 3002;
const IDM_TRAY_EXIT: usize = 3003;

static IS_OPTIMIZING: AtomicBool = AtomicBool::new(false);
static LAST_OPTIMIZE_SEC: AtomicU64 = AtomicU64::new(0);

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
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

pub fn run_gui() {
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
        let win_height = 490;
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let pos_x = (screen_w - win_width) / 2;
        let pos_y = (screen_h - win_height) / 2;

        let title = to_wide("Ram Optimizer v1.1.0");
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE,
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

        add_tray_icon(hwnd);
        SetTimer(hwnd, ID_TIMER_TICK, 1000, None);
        update_gui_metrics(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        remove_tray_icon(hwnd);
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

    let tip = to_wide("Ram Optimizer v1.1.0");
    for (i, &c) in tip.iter().take(nid.szTip.len() - 1).enumerate() {
        nid.szTip[i] = c;
    }

    Shell_NotifyIconW(NIM_ADD, &nid);
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
    if chk_interval == BST_CHECKED {
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
    if chk_threshold == BST_CHECKED {
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

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let h_inst = GetModuleHandleW(ptr::null());

            let font_title = create_font("Segoe UI", -17, FW_BOLD);
            let font_bold = create_font("Segoe UI", -14, FW_BOLD);
            let font_normal = create_font("Segoe UI", -13, FW_NORMAL);
            let font_stat = create_font("Segoe UI", -15, FW_BOLD);
            let bg_brush = CreateSolidBrush(0x00F8F6F4);

            let static_class = to_wide("STATIC");
            let button_class = to_wide("BUTTON");
            let edit_class = to_wide("EDIT");

            let h_title = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Ram Optimizer - Windows Memory Cleaner").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 15, 385, 25,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_title, 0x0030, font_title as usize, 1);

            let h_lbl_pct = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Memory Load: 0%").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                25, 52, 190, 22,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_pct, 0x0030, font_stat as usize, 1);

            let h_lbl_total = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Total: 0 GB (0 MB)").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                220, 52, 190, 22,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_total, 0x0030, font_normal as usize, 1);

            let h_progress = CreateWindowExW(
                0,
                PROGRESS_CLASSW,
                ptr::null(),
                WS_CHILD | WS_VISIBLE,
                25, 80, 375, 24,
                hwnd,
                IDC_METER_PROGRESS as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_progress, PBM_SETRANGE32, 0, 100);

            let h_lbl_used = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Used: 0 GB (0 MB)").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                25, 112, 180, 20,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_used, 0x0030, font_bold as usize, 1);

            let h_lbl_free = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Free: 0 GB (0 MB)").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                220, 112, 180, 20,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_free, 0x0030, font_bold as usize, 1);

            let h_btn_optimize = CreateWindowExW(
                0,
                button_class.as_ptr(),
                to_wide("Optimize RAM Now").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
                25, 145, 375, 45,
                hwnd,
                IDC_BTN_OPTIMIZE as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_btn_optimize, 0x0030, font_title as usize, 1);

            let h_lbl_status = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Ready to optimize physical memory & standby list.").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                25, 200, 375, 22,
                hwnd,
                IDC_LBL_STATUS as HMENU,
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_status, 0x0030, font_normal as usize, 1);

            let h_grp_auto = CreateWindowExW(
                0,
                button_class.as_ptr(),
                to_wide(" Background Automation ").as_ptr(),
                WS_CHILD | WS_VISIBLE | 0x00000007,
                20, 230, 385, 145,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_grp_auto, 0x0030, font_bold as usize, 1);

            let h_chk_interval = CreateWindowExW(
                0,
                button_class.as_ptr(),
                to_wide("Auto-optimize every").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX as u32,
                35, 260, 140, 24,
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
                180, 260, 45, 24,
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
                232, 263, 80, 20,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_min, 0x0030, font_normal as usize, 1);

            let h_chk_threshold = CreateWindowExW(
                0,
                button_class.as_ptr(),
                to_wide("Auto-optimize when RAM load >").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX as u32,
                35, 300, 195, 24,
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
                235, 300, 45, 24,
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
                287, 303, 30, 20,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_pct_sign, 0x0030, font_normal as usize, 1);

            let h_lbl_tip = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("Tip: Minimizing window hides it to System Tray.").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                35, 340, 350, 20,
                hwnd,
                ptr::null_mut(),
                h_inst,
                ptr::null_mut(),
            );
            SendMessageW(h_lbl_tip, 0x0030, font_normal as usize, 1);

            let h_lbl_footer = CreateWindowExW(
                0,
                static_class.as_ptr(),
                to_wide("GlitchWorlds • Native Rust Win32/NT FFI • Zero Overhead").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 390, 385, 20,
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
            ShowWindow(hwnd, SW_HIDE);
            0
        }

        WM_DESTROY => {
            KillTimer(hwnd, ID_TIMER_TICK);
            remove_tray_icon(hwnd);
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiControls;
            if !state_ptr.is_null() {
                let mut controls = Box::from_raw(state_ptr);
                controls.cleanup();
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