//! Reconcile Claude duplicates before pricing without retaining all records.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};

use serde::{Deserialize, Serialize};

use crate::records::UsageRecord;
use crate::TokenBreakdown;

pub(super) fn merge_input(
    input: &mut i64,
    is_split: &mut bool,
    incoming: i64,
    incoming_is_split: bool,
) {
    match (*is_split, incoming_is_split) {
        (false, true) => *input = incoming,
        (true, false) => {}
        _ => *input = (*input).max(incoming),
    }
    *is_split |= incoming_is_split;
}

// This prefix has a fixed bincode layout: six i64 counters and one bool.
// Only the prefix is overwritten; the original record identity stays intact.
const USAGE_BYTES: usize = 6 * 8 + 1;

#[derive(Serialize, Deserialize)]
struct MergeUsage {
    tokens: TokenBreakdown,
    input_is_split: bool,
}

impl MergeUsage {
    fn read_from(reader: &mut impl Read) -> io::Result<Self> {
        let mut bytes = [0; USAGE_BYTES];
        reader.read_exact(&mut bytes)?;
        bincode::deserialize(&bytes).map_err(codec_error)
    }

    fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        let mut bytes = [0; USAGE_BYTES];
        bincode::serialize_into(bytes.as_mut_slice(), self).map_err(codec_error)?;
        writer.write_all(&bytes)
    }

    fn from_record(record: &UsageRecord) -> Self {
        Self {
            tokens: record.tokens.clone(),
            input_is_split: record.claude_input_is_split,
        }
    }

    fn merge(&mut self, record: &UsageRecord) {
        merge_input(
            &mut self.tokens.input,
            &mut self.input_is_split,
            record.tokens.input,
            record.claude_input_is_split,
        );
        self.tokens.output = self.tokens.output.max(record.tokens.output);
        self.tokens.cache_read = self.tokens.cache_read.max(record.tokens.cache_read);
        self.tokens.cache_write = self.tokens.cache_write.max(record.tokens.cache_write);
        self.tokens.cache_write_1h = self.tokens.cache_write_1h.max(record.tokens.cache_write_1h);
        self.tokens.reasoning = self.tokens.reasoning.max(record.tokens.reasoning);
    }
}

fn codec_error(error: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

/// Owns an anonymous private temporary file, deleted on close. The in-memory
/// index contains only dedup keys and offsets, across all acquisition batches.
pub(super) struct ClaudeMerge {
    writer: BufWriter<File>,
    offsets: HashMap<u64, u64>,
    end: u64,
    records: usize,
}

impl ClaudeMerge {
    pub(super) fn new() -> io::Result<Self> {
        Ok(Self {
            writer: BufWriter::new(tempfile::tempfile()?),
            offsets: HashMap::new(),
            end: 0,
            records: 0,
        })
    }

    pub(super) fn push(&mut self, input_index: usize, record: UsageRecord) -> io::Result<()> {
        if let Some(offset) = record.dedup_key.and_then(|key| self.offsets.get(&key)) {
            self.writer.flush()?;
            let file = self.writer.get_mut();
            file.seek(SeekFrom::Start(*offset))?;
            let mut usage = MergeUsage::read_from(file)?;
            usage.merge(&record);
            file.seek(SeekFrom::Start(*offset))?;
            usage.write_to(file)?;
            file.seek(SeekFrom::Start(self.end))?;
            return Ok(());
        }

        let usage = MergeUsage::from_record(&record);
        let entry = (input_index, &record);
        usage.write_to(&mut self.writer)?;
        bincode::serialize_into(&mut self.writer, &entry).map_err(codec_error)?;
        if let Some(key) = record.dedup_key {
            self.offsets.insert(key, self.end);
        }
        self.end += USAGE_BYTES as u64 + bincode::serialized_size(&entry).map_err(codec_error)?;
        self.records += 1;
        Ok(())
    }

    pub(super) fn into_records(self) -> io::Result<MergedRecords> {
        let mut file = self
            .writer
            .into_inner()
            .map_err(|error| error.into_error())?;
        file.rewind()?;
        Ok(MergedRecords {
            reader: BufReader::new(file),
            remaining: self.records,
        })
    }
}

pub(super) struct MergedRecords {
    reader: BufReader<File>,
    remaining: usize,
}

impl Iterator for MergedRecords {
    type Item = io::Result<(usize, UsageRecord)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some((|| {
            let usage = MergeUsage::read_from(&mut self.reader)?;
            let (input_index, mut record): (usize, UsageRecord) =
                bincode::deserialize_from(&mut self.reader).map_err(codec_error)?;
            record.tokens = usage.tokens;
            record.claude_input_is_split = usage.input_is_split;
            Ok((input_index, record))
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(key: Option<u64>, input: i64, split: bool) -> UsageRecord {
        let mut record = UsageRecord::new_with_dedup(
            "claude-sonnet-4.6",
            "anthropic",
            "session",
            1,
            TokenBreakdown {
                input,
                output: 1,
                ..Default::default()
            },
            0.0,
            key,
        );
        record.claude_input_is_split = split;
        record
    }

    #[test]
    fn duplicate_updates_preserve_neighbors_order_and_unkeyed_records() {
        let mut merge = ClaudeMerge::new().unwrap();
        for key in 0..130 {
            merge.push(0, record(Some(key), 100, false)).unwrap();
        }
        merge.push(1, record(Some(0), 0, true)).unwrap();
        merge.push(1, record(Some(64), 2, true)).unwrap();
        merge.push(1, record(Some(129), 3, true)).unwrap();
        merge.push(1, record(None, 4, false)).unwrap();
        merge.push(1, record(None, 5, false)).unwrap();
        let records = merge
            .into_records()
            .unwrap()
            .collect::<io::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(records.len(), 132);
        for (key, (owner, record)) in records[..130].iter().enumerate() {
            assert_eq!(*owner, 0);
            assert_eq!(record.dedup_key, Some(key as u64));
            assert_eq!(
                record.tokens.input,
                match key {
                    0 => 0,
                    64 => 2,
                    129 => 3,
                    _ => 100,
                }
            );
        }
        assert_eq!(records[130].0, 1);
        assert_eq!(records[130].1.tokens.input, 4);
        assert_eq!(records[131].1.tokens.input, 5);
    }

    #[test]
    fn truncated_merge_storage_is_an_explicit_error() {
        let mut merge = ClaudeMerge::new().unwrap();
        merge.push(0, record(Some(1), 100, false)).unwrap();
        merge.writer.flush().unwrap();
        merge.writer.get_mut().set_len(10).unwrap();
        assert!(merge.into_records().unwrap().next().unwrap().is_err());
    }
}
