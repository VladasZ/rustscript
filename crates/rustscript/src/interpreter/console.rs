//! Decoding the bytes a child process wrote.
//!
//! Everywhere except Windows this is UTF-8 and there is nothing to do. Windows
//! console tools write in the console output code page instead, cp437 or cp866
//! or whatever the machine is set to, so `from_utf8_lossy` turns every non
//! ASCII byte into a replacement character. A path with an accent in it, or a
//! localized message, comes back mangled and any parse of it then fails.
//!
//! UTF-8 is still tried first, because a growing number of tools emit it
//! regardless of the code page, and it is the right answer when it parses.

/// Decode child process output, falling back to the console code page when the
/// bytes are not valid UTF-8.
pub(super) fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => decode_native(bytes),
    }
}

#[cfg(windows)]
fn decode_native(bytes: &[u8]) -> String {
    use windows_sys::Win32::System::Console::GetConsoleOutputCP;

    // A detached process, a scheduled task or an ssh session, has no console
    // and GetConsoleOutputCP returns 0. cp1252 is the sane default there.
    let cp = match unsafe { GetConsoleOutputCP() } {
        0 => 1252,
        n => n,
    };
    let Some(encoding) = codepage_encoding(cp) else {
        return String::from_utf8_lossy(bytes).into_owned();
    };
    encoding.decode(bytes).0.into_owned()
}

#[cfg(not(windows))]
fn decode_native(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Map the code pages a Windows console actually gets set to onto the labels
/// encoding_rs knows. Anything else falls back to lossy UTF-8, which is no
/// worse than the behavior this replaced.
#[cfg(windows)]
fn codepage_encoding(cp: u32) -> Option<&'static encoding_rs::Encoding> {
    let label: &str = match cp {
        65001 => return Some(encoding_rs::UTF_8),
        437 | 850 | 858 | 1252 => "windows-1252",
        866 => "ibm866",
        1251 => "windows-1251",
        1250 => "windows-1250",
        1253 => "windows-1253",
        1254 => "windows-1254",
        1255 => "windows-1255",
        1256 => "windows-1256",
        1257 => "windows-1257",
        1258 => "windows-1258",
        932 => "shift_jis",
        936 => "gbk",
        949 => "euc-kr",
        950 => "big5",
        _ => return None,
    };
    encoding_rs::Encoding::for_label(label.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn plain_utf8_round_trips() {
        assert_eq!(decode("hello".as_bytes()), "hello");
        assert_eq!(decode("naïve".as_bytes()), "naïve");
    }

    /// The byte 0xE9 is not valid UTF-8. It must not become a replacement
    /// character, which is what the old lossy decode produced.
    #[cfg(windows)]
    #[test]
    fn non_utf8_falls_back_to_the_code_page() {
        assert!(!decode(&[b'c', b'a', b'f', 0xE9]).contains('\u{fffd}'));
    }
}
