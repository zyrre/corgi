//! Reading the records of a session file: back from its end, from where the
//! last read stopped, and forward from its head.

use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use serde_json::Value;

/// What a scan collects from the records of a session file.
///
/// A fold is made from one record at a time and combined newest first, so a
/// scan can run in either direction: back from the end of a file until the
/// fold is complete, or over the records appended since the last scan, whose
/// fold then takes precedence over what was already known.
pub(super) trait Fold: Default {
    /// Both folds together, `self` holding the newer records.
    fn or(self, older: Self) -> Self;
    /// Whether older records can add nothing.
    fn complete(&self) -> bool;
}

/// A fold over a session file, and how far into the file it has read.
#[derive(Debug, Default)]
pub(super) struct Tail<F> {
    /// Just past the newest record folded in: where the next one begins.
    end: u64,
    found: F,
}

impl<F: Fold> Tail<F> {
    pub(super) fn found(&self) -> &F {
        &self.found
    }

    /// Folds in the records written since the last update.
    ///
    /// The first update, and the first after the file was replaced by a
    /// shorter one, reads back from the end of the file in `windows` of
    /// growing size; every later one reads only the appended bytes, back from
    /// their end too. Either stops as soon as what it has read completes the
    /// fold, and a field that is never recorded costs one read of the largest
    /// window, not one per refresh. Each byte is parsed once.
    pub(super) fn update(&mut self, path: &Path, windows: &[u64], read: impl Fn(&Value) -> F) {
        if file_length(path) < self.end {
            *self = Self::default();
        }
        let mut newer = F::default();
        let Some(end) = scan_back(path, self.end, windows, |record| {
            newer = std::mem::take(&mut newer).or(read(record));
            !newer.complete()
        }) else {
            return;
        };
        self.found = newer.or(std::mem::take(&mut self.found));
        self.end = end;
    }
}

/// Visits the records of `path` from offset `from` on, newest first, until
/// `visit` returns `false`. `from` must be where a record begins.
///
/// The file is read back from its end in `windows` of growing size, measured
/// from the end and never reaching before `from`, and each byte is read once:
/// the start of a record that crosses into the previous window is carried
/// over and completed by the next. Returns the offset just past the newest
/// complete record, so a record still being written is read by the next call.
pub(super) fn scan_back(
    path: &Path,
    from: u64,
    windows: &[u64],
    mut visit: impl FnMut(&Value) -> bool,
) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    if length <= from {
        return Some(length);
    }
    let mut end = None;
    let mut high = length;
    // Bytes from `high` to the end of the record that crosses it.
    let mut carried: Vec<u8> = Vec::new();
    for (index, window) in windows.iter().enumerate() {
        let low = length.saturating_sub(*window).max(from);
        if low >= high {
            continue;
        }
        // The last window's first line is parsed too: it is a whole record
        // when the window starts at `from`, and otherwise fails to parse.
        let last = low == from || index + 1 == windows.len();
        let mut buffer = vec![0; usize::try_from(high - low).ok()?];
        file.seek(SeekFrom::Start(low)).ok()?;
        file.read_exact(&mut buffer).ok()?;
        buffer.extend_from_slice(&carried);
        let mut lines = buffer.len();
        if end.is_none() {
            // The newest line is a record once its newline is written, or
            // sooner if it parses as one.
            let Some(cut) = newest_newline(&buffer, last) else {
                carried = buffer;
                high = low;
                continue;
            };
            let trailing = parse_record(&buffer[cut..]);
            end = Some(match trailing {
                Some(_) => length,
                None => length - (buffer.len() - cut) as u64,
            });
            if let Some(record) = trailing
                && !visit(&record)
            {
                return end;
            }
            lines = cut;
        }
        let start = if last {
            0
        } else {
            buffer[..lines]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(lines, |at| at + 1)
        };
        for line in buffer[start..lines].split(|byte| *byte == b'\n').rev() {
            if let Some(record) = parse_record(line)
                && !visit(&record)
            {
                return end;
            }
        }
        if last {
            break;
        }
        buffer.truncate(start);
        carried = buffer;
        high = low;
    }
    end
}

/// Where the newest line of `buffer` begins: just past its last newline, or
/// at its start when it holds none and no older bytes remain to be read.
pub(super) fn newest_newline(buffer: &[u8], last: bool) -> Option<usize> {
    match buffer.iter().rposition(|byte| *byte == b'\n') {
        Some(at) => Some(at + 1),
        None => last.then_some(0),
    }
}

pub(super) fn parse_record(line: &[u8]) -> Option<Value> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    serde_json::from_str(&String::from_utf8_lossy(line)).ok()
}

/// The first value `find` makes of a record within the first bytes of a
/// session file. Each update reads on from where the previous one stopped, so
/// a value that takes a while to be written costs each byte one parse.
#[derive(Debug)]
pub(super) struct Head<T> {
    read: u64,
    pub(super) found: Option<T>,
}

impl<T> Default for Head<T> {
    fn default() -> Self {
        Self {
            read: 0,
            found: None,
        }
    }
}

impl<T> Head<T> {
    pub(super) fn update(
        &mut self,
        path: &Path,
        limit: u64,
        mut find: impl FnMut(&Value) -> Option<T>,
    ) {
        let length = file_length(path);
        if length < self.read {
            *self = Self::default();
        }
        if self.found.is_some() || self.read >= limit.min(length) {
            return;
        }
        let Ok(mut file) = File::open(path) else {
            return;
        };
        let mut buffer = Vec::new();
        if file.seek(SeekFrom::Start(self.read)).is_err()
            || (&mut file)
                .take(limit - self.read)
                .read_to_end(&mut buffer)
                .is_err()
        {
            return;
        }
        let limited = self.read + buffer.len() as u64 >= limit;
        let cut = newest_newline(&buffer, true).unwrap_or(0);
        for line in buffer[..cut].split(|byte| *byte == b'\n') {
            if let Some(value) = parse_record(line).as_ref().and_then(&mut find) {
                self.found = Some(value);
                return;
            }
        }
        // A line without its newline yet is read again next time, unless it
        // already parses or is where the head ends.
        let trailing = parse_record(&buffer[cut..]);
        if let Some(value) = trailing.as_ref().and_then(&mut find) {
            self.found = Some(value);
            return;
        }
        self.read += if trailing.is_some() || limited {
            buffer.len()
        } else {
            cut
        } as u64;
    }
}

pub(super) fn file_length(path: &Path) -> u64 {
    fs::metadata(path).map(|data| data.len()).unwrap_or(0)
}
