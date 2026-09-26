# QC Checklist — RAM Optimizer Tray Exit (bug fix v1.3.4 / commit ce27e04)

## Bug ringkas
- TrackPopupMenu tanpa TPM_RETURNCMD: return selalu 0, tidak pernah sama dengan IDM_TRAY_EXIT (3003) sehingga Exit tidak jalan.
- Menu modal blokir message loop tanpa PostMessage WM_NULL setelah SetForegroundWindow.
- Fix: flags TPM_RIGHTBUTTON|TPM_LEFTALIGN|TPM_BOTTOMALIGN|TPM_RETURNCMD|TPM_NONOTIFY + PostMessageW(hwnd, WM_NULL) + cmd==IDM_TRAY_EXIT => DestroyWindow.

## Checklist tray (berlaku ram-optimizer + ram-optimizer-selfdestruct, satu gui.rs bersama)
- [x] 1. TrackPopupMenu pakai TPM_RETURNCMD + TPM_NONOTIFY (gui.rs ~L988).
- [x] 2. SetForegroundWindow(hwnd) sebelum TrackPopupMenu.
- [x] 3. PostMessageW(hwnd, WM_NULL, 0, 0) segera setelah TrackPopupMenu + DestroyMenu.
- [x] 4. IDM_TRAY_EXIT (3003) reachable: AppendMenuW Exit + cabang cmd==IDM_TRAY_EXIT => DestroyWindow(hwnd).
- [x] 5. WM_CLOSE => DestroyWindow(hwnd) (tombol X / taskkill graceful keluar via WM_CLOSE).
- [x] 6. WM_DESTROY: KillTimer(ID_TIMER_TICK) + remove_tray_icon (non-selfdestruct) + cleanup GDI/Box + PostQuitMessage(0).
- [x] 7. remove_tray_icon pakai NIM_DELETE (uID=1).
- [x] 8. Exit terminate proses: GetMessageW loop keluar setelah PostQuitMessage; tasklist hilang.
- [x] 9. Icon hilang: NIM_DELETE di WM_DESTROY + di akhir run_gui (loop exit path).
- [x] 10. No hang: WM_NULL pelepas menu modal; static grep PostMessageW(hwnd, WM_NULL) ada.
- [x] 11. Selfdestruct berbagi gui.rs: IS_SELF_DESTRUCT skip NIM_ADD/NIM_DELETE, WM_DESTROY/WM_QUERYENDSESSION/WM_ENDSESSION => trigger_self_destruct; live-run selfdestruct SENGAJA diskip agar watchdog tidak hapus binary debug.

## Functional test (2026-09-26, host Windows_NT 10.0.26200)
1. cargo check => Finished dev profile (ok).
2. cargo build => Finished dev profile, ram-optimizer.exe 5936173 B + ram-optimizer-selfdestruct.exe 5960437 B (ok).
3. cargo test => 6 passed (main) + 6 passed (selfdestruct), 0 failed (ok).
4. Static grep => TPM_RETURNCMD/TPM_NONOTIFY/WM_NULL/IDM_TRAY_EXIT/KillTimer/PostQuitMessage/remove_tray_icon semua ada (ok).
5. Lifecycle ram-optimizer.exe --minimized (bg session vivid-kelp): tasklist PID 41940 Mem ~10.5MB (hidup, ok).
6. taskkill /PID 41940 (graceful WM_CLOSE) => SUCCESS, tasklist kosong "No tasks are running" (exit terminate, ok).
7. Tray-menu klik kanan Exit tidak bisa diotomasi headless (butuh cursor/user32 interaktif); diwakili static path cmd==IDM_TRAY_EXIT=>DestroyWindow + WM_CLOSE path yang sama-sama bermuara ke WM_DESTROY (partial, jujur).
8. Selfdestruct live-run diskip (static only) demi keamanan binary (partial, jujur).

## Evaluasi root-cause (kenapa lolos)
- Tidak ada checklist Win32 tray: TPM_RETURNCMD vs TPM_NONOTIFY vs WM_NULL adalah syarat klasik MSDN yang tidak tertulis di definisi selesai.
- Tidak ada test exit-path: 6 unit test hanya config/to_wide/registry, nol test menu/WM_CLOSE/WM_DESTROY.
- Review terlewat: diff b41a6dc (minimize-to-tray) tidak menuntut bukti klik Exit + tasklist hilang sebelum merge.
- Satu gui.rs dipakai dua binary membuat asumsi "fix satu = aman dua" tanpa verifikasi ganda.

## Guardrail permanen
1. Checklist tray wajib tiap release (11 poin di atas), dilampirkan sebagai evidence.
2. Test exit tiap release: cargo test + launch --minimized => tasklist ada => taskkill => tasklist hilang (timeout, catat PID).
3. Larang klaim done tanpa functional lifecycle; tray-klik manual dicatat bila tidak bisa otomatis.
4. Selfdestruct hanya static-check + review diff gui.rs, dilarang live-run di target/ tanpa kopian.
5. Tag release wajib sebut commit fix tray bila menyentuh TrackPopupMenu/window_proc.
