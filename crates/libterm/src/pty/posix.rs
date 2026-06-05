// POSIX PTY stub — Phase 2/3 (macOS + Linux).
use super::Pty;

pub struct PosixPty;

impl Pty for PosixPty {
    fn write(&mut self, _data: &[u8]) -> anyhow::Result<()> {
        unimplemented!("POSIX PTY is Phase 2")
    }
    fn resize(&mut self, _cols: u16, _rows: u16) -> anyhow::Result<()> {
        unimplemented!("POSIX PTY is Phase 2")
    }
}
