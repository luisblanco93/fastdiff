use crate::{Error, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{Read, Write};

pub const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<()> {
    let payload = serde_json::to_vec(value)?;
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err(Error::Protocol(format!(
            "Message exceeds {MAX_MESSAGE_BYTES} bytes; open a smaller document or comparison"
        )));
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<Option<T>> {
    let mut prefix = [0u8; 4];
    // A clean EOF is allowed only between frames, never within a prefix or payload.
    match reader.read(&mut prefix[..1]) {
        Ok(0) => return Ok(None),
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => return read_frame(reader),
        Err(error) => return Err(error.into()),
    }
    reader.read_exact(&mut prefix[1..])?;
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > MAX_MESSAGE_BYTES {
        return Err(Error::Protocol(format!(
            "Invalid frame length {length}; maximum is {MAX_MESSAGE_BYTES}"
        )));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    Ok(Some(serde_json::from_slice(&payload)?))
}
