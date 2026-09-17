# ⚡ Ram-Optimizer

> **Ultra-Lightweight, Blazing-Fast Windows RAM Optimizer written in Pure Rust using Native Win32 & NT Kernel FFI.**

[![Rust](https://img.shields.io/badge/Rust-1.97%2B-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%2010%20%2F%2011%20%2F%20Server-lightgrey.svg?style=flat-square&logo=windows)](https://microsoft.com/windows)

**Ram-Optimizer** is an open-source, zero-cost abstraction alternative to proprietary memory cleaners like Wise Memory Optimizer or CleanMem. Built specifically for Windows systems, it communicates directly with low-level Windows NT subsystems without heavy frameworks, .NET runtime dependencies, or garbage collectors.

---

## ✨ Features

- **Process Working Set Trimming**: Enumerates active process handles and calls `EmptyWorkingSet` to flush idle physical pages to the pagefile or drop unreferenced working sets.
- **Standby List & System Cache Purging**: Communicates directly with the Windows NT kernel through `NtSetSystemInformation` (`SystemMemoryListInformation` class 80) to purge the standby list (`SystemPurgeStandbyList = 3`) and flush system working sets (`SystemEmptyWorkingSetList = 2`).
- **Token Privilege Escalation**: Automatically requests and enables `SeDebugPrivilege`, `SeIncreaseQuotaPrivilege`, and `SeProfileSingleProcessPrivilege` on token creation.
- **Real-Time Memory Metrics**: Reads live physical RAM usage and workload percentages directly via `GlobalMemoryStatusEx`.
- **Multiple Execution Modes**:
  - `--once` : Instant one-shot optimization with before/after memory diff.
  - `--interval <mins>` : Periodic background optimization daemon.
  - `--threshold <pct>` : Intelligent trigger mode that cleans RAM only when memory pressure reaches a threshold percentage.
  - **Interactive Console UI** : Live status dashboard with instant action shortcuts when launched without arguments.
- **Portable & Tiny Footprint**: Compiles to a single, statically linked `.exe` under 2 MB with runtime memory usage below 5 MB.

---

## 📊 Benchmark Comparison

| Metric | Ram-Optimizer (Rust) | C# / .NET Based Cleaners | C++ Alternatives |
| :--- | :---: | :---: | :---: |
| **Runtime Footprint** | **~3 - 5 MB RAM** | 40 - 120 MB RAM | 8 - 15 MB RAM |
| **Runtime Dependency** | **None (Native Machine Code)** | .NET Runtime / CLR | MSVC Redistributable |
| **Garbage Collector** | **Zero (Deterministic RAII)** | GC Pauses | None |
| **Binary Size** | **~1.5 MB** | 15 - 50 MB+ | ~2 - 5 MB |
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

### 1. One-Shot Clean
Clean all processes and standby lists immediately, view the freed memory, and exit:
```powershell
.\ram-optimizer.exe --once
```

### 2. Auto-Clean on Interval (Daemon)
Clean memory automatically every 15 minutes:
```powershell
.\ram-optimizer.exe --interval 15
```

### 3. Threshold-Based Auto-Clean
Watch memory load and automatically trigger optimization whenever usage exceeds 80%:
```powershell
.\ram-optimizer.exe --threshold 80 --interval 2
```

### 4. Interactive Console Mode
Simply run the executable without flags:
```powershell
.\ram-optimizer.exe
```

---

## 🛡️ Security & Integrity
This application does not terminate processes or destroy state. It strictly invokes documented and semi-documented Windows kernel memory management routines (`EmptyWorkingSet` and `NtSetSystemInformation`) that Windows itself exposes for system diagnostics and memory defragmentation.

---

## 📄 License
This project is licensed under the [MIT License](LICENSE).
