use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::ReadFile;
use windows::Win32::Storage::FileSystem::WriteFile;
use windows::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, InitializeProcThreadAttributeList, UpdateProcThreadAttribute,
    WaitForSingleObject, EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTUPINFOEXW,
};

use super::Pty;

/// Windows ConPTY-backed PTY session.
///
/// SAFETY: HANDLE is a raw pointer typedef, but ConPTY handles are safe to
/// send across threads because they are reference-counted OS objects.
pub struct ConPty {
    hpcon: HPCON,
    write_pipe: HANDLE,
    process_info: PROCESS_INFORMATION,
}

// SAFETY: Windows kernel objects (HANDLE) are reference-counted and thread-safe
// when accessed via the documented Win32 API (WriteFile, CloseHandle, etc.).
unsafe impl Send for ConPty {}

impl ConPty {
    /// Spawn `command` inside a ConPTY of size `cols`×`rows`.
    /// `output_tx` receives raw output bytes from the child process.
    pub fn spawn(
        command: &str,
        cols: u16,
        rows: u16,
        output_tx: mpsc::UnboundedSender<Vec<u8>>,
    ) -> Result<Self> {
        unsafe {
            // ── Pipe pair: child stdout → our reader ──────────────────────────
            let mut pty_read = HANDLE::default();   // we read from this
            let mut pty_write = HANDLE::default();  // child writes to this (via ConPTY)
            CreatePipe(&mut pty_read, &mut pty_write, None, 0)
                .context("CreatePipe (output)")?;

            // ── Pipe pair: our writer → child stdin ───────────────────────────
            let mut shell_read = HANDLE::default();  // child reads (via ConPTY)
            let mut shell_write = HANDLE::default(); // we write to this
            CreatePipe(&mut shell_read, &mut shell_write, None, 0)
                .context("CreatePipe (input)")?;

            // ── Create the pseudo-console ─────────────────────────────────────
            let size = COORD { X: cols as i16, Y: rows as i16 };
            // ConPTY takes ownership of shell_read (input) and pty_write (output)
            let hpcon = CreatePseudoConsole(size, shell_read, pty_write, 0)
                .context("CreatePseudoConsole")?;

            // The ConPTY now owns these ends — close our copies.
            CloseHandle(shell_read).ok();
            CloseHandle(pty_write).ok();

            // ── Async reader task ─────────────────────────────────────────────
            // Send raw isize (handle value) across the thread boundary.
            let read_handle_raw = pty_read.0 as isize;
            tokio::task::spawn_blocking(move || {
                let handle = HANDLE(read_handle_raw as *mut std::ffi::c_void);
                let mut buf = vec![0u8; 4096];
                loop {
                    let mut read = 0u32;
                    let ok = ReadFile(handle, Some(&mut buf), Some(&mut read), None);
                    if ok.is_err() || read == 0 {
                        break;
                    }
                    if output_tx.send(buf[..read as usize].to_vec()).is_err() {
                        break;
                    }
                }
                // Close read end when done
                let _ = CloseHandle(handle);
            });

            // ── Build STARTUPINFOEX with the ConPTY attribute ─────────────────
            // First call: query required buffer size
            let mut attr_size = 0usize;
            let _ = InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST::default(),
                1,
                0,
                &mut attr_size,
            );

            let mut attr_buf = vec![0u8; attr_size];
            let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut _);

            // Second call: initialise the list in our buffer
            InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_size)
                .context("InitializeProcThreadAttributeList")?;

            // Add the ConPTY attribute
            let hpcon_val = hpcon.0 as usize;
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(&hpcon_val as *const usize as *const std::ffi::c_void),
                std::mem::size_of::<usize>(),
                None,
                None,
            )
            .context("UpdateProcThreadAttribute")?;

            // ── Spawn the child process ───────────────────────────────────────
            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = attr_list;

            let mut cmd_wide: Vec<u16> = OsStr::new(command)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let mut process_info = PROCESS_INFORMATION::default();
            CreateProcessW(
                None,
                windows::core::PWSTR(cmd_wide.as_mut_ptr()),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                None,
                &si.StartupInfo,
                &mut process_info,
            )
            .context("CreateProcessW")?;

            Ok(Self { hpcon, write_pipe: shell_write, process_info })
        }
    }

    /// Await child process exit (non-blocking wrapper).
    pub async fn wait(&self) {
        let h = self.process_info.hProcess.0 as isize;
        tokio::task::spawn_blocking(move || unsafe {
            WaitForSingleObject(HANDLE(h as *mut std::ffi::c_void), u32::MAX);
        })
        .await
        .ok();
    }
}

impl Pty for ConPty {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        unsafe {
            let mut written = 0u32;
            WriteFile(self.write_pipe, Some(data), Some(&mut written), None)
                .context("WriteFile to ConPTY")?;
        }
        Ok(())
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        unsafe {
            ResizePseudoConsole(self.hpcon, COORD { X: cols as i16, Y: rows as i16 })
                .context("ResizePseudoConsole")?;
        }
        Ok(())
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        unsafe {
            ClosePseudoConsole(self.hpcon);
            CloseHandle(self.write_pipe).ok();
            CloseHandle(self.process_info.hProcess).ok();
            CloseHandle(self.process_info.hThread).ok();
        }
    }
}
