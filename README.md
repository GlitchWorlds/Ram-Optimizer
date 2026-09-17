# ⚡ Ram-Optimizer

> **Ultra-Lightweight, Blazing-Fast Windows RAM Optimizer written in Pure Rust using Native Win32 & NT Kernel FFI.**

[![Rust](https://img.shields.io/badge/Rust-1.97%2B-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%2010%20%2F%2011%20%2F%20Server-lightgrey.svg?style=flat-square&logo=windows)](https://microsoft.com/windows)

**Ram-Optimizer** is an open-source, zero-cost abstraction alternative to proprietary memory cleaners like Wise Memory Optimizer or CleanMem. Built specifically for Windows systems, it communicates directly with low-level Windows NT subsystems without heavy frameworks, .NET runtime dependencies, or garbage collectors.

---

## ✨ Features

- **Native Win32 GUI Interface**:
  - Zero-overhead pure Win32 API window (~440x490 fixed).
  - Real-time Visual Memory Meter (`msctls_progress32`) and memory stats (Total, Used, Free in GB/MB).
  - Prominent one-click "⚡ Optimize RAM Now" action button.
  - Background automation controls: auto-optimize every N minutes or when memory load exceeds N%.
  - Full System Tray support (`Shell_NotifyIconW`): minimize or close to tray, restore on double-click, and quick context menu.
- **Process Working Set Trimming**: Enumerates active process handles and calls `EmptyWorkingSet` to flush idle physical pages to the pagefile or drop unreferenced working sets.
- **Standby List & System Cache Purging**: Communicates directly with the Windows NT kernel through `NtSetSystemInformation` (`SystemMemoryListInformation` class 80) to purge the standby list (`SystemPurgeStandbyList = 3`) and flush system working sets (`SystemEmptyWorkingSetList = 2`).
- **Token Privilege Escalation**: Automatically requests and enables `SeDebugPrivilege`, `SeIncreaseQuotaPrivilege`, and `SeProfileSingleProcessPrivilege` on token creation.
- **Dual-Mode Execution Architecture**:
  - **No Arguments**: Automatically launches the modern Native Win32 GUI with real-time stats and tray support.
  - **CLI Flags**: Seamlessly executes in headless/terminal mode for scripts and automation.
    - `--once` : Instant one-shot optimization with before/after memory diff.
    - `--interval <mins>` : Periodic background optimization daemon.
    - `--threshold <pct>` : Intelligent trigger mode that cleans RAM only when memory pressure reaches a threshold percentage.
- **Portable & Tiny Footprint**: Compiles to a single, statically linked `.exe` (~320 KB) with runtime memory usage below 5-10 MB.

---

## 📊 Benchmark Comparison

| Metric | Ram-Optimizer (Rust) | C# / .NET Based Cleaners | C++ Alternatives |
| :--- | :---: | :---: | :---: |
| **Runtime Footprint** | **~5 - 12 MB RAM** | 40 - 120 MB RAM | 8 - 15 MB RAM |
| **Runtime Dependency** | **None (Native Machine Code)** | .NET Runtime / CLR | MSVC Redistributable |
| **Garbage Collector** | **Zero (Deterministic RAII)** | GC Pauses | None |
| **Binary Size** | **~320 KB** | 15 - 50 MB+ | ~2 - 5 MB |
| **Kernel FFI Calls** | **Direct `ntdll` / `psapi`** | P/Invoke overhead | Win32 / NT API |

---

## 🚀 Getting Started

### Prerequisites
- Windows 10, Windows 11, or Windows Server (x86_64).
- (Optional, for building from source) [Rust Toolchain](https://rustup.rs/) with `cargo`.
- **Administrator Privileges**: Recommended for flushing kernel-level standby lists and privileged system processes.

### Build from Source
```powershell
# Clone the repository
git clone https://github.com/GlitchWorlds/Ram-Optimizer.git
cd Ram-Optimizer

# Build release binary (LTO + Strip enabled)
cargo build --release

# The compiled binary will be located at target/release/ram-optimizer.exe
```

---

## 📖 Usage & Examples

### 1. Graphical User Interface (GUI Mode)
Launch the application without any command-line arguments:
```powershell
.\ram-optimizer.exe
```
This opens the native Win32 window with real-time memory meters, one-click optimization, background automation timers, and system tray minimization.

### 2. One-Shot Clean (CLI)
Clean all processes and standby lists immediately, view the freed memory, and exit:
```powershell
.\ram-optimizer.exe --once
```

### 3. Auto-Clean on Interval (Daemon)
Clean memory automatically every 15 minutes:
```powershell
.\ram-optimizer.exe --interval 15
```

### 4. Threshold-Based Auto-Clean
Watch memory load and automatically trigger optimization whenever usage exceeds 80%:
```powershell
.\ram-optimizer.exe --threshold 80 --interval 2
```

---

## 🛡️ Security & Integrity
This application does not terminate processes or destroy state. It strictly invokes documented and semi-documented Windows kernel memory management routines (`EmptyWorkingSet` and `NtSetSystemInformation`) that Windows itself exposes for system diagnostics and memory defragmentation.

---

## 📄 License
This project is licensed under the [MIT License](LICENSE).