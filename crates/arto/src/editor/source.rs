//! The text an editor shows, and the bytes it came from.
//!
//! A `<textarea>` only ever holds `\n`: whatever line endings are put into
//! it, its value comes back with each one turned into a line feed. A file
//! written on Windows would therefore come back from an edit with every line
//! changed, which is a diff of the whole document for a one-word fix. So the
//! editor is handed the text with `\n` line endings, and what the file used is
//! recorded here and put back on the way out.
//!
//! The same goes for a UTF-8 byte order mark: it is not text, so it is not
//! shown, and it is not lost either.

/// The line ending a file is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::Crlf => "CRLF",
        }
    }
}

/// How a file's text is laid out in bytes, beyond the text itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceFormat {
    /// Whether the file starts with a UTF-8 byte order mark.
    pub bom: bool,
    /// The line ending the file is written with. For a file that mixes them,
    /// the one most of its lines use.
    pub line_ending: LineEnding,
    /// Whether the file used more than one kind of line ending.
    ///
    /// Such a file cannot be written back byte for byte from a `\n`-only
    /// buffer — which lines had which ending is exactly what the buffer
    /// forgets — so saving it settles on [`Self::line_ending`], and the
    /// editor says so before it happens.
    pub mixed: bool,
}

const BOM: &str = "\u{feff}";

/// Split a file's text into what the editor shows and how to write it back.
///
/// The returned text has `\n` line endings only, and no byte order mark.
pub fn decode(raw: &str) -> (String, SourceFormat) {
    let (bom, body) = match raw.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, raw),
    };

    let bytes = body.as_bytes();
    let (mut crlf, mut lf, mut cr) = (0usize, 0usize, 0usize);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                crlf += 1;
                i += 1;
            }
            b'\r' => cr += 1,
            b'\n' => lf += 1,
            _ => {}
        }
        i += 1;
    }

    let kinds = [crlf, lf, cr].iter().filter(|&&n| n > 0).count();
    let line_ending = if crlf > lf + cr {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    };
    let text = if crlf == 0 && cr == 0 {
        body.to_string()
    } else {
        to_lf(body)
    };

    (
        text,
        SourceFormat {
            bom,
            line_ending,
            mixed: kinds > 1 || cr > 0,
        },
    )
}

/// The bytes to write for `text` in `format`.
///
/// `text` is expected to hold `\n` only, which is what the editor produces;
/// anything else is folded to `\n` first so that a stray `\r` cannot become a
/// `\r\r\n`.
pub fn encode(text: &str, format: &SourceFormat) -> Vec<u8> {
    let text = if text.contains('\r') {
        std::borrow::Cow::Owned(to_lf(text))
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    let mut out = String::with_capacity(text.len() + text.len() / 32 + BOM.len());
    if format.bom {
        out.push_str(BOM);
    }
    match format.line_ending {
        LineEnding::Lf => out.push_str(&text),
        LineEnding::Crlf => out.push_str(&text.replace('\n', "\r\n")),
    }
    out.into_bytes()
}

fn to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    fn round_trip(raw: &str) -> Vec<u8> {
        let (text, format) = decode(raw);
        encode(&text, &format)
    }

    #[test]
    fn a_plain_file_comes_back_byte_for_byte() {
        let raw = indoc! {"
            # Design

            Some *text*.
        "};
        let (text, format) = decode(raw);
        assert_eq!(text, raw);
        assert_eq!(format, SourceFormat::default());
        assert_eq!(round_trip(raw), raw.as_bytes());
    }

    #[test]
    fn crlf_is_hidden_from_the_editor_and_restored_on_save() {
        let raw = "# Title\r\n\r\nBody\r\n";
        let (text, format) = decode(raw);
        assert_eq!(text, "# Title\n\nBody\n");
        assert_eq!(format.line_ending, LineEnding::Crlf);
        assert!(!format.mixed);
        assert_eq!(round_trip(raw), raw.as_bytes());
    }

    #[test]
    fn a_byte_order_mark_is_kept_but_not_shown() {
        let raw = "\u{feff}# Title\n";
        let (text, format) = decode(raw);
        assert_eq!(text, "# Title\n");
        assert!(format.bom);
        assert_eq!(round_trip(raw), raw.as_bytes());
    }

    #[test]
    fn bom_and_crlf_together_survive() {
        let raw = "\u{feff}a\r\nb\r\n";
        assert_eq!(round_trip(raw), raw.as_bytes());
    }

    #[test]
    fn a_missing_final_newline_stays_missing() {
        assert_eq!(round_trip("a\r\nb"), b"a\r\nb");
        assert_eq!(round_trip("a\nb"), b"a\nb");
    }

    #[test]
    fn an_empty_file_is_an_empty_file() {
        let (text, format) = decode("");
        assert_eq!(text, "");
        assert_eq!(format, SourceFormat::default());
        assert!(round_trip("").is_empty());
    }

    #[test]
    fn mixed_endings_are_reported_and_settle_on_the_majority() {
        let (text, format) = decode("a\r\nb\r\nc\nd\r\n");
        assert_eq!(text, "a\nb\nc\nd\n");
        assert!(format.mixed);
        assert_eq!(format.line_ending, LineEnding::Crlf);

        let (_, format) = decode("a\nb\nc\r\n");
        assert!(format.mixed);
        assert_eq!(format.line_ending, LineEnding::Lf);
    }

    #[test]
    fn a_lone_carriage_return_is_a_line_ending_too() {
        let (text, format) = decode("a\rb\n");
        assert_eq!(text, "a\nb\n");
        assert!(format.mixed);
    }

    #[test]
    fn non_ascii_text_is_untouched() {
        let raw = "# 設計書\r\n\r\n日本語の本文 📝\r\n";
        assert_eq!(round_trip(raw), raw.as_bytes());
    }

    #[test]
    fn a_stray_carriage_return_in_the_buffer_does_not_double_up() {
        let format = SourceFormat {
            line_ending: LineEnding::Crlf,
            ..Default::default()
        };
        assert_eq!(encode("a\r\nb\n", &format), b"a\r\nb\r\n");
    }
}
