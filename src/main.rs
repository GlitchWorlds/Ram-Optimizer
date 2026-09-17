use std::env;
use std::io::{self, Write};
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, HMODULE, LUID};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows_sys::Win32::System::SystemInformation::{
    GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows_sys::Win32::System::ProcessStatus::{
    EmptyWorkingSet, EnumProcesses,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_INFORMATION,
    PROCESS_SET_QUOTA,
};

type NtSetSystemInformationFn = unsafe extern "system" fn(
    system_information_class: u32,
    system_information: *const std::ffi::c_void,
    system_information_length: u32,
) -> i32;

extern "system" {
    fn GetModuleHandleA(lpmodulename: *const u8) -> HMODULE;
    fn GetProcAddress(hmodule: HMODULE, lpprocname: *const u8) -> *mut std::ffi::c_void;
}

const SYSTEM_MEMORY_LIST_INFORMATION: u32 = 80;
const SYSTEM_EMPTY_WORKING_SET_LIST: u32 = 2;
const SYSTEM_PURGE_STANDBY_LIST: u32 = 3;

#[derive(Debug, Clone, Copy)]
pub struct RamMetrics {
    pub total_phys_mb: u64,
    pub avail_phys_mb: u64,
    pub used_phys_mb: u64,
    pub memory_load_pct: u32,
}

pub fn get_ram_metrics() -> io::Result<RamMetrics> {
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;

        if GlobalMemoryStatusEx(&mut status) == 0 {
            return Err(io::Error::last_os_error());
        }

        let mb = 1024 * 1024;
        let total = status.ullTotalPhys / mb;
        let avail = status.ullAvailPhys / mb;
        let used = total.saturating_sub(avail);

        Ok(RamMetrics {
            total_phys_mb: total,
            avail_phys_mb: avail,
            used_phys_mb: used,
            memory_load_pct: status.dwMemoryLoad,
        })
    }
}

pub fn enable_privilege(privilege_name: &str) -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        ) == 0
        {
            return false;
        }

        let wide_name: Vec<u16> = privilege_name.encode_utf16().chain(Some(0)).collect();
        let mut luid: LUID = std::mem::zeroed();

        if LookupPrivilegeValueW(std::ptr::null(), wide_name.as_ptr(), &mut luid) == 0 {
            CloseHandle(token);
            return false;
        }

        let mut tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };

        let res = AdjustTokenPrivileges(
            token,
            0,
            &mut tp,
            size_of::<TOKEN_PRIVILEGES>() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );

        let err = GetLastError();
        CloseHandle(token);

        res != 0 && err == 0
    }
}

pub fn purge_processes_working_set() -> (usize, usize) {
    let mut pids = vec![0u32; 2048];
    let mut bytes_returned = 0u32;

    unsafe {
        if EnumProcesses(
            pids.as_mut_ptr(),
            (pids.len() * size_of::<u32>()) as u32,
            &mut bytes_returned,
        ) == 0
        {
            return (0, 0);
        }
    }

    let count = (bytes_returned as usize) / size_of::<u32>();
    let mut purged_count = 0;
    let mut failed_count = 0;

    for &pid in pids.iter().take(count) {
        if pid == 0 {
            continue;
        }
        unsafe {
            let handle = OpenProcess(
                PROCESS_QUERY_INFORMATION | PROCESS_SET_QUOTA,
                0,
                pid,
            );
            if !handle.is_null() {
                if EmptyWorkingSet(handle) != 0 {
                    purged_count += 1;
                } else {
                    failed_count += 1;
                }
                CloseHandle(handle);
            } else {
                failed_count += 1;
            }
        }
    }

    (purged_count, failed_count)
}

pub fn purge_standby_and_system_cache() -> bool {
    unsafe {
        let ntdll = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
        if ntdll.is_null() {
            return false;
        }

        let func_ptr = GetProcAddress(ntdll, b"NtSetSystemInformation\0".as_ptr());
        if func_ptr.is_null() {
            return false;
        }

        let nt_set_system_information: NtSetSystemInformationFn =
            std::mem::transmute(func_ptr);

        let cmd_working_set: u32 = SYSTEM_EMPTY_WORKING_SET_LIST;
        let res_ws = nt_set_system_information(
            SYSTEM_MEMORY_LIST_INFORMATION,
            &cmd_working_set as *const _ as *const std::ffi::c_void,
            size_of::<u32>() as u32,
        );

        let cmd_standby: u32 = SYSTEM_PURGE_STANDBY_LIST;
        let res_sb = nt_set_system_information(
            SYSTEM_MEMORY_LIST_INFORMATION,
            &cmd_standby as *const _ as *const std::ffi::c_void,
            size_of::<u32>() as u32,
        );

        res_ws >= 0 || res_sb >= 0
    }
}

