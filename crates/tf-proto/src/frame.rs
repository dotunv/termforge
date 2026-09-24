use serde::{de::DeserializeOwned, Serialize};
use std::io::{self, Read, Write};

/// Upper bound on a single frame. Output events are chunked well below this.
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
    #[error("frame of {0} bytes exceeds limit of {MAX_FRAME_LEN}")]
    TooLarge(usize),
    #[error("encode/decode: {0}")]
    Codec(#[from] postcard::Error),
}

/// Encode `msg` and write it as one length-prefixed frame using a single
/// `write_all` call.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), FrameError> {
    // Encode after a 4-byte placeholder so header and payload go out together.
    let mut buf = postcard::to_extend(msg, vec![0u8; 4])?;
    let payload_len = buf.len() - 4;
    if payload_len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(payload_len));
    }
    // Cannot truncate: MAX_FRAME_LEN fits in u32.
    buf[..4].copy_from_slice(&(payload_len as u32).to_le_bytes());
    w.write_all(&buf)?;
    Ok(())
}

/// Read one frame. Returns `Ok(None)` on a clean EOF at a frame boundary.
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<Option<T>, FrameError> {
    let mut header = [0u8; 4];
    match r.read_exact(&mut header) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(header) as usize;
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok(Some(postcard::from_bytes(&payload)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_header() {
        let bytes = ((MAX_FRAME_LEN as u32) + 1).to_le_bytes();
        let r: Result<Option<u8>, _> = read_frame(&mut bytes.as_slice());
        assert!(matches!(r, Err(FrameError::TooLarge(_))));
    }

    #[test]
    fn clean_eof_is_none() {
        let r: Option<u8> = read_frame(&mut [].as_slice()).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn truncated_payload_is_error() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &"hello".to_string()).unwrap();
        buf.pop();
        let r: Result<Option<String>, _> = read_frame(&mut buf.as_slice());
        assert!(r.is_err());
    }

    #[test]
    fn many_frames_back_to_back() {
        let mut buf = Vec::new();
        for i in 0..100u32 {
            write_frame(&mut buf, &i).unwrap();
        }
        let mut r = buf.as_slice();
        for i in 0..100u32 {
            assert_eq!(read_frame::<_, u32>(&mut r).unwrap(), Some(i));
        }
        assert_eq!(read_frame::<_, u32>(&mut r).unwrap(), None);
    }
}
