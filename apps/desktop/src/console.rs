//! Release builds on Windows are windowed programs (no console window
//! pops up next to Wall when it starts from the Start menu). When started
//! from a terminal, `attach` borrows that terminal so `wall: …` lines still
//! show. Output that is redirected (`wall.exe > log.txt`, or a pipe) works
//! as before. A windowed program doesn't make the terminal wait: scripts
//! that need to wait use `Start-Process -Wait` or pipe the output
//! (`.\wall.exe | Out-Host`).

#[cfg(all(windows, not(debug_assertions)))]
pub fn attach() {
    use std::os::windows::io::IntoRawHandle;
    type Handle = *mut core::ffi::c_void;
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(pid: u32) -> i32;
        fn GetStdHandle(which: u32) -> Handle;
        fn SetStdHandle(which: u32, handle: Handle) -> i32;
    }
    unsafe {
        let unset = |h: Handle| h.is_null() || h as isize == -1;
        // Already redirected: keep it.
        if !unset(GetStdHandle(STD_OUTPUT_HANDLE)) && !unset(GetStdHandle(STD_ERROR_HANDLE)) {
            return;
        }
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return; // started from Explorer or the Start menu: no terminal
        }
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            if unset(GetStdHandle(which)) {
                if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                    SetStdHandle(which, f.into_raw_handle() as Handle);
                }
            }
        }
    }
}

#[cfg(not(all(windows, not(debug_assertions))))]
pub fn attach() {}