pub fn optimize_memory(verbose: bool) -> (RamMetrics, RamMetrics) {
    let before = get_ram_metrics().expect("Failed to query initial memory state");

    if verbose {
        println!("[*] Initial State : {} MB used / {} MB total ({}%)",
            before.used_phys_mb, before.total_phys_mb, before.memory_load_pct);
        print!("[*] Elevating token privileges (SeDebugPrivilege, SeIncreaseQuotaPrivilege)... ");
        io::stdout().flush().unwrap();
    }

    let priv_debug = enable_privilege("SeDebugPrivilege");
    let priv_quota = enable_privilege("SeIncreaseQuotaPrivilege");
    let _priv_profile = enable_privilege("SeProfileSingleProcessPrivilege");

    if verbose {
        if priv_debug || priv_quota {
            println!("OK");
        } else {
            println!("Limited (Run as Administrator for full system cache purge)");
        }
        print!("[*] Purging process working sets... ");
        io::stdout().flush().unwrap();
    }

    let (purged, failed) = purge_processes_working_set();

    if verbose {
        println!("Done ({} processes trimmed, {} skipped)", purged, failed);
        print!("[*] Purging system file cache & standby memory list... ");
        io::stdout().flush().unwrap();
    }

    let nt_success = purge_standby_and_system_cache();

    if verbose {
        if nt_success {
            println!("OK");
        } else {
            println!("Skipped/Access Denied (Requires Admin)");
        }
    }

    let after = get_ram_metrics().expect("Failed to query post-optimization memory state");

    if verbose {
        let freed = before.used_phys_mb.saturating_sub(after.used_phys_mb);
        println!("[*] Post-Optimization: {} MB used / {} MB total ({}%)",
            after.used_phys_mb, after.total_phys_mb, after.memory_load_pct);
        if before.used_phys_mb >= after.used_phys_mb {
            println!("[+] Successfully freed: ~{} MB RAM", freed);
        } else {
            println!("[*] Working sets cleared. Active system allocation reclaimed cache.");
        }
    }

    (before, after)
}

