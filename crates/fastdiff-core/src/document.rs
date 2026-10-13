use crate::{CancelToken, Error, MAX_DIFF_ROWS, MAX_SOURCE_BYTES, Result};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocument {
    pub path: String,
    pub text: String,
}

/// Read the requested name each time so atomic saves and disappearance are observed.
pub fn read_document(path: impl AsRef<Path>, cancel: &CancelToken) -> Result<TextDocument> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let path = path.as_ref();
    if !std::fs::metadata(path)?.is_file() {
        return Err(Error::Document(
            "Document path is not a regular file".into(),
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // An atomic replacement with a FIFO must not block the loader.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::Document(
            "Document path is not a regular file".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let text = decode_document(&bytes)?;
    Ok(TextDocument {
        path: std::fs::canonicalize(path)?.to_string_lossy().into_owned(),
        text,
    })
}

fn decode_document(bytes: &[u8]) -> Result<String> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(Error::Document(
            "Document exceeds the 10 MiB loading limit".into(),
        ));
    }
    let encoding_error =
        || Error::Document("Unsupported document encoding; use UTF-8 or BOM-marked UTF-16".into());
    let text = if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let little_endian = bytes[0] == 0xff;
        if !(bytes.len() - 2).is_multiple_of(2) {
            return Err(encoding_error());
        }
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| {
                let pair = [pair[0], pair[1]];
                if little_endian {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            })
            .collect::<Vec<_>>();
        String::from_utf16(&units).map_err(|_| encoding_error())?
    } else {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        if bytes.contains(&0) {
            return Err(Error::Document(
                "Binary content cannot be viewed as a text document".into(),
            ));
        }
        String::from_utf8(bytes.to_vec()).map_err(|_| encoding_error())?
    };
    if text.contains('\0') {
        return Err(Error::Document(
            "Binary content cannot be viewed as a text document".into(),
        ));
    }
    if text.len() > MAX_SOURCE_BYTES {
        return Err(Error::Document(
            "Decoded document exceeds the 10 MiB loading limit".into(),
        ));
    }
    if text.lines().count() > MAX_DIFF_ROWS {
        return Err(Error::Document(
            "Document exceeds the 100,000-line loading limit".into(),
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_encodings_and_binary_rejection() {
        for bytes in [b"hello\n".as_slice(), b"\xef\xbb\xbfhello\n"] {
            assert_eq!(decode_document(bytes).unwrap(), "hello\n");
        }
        assert_eq!(
            decode_document(&[0xff, 0xfe, b'A', 0, 0xac, 0x20]).unwrap(),
            "A€"
        );
        assert_eq!(
            decode_document(&[0xfe, 0xff, 0, b'A', 0x20, 0xac]).unwrap(),
            "A€"
        );
        assert!(
            decode_document(b"a\0b")
                .unwrap_err()
                .to_string()
                .contains("Binary")
        );
        assert!(
            decode_document(&[0xff])
                .unwrap_err()
                .to_string()
                .contains("encoding")
        );
        assert!(decode_document(&[0xff, 0xfe, 0]).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_fifo_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pipe");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a valid NUL-terminated path in an owned temporary directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(
            read_document(path, &CancelToken::default())
                .unwrap_err()
                .to_string()
                .contains("regular file")
        );
    }
    #[test]
    fn limits_are_explicit() {
        assert!(
            decode_document(&vec![b'x'; MAX_SOURCE_BYTES + 1])
                .unwrap_err()
                .to_string()
                .contains("10 MiB")
        );
        assert!(
            decode_document("x\n".repeat(MAX_DIFF_ROWS + 1).as_bytes())
                .unwrap_err()
                .to_string()
                .contains("100,000")
        );
        assert!(decode_document("x\n".repeat(MAX_DIFF_ROWS).as_bytes()).is_ok());
    }
    #[test]
    fn rereads_replacement_and_reports_disappearance() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document.txt");
        std::fs::write(&path, "first").unwrap();
        let token = CancelToken::default();
        assert_eq!(read_document(&path, &token).unwrap().text, "first");
        let replacement = directory.path().join("replacement");
        std::fs::write(&replacement, "second").unwrap();
        #[cfg(windows)]
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert_eq!(read_document(&path, &token).unwrap().text, "second");
        std::fs::remove_file(path.clone()).unwrap();
        assert!(read_document(&path, &token).is_err());
        token.cancel();
        assert!(matches!(read_document(path, &token), Err(Error::Cancelled)));
    }
}
