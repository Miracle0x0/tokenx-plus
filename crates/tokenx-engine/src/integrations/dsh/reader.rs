//! Stream one physical snapshot without materializing the decoded transcript.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use crate::records::error::{SessionParseError, SessionParseResult};

pub(super) const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

pub(super) struct Transcript {
    input: Box<dyn BufRead>,
    path: PathBuf,
    operation: &'static str,
    line: Vec<u8>,
    first: bool,
    finished: bool,
}

impl Transcript {
    pub(super) fn open(path: &Path) -> SessionParseResult<Self> {
        let file = File::open(path)
            .map_err(|error| SessionParseError::at_path(path, "read DSH session", error))?;
        let length = file
            .metadata()
            .map_err(|error| SessionParseError::at_path(path, "inspect DSH session", error))?
            .len();
        let mut input = BufReader::new(file.take(length));
        let compressed = input
            .fill_buf()
            .map_err(|error| SessionParseError::at_path(path, "read DSH session", error))?
            .starts_with(&ZSTD_MAGIC);
        let operation = if compressed {
            "decode DSH zstd stream"
        } else {
            "read DSH session"
        };
        let input: Box<dyn BufRead> = if compressed {
            let decoder = zstd::stream::read::Decoder::with_buffer(input)
                .map_err(|error| SessionParseError::at_path(path, operation, error))?;
            Box::new(BufReader::new(decoder))
        } else {
            Box::new(input)
        };
        Ok(Self {
            input,
            path: path.to_owned(),
            operation,
            line: Vec::new(),
            first: true,
            finished: false,
        })
    }
}

impl Iterator for Transcript {
    type Item = SessionParseResult<String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        self.line.clear();
        match self.input.read_until(b'\n', &mut self.line) {
            Ok(0) => {
                self.finished = true;
                None
            }
            Ok(_) => {
                let bytes = if std::mem::take(&mut self.first) {
                    self.line
                        .strip_prefix(b"\xef\xbb\xbf")
                        .unwrap_or(&self.line)
                } else {
                    &self.line
                };
                Some(Ok(String::from_utf8_lossy(bytes).into_owned()))
            }
            Err(error) => {
                self.finished = true;
                Some(Err(SessionParseError::at_path(
                    &self.path,
                    self.operation,
                    error,
                )))
            }
        }
    }
}