fn print_banner() {
    println!(r#"
===================================================================
     ____                       ___        _   _           _              
    |  _ \  __ _  _ __ ___     / _ \  _ __ | |_(_) _ __ ___ (_)_______ _ __ 
    | |_) |/ _` || '_ ` _ \   | | | || '_ \| __| || '_ ` _ \| |_  / _ \ '__|
    |  _ <| (_| || | | | | |  | |_| || |_) | |_| || | | | | | |/ /  __/ |   
    |_| \_\\__,_||_| |_| |_|   \___/ | .__/ \__|_||_| |_| |_|_/___\___|_|   
                                     |_|                                     
          High-Performance Windows RAM Optimizer - Native Rust Engine
          GlitchWorlds | Zero-Overhead | Native Win32/NT FFI
===================================================================
"#);
}

fn print_help() {
    println!("Usage:");
    println!("  ram-optimizer [FLAGS]");
    println!();
    println!("Flags:");
    println!("  --once                  Clean RAM immediately once, display freed memory, and exit");
    println!("  --interval <minutes>    Continuously optimize RAM every N minutes");
    println!("  --threshold <percent>   Auto-clean memory whenever load exceeds N%");
    println!("  --help, -h              Display this help information");
    println!();
    println!("Examples:");
    println!("  ram-optimizer --once");
    println!("  ram-optimizer --interval 15");
    println!("  ram-optimizer --threshold 80 --interval 5");
}

fn run_interactive_mode() {
    print_banner();
    loop {
        let current = match get_ram_metrics() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Error querying memory: {}", e);
                break;
            }
        };

        println!("-------------------------------------------------------------------");
        println!(" CURRENT MEMORY METRICS:");
        println!("  - Used Physical RAM  : {:>6} MB / {} MB", current.used_phys_mb, current.total_phys_mb);
        println!("  - Free Physical RAM  : {:>6} MB", current.avail_phys_mb);
        println!("  - Memory Load (Load) : {:>5}%", current.memory_load_pct);
        println!("-------------------------------------------------------------------");
        println!(" 1. Optimize RAM Now (Clean Working Sets + Standby List)");
        println!(" 2. Run Auto-Clean Timer (Interval Loop)");
        println!(" 3. Set Memory Load Threshold Watcher");
        println!(" 4. Exit");
        print!("\nSelect option [1-4]: ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            break;
        }

        match input.trim() {
            "1" => {
                println!();
                optimize_memory(true);
                println!();
            }
            "2" => {
                print!("Enter interval in minutes (e.g. 10): ");
                io::stdout().flush().unwrap();
                let mut int_str = String::new();
                if io::stdin().read_line(&mut int_str).is_ok() {
                    if let Ok(mins) = int_str.trim().parse::<u64>() {
                        if mins > 0 {
                            run_loop_mode(mins, None);
                        } else {
                            println!("Interval must be greater than 0.");
                        }
                    } else {
                        println!("Invalid number entered.");
                    }
                }
            }
            "3" => {
                print!("Enter threshold percentage (1-99): ");
                io::stdout().flush().unwrap();
                let mut pct_str = String::new();
                if io::stdin().read_line(&mut pct_str).is_ok() {
                    if let Ok(pct) = pct_str.trim().parse::<u32>() {
                        if pct > 0 && pct < 100 {
                            run_loop_mode(1, Some(pct));
                        } else {
                            println!("Threshold must be between 1 and 99.");
                        }
                    } else {
                        println!("Invalid percentage entered.");
                    }
                }
            }
            "4" | "exit" | "quit" | "q" => {
                println!("Exiting RAM Optimizer. Goodbye!");
                break;
            }
            _ => {
                println!("Unknown option. Please choose 1, 2, 3, or 4.");
            }
        }
    }
}

fn run_loop_mode(interval_mins: u64, threshold_pct: Option<u32>) {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    println!();
    println!("[*] Starting Background Optimization Daemon.");
    if let Some(pct) = threshold_pct {
        println!("[*] Trigger: Memory Load >= {}% (checking every {} min)", pct, interval_mins);
    } else {
        println!("[*] Trigger: Every {} minutes", interval_mins);
    }
    println!("[*] Press Ctrl+C to stop.\n");

    let sleep_duration = Duration::from_secs(interval_mins * 60);

    while r.load(Ordering::SeqCst) {
        let metrics = match get_ram_metrics() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[!] Failed to read RAM status: {}", e);
                break;
            }
        };

        let should_clean = match threshold_pct {
            Some(t) => metrics.memory_load_pct >= t,
            None => true,
        };

        if should_clean {
            println!("[{}] Trigger fired (Current Load: {}%). Running optimizer...",
                chrono_timestamp(), metrics.memory_load_pct);
            optimize_memory(true);
            println!();
        } else {
            println!("[{}] RAM Load ({}%) below threshold ({}%). Standby...",
                chrono_timestamp(), metrics.memory_load_pct, threshold_pct.unwrap());
        }

        thread::sleep(sleep_duration);
    }
}

fn chrono_timestamp() -> String {
    let now = std::time::SystemTime::now();
    let duration = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs() % 86400;
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02} UTC", hours, mins, s)
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() == 1 {
        run_interactive_mode();
        return;
    }

    let mut once = false;
    let mut interval: Option<u64> = None;
    let mut threshold: Option<u32> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--once" => {
                once = true;
            }
            "--interval" => {
                if i + 1 < args.len() {
                    if let Ok(val) = args[i + 1].parse::<u64>() {
                        interval = Some(val);
                        i += 1;
                    } else {
                        eprintln!("Error: --interval requires an integer (minutes).");
                        std::process::exit(1);
                    }
                } else {
                    eprintln!("Error: --interval requires a value.");
                    std::process::exit(1);
                }
            }
            "--threshold" => {
                if i + 1 < args.len() {
                    if let Ok(val) = args[i + 1].parse::<u32>() {
                        threshold = Some(val);
                        i += 1;
                    } else {
                        eprintln!("Error: --threshold requires an integer percentage (1-100).");
                        std::process::exit(1);
                    }
                } else {
                    eprintln!("Error: --threshold requires a value.");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                print_banner();
                print_help();
                return;
            }
            unknown => {
                eprintln!("Unknown option: {}", unknown);
                print_help();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    if once {
        print_banner();
        optimize_memory(true);
        return;
    }

    if interval.is_some() || threshold.is_some() {
        print_banner();
        let interval_mins = interval.unwrap_or(5);
        run_loop_mode(interval_mins, threshold);
        return;
    }

    run_interactive_mode();
}
