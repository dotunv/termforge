#[cfg(windows)]
pub mod conpty;
#[cfg(unix)]
pub mod posix;

/// Common trait for PTY implementations.
pub trait Pty: Send {
    /// Write bytes to the PTY input (as if typed by the user).
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()>;
    /// Resize the PTY.
    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()>;
    /// Suspend the child process (freeze CPU).  Default: no-op.
    fn suspend(&mut self) {}
    /// Resume a previously suspended child process.  Default: no-op.
    fn resume(&mut self) {}
}
