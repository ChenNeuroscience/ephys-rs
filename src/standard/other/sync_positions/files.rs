//! A recording's files, found from its `.set` file, as Write LTX names them (see `crate::standard::recording::ltx`), and
//! reading and writing them: its positions (`name.pos`), the board's digital inputs (`name.ttl`, timed by the board's
//! clock) and the timing LED's changes (`name.pos.ttl`, timed by the camera's), with the original positions kept beside
//! them (`name.pos.original`), as Windows names a copy when the name's taken (`name (2).pos.original`...).

use std::io;
use std::path::{Path, PathBuf};

/// The header a synced positions file has, naming the file it was synced from (beside it).
pub const SYNCED_FROM: &str = "synced_from";
/// The header a synced positions file has, saying what synced it.
pub const SYNCED_BY: &str = "synced_by";

/// A position record's size: a 4-byte timestamp, then 8 two-byte values.
pub const POS_RECORD: usize = 20;

/// A recording's files, from its `.set` file's path: whether they're there's up to the disk.
#[derive(Clone, PartialEq, Debug)]
pub struct Recording {
    pub set: PathBuf,
    pub pos: PathBuf,
    pub ttl: PathBuf,
    pub pos_ttl: PathBuf,
}

impl Recording {
    /// The recording `set` is the `.set` file of, if it is one.
    pub fn of(set: &Path) -> Option<Self> {
        if set.extension()? != "set" {
            return None;
        }
        let with = |extension: &str| set.with_extension(extension);
        Some(Recording { set: set.to_path_buf(), pos: with("pos"), ttl: with("ttl"), pos_ttl: with("pos.ttl") })
    }

    /// The recording's name: the `.set` file's, without its extension.
    pub fn name(&self) -> String {
        self.set.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned())
    }

    /// The `n`th name for the original positions: `name.pos.original`, then `name (2).pos.original`...
    pub fn original(&self, n: usize) -> PathBuf {
        let name = match n {
            1 => format!("{}.pos.original", self.name()),
            n => format!("{} ({n}).pos.original", self.name()),
        };
        self.set.with_file_name(name)
    }

    /// The first name for the original positions that isn't taken.
    pub fn free_original(&self) -> PathBuf {
        (1..).map(|n| self.original(n)).find(|path| !path.exists()).expect("a free name")
    }

    /// What syncing would read, and what it'd do to the positions there are: from the original, if they were synced
    /// from one before (and it's still there), otherwise from them, kept first as a new original.
    pub fn plan(&self) -> io::Result<Plan> {
        let headers = read_headers(&self.pos)?;
        let earlier = header(&headers, SYNCED_FROM).map(|name| self.set.with_file_name(name));
        Ok(match earlier {
            Some(original) if original.exists() => Plan::Resync { original },
            _ => Plan::Keep { original: self.free_original() },
        })
    }
}

/// What syncing does to the positions there are.
#[derive(Clone, PartialEq, Debug)]
pub enum Plan {
    /// They're as recorded: they're copied to `original` first, then synced.
    Keep { original: PathBuf },
    /// They were synced before, from `original`: it's synced again, in their place.
    Resync { original: PathBuf },
}

impl Plan {
    /// Where the positions synced come from.
    pub fn original(&self) -> &Path {
        match self {
            Plan::Keep { original } | Plan::Resync { original } => original,
        }
    }
}

/// An LTX file: its header lines, as `key value`, and its data, between `data_start` and `data_end`.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct LtxFile {
    pub headers: Vec<(String, String)>,
    pub data: Vec<u8>,
}

const DATA_START: &[u8] = b"data_start";
const DATA_END: &[u8] = b"\r\ndata_end";

impl LtxFile {
    pub fn read(path: &Path) -> io::Result<Self> {
        Self::parse(&std::fs::read(path)?).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
    }

    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
        let (head, data) = match find(bytes, DATA_START) {
            Some(at) => (&bytes[..at], &bytes[at + DATA_START.len()..]),
            // Nothing recorded: only headers.
            None => (bytes, &[][..]),
        };
        let data = match data.len().checked_sub(DATA_END.len()) {
            Some(end) if data.ends_with(DATA_END) => &data[..end],
            _ if data.is_empty() => data,
            _ => return Err(invalid("its data doesn't end with data_end: was it finished?")),
        };
        let head = String::from_utf8_lossy(head);
        let lines = head.split("\r\n").map(str::trim_end).filter(|line| !line.is_empty());
        let headers = lines
            .map(|line| match line.split_once(' ') {
                Some((key, value)) => (key.to_string(), value.trim().to_string()),
                None => (line.to_string(), String::new()),
            })
            .collect();
        Ok(LtxFile { headers, data: data.to_vec() })
    }

    pub fn header(&self, key: &str) -> Option<&str> {
        header(&self.headers, key)
    }

    /// Sets `key`'s value, where it is, or at the end.
    pub fn set_header(&mut self, key: &str, value: impl ToString) {
        match self.headers.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value.to_string(),
            None => self.headers.push((key.to_string(), value.to_string())),
        }
    }

    pub fn bytes(&self) -> Vec<u8> {
        let lines: Vec<String> = self.headers.iter().map(|(k, v)| format!("{k} {v}")).collect();
        let mut bytes = lines.join("\r\n").into_bytes();
        if !self.data.is_empty() {
            bytes.extend_from_slice(b"\r\n");
            bytes.extend_from_slice(DATA_START);
            bytes.extend_from_slice(&self.data);
            bytes.extend_from_slice(DATA_END);
        }
        bytes
    }

    /// Written whole, or not at all: to a file beside it first, then put in its place.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".writing");
        let writing = path.with_file_name(name);
        std::fs::write(&writing, self.bytes())?;
        std::fs::rename(&writing, path)
    }
}

fn header<'a>(headers: &'a [(String, String)], key: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn find(bytes: &[u8], what: &[u8]) -> Option<usize> {
    bytes.windows(what.len()).position(|window| window == what)
}

/// Just a file's headers, without reading what may be a lot of data after them.
fn read_headers(path: &Path) -> io::Result<Vec<(String, String)>> {
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(path)?.take(64 * 1024).read_to_end(&mut head)?;
    if let Some(at) = find(&head, DATA_START) {
        head.truncate(at);
    }
    Ok(LtxFile::parse(&head)?.headers)
}

/// A change of a digital line, from a TTL file: which line (from 1), when, in seconds (counting from what the file's
/// `ttl_seconds_from` says), and whether it went up.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Change {
    pub line: u32,
    pub seconds: f64,
    pub up: bool,
}

/// A TTL file's changes, in order, as `ttl_<line> <seconds> <state>` lines.
pub fn changes(file: &LtxFile) -> Result<Vec<Change>, String> {
    let text = String::from_utf8_lossy(&file.data);
    let lines = text.split("\r\n").map(str::trim).filter(|line| !line.is_empty());
    lines
        .map(|line| {
            let bad = || format!("Not a TTL change: \"{line}\"");
            let mut words = line.split_whitespace();
            let line = words.next().and_then(|w| w.strip_prefix("ttl_")?.parse().ok()).ok_or_else(bad)?;
            let seconds = words.next().and_then(|w| w.parse().ok()).ok_or_else(bad)?;
            let up = match words.next() {
                Some("1") => true,
                Some("0") => false,
                _ => return Err(bad()),
            };
            Ok(Change { line, seconds, up })
        })
        .collect()
}

/// A position record's timestamp, in ticks.
pub fn timestamp(record: &[u8]) -> u32 {
    u32::from_be_bytes([record[0], record[1], record[2], record[3]])
}

pub fn set_timestamp(record: &mut [u8], ticks: u32) {
    record[..4].copy_from_slice(&ticks.to_be_bytes());
}

/// A positions file's timebase, in ticks a second.
pub fn timebase(file: &LtxFile) -> Result<f64, String> {
    let value = file.header("timestamp_timebase").ok_or("The positions have no timestamp_timebase")?;
    let hz = value.trim_end_matches("hz").trim().parse::<f64>();
    hz.ok().filter(|hz| *hz > 0.0).ok_or_else(|| format!("The positions' timestamp_timebase isn't a rate: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ephys-rs-sync-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_files_are_found_from_the_set_file() {
        let recording = Recording::of(Path::new("/r/session 1.set")).unwrap();
        assert_eq!(recording.pos, Path::new("/r/session 1.pos"));
        assert_eq!(recording.ttl, Path::new("/r/session 1.ttl"));
        assert_eq!(recording.pos_ttl, Path::new("/r/session 1.pos.ttl"));
        assert!(Recording::of(Path::new("/r/session.pos")).is_none(), "only from a .set file");
    }

    #[test]
    fn originals_are_named_as_windows_names_copies() {
        let dir = scratch("originals");
        let recording = Recording::of(&dir.join("s.set")).unwrap();
        assert_eq!(recording.free_original(), dir.join("s.pos.original"));
        std::fs::write(dir.join("s.pos.original"), "").unwrap();
        assert_eq!(recording.free_original(), dir.join("s (2).pos.original"));
        std::fs::write(dir.join("s (2).pos.original"), "").unwrap();
        assert_eq!(recording.free_original(), dir.join("s (3).pos.original"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn positions_synced_before_are_synced_again_from_their_original() {
        let dir = scratch("plan");
        let recording = Recording::of(&dir.join("s.set")).unwrap();
        let mut pos = LtxFile { headers: vec![("trial_date".into(), "today".into())], data: vec![0; POS_RECORD] };
        pos.write(&recording.pos).unwrap();
        assert_eq!(recording.plan().unwrap(), Plan::Keep { original: dir.join("s.pos.original") }, "as recorded");
        pos.set_header(SYNCED_FROM, "s (2).pos.original");
        pos.write(&recording.pos).unwrap();
        assert_eq!(
            recording.plan().unwrap(),
            Plan::Keep { original: dir.join("s.pos.original") },
            "its original's gone: it's kept as it is"
        );
        std::fs::write(dir.join("s (2).pos.original"), "").unwrap();
        assert_eq!(recording.plan().unwrap(), Plan::Resync { original: dir.join("s (2).pos.original") });
        assert!(Recording::of(&dir.join("missing.set")).unwrap().plan().is_err(), "no positions");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ltx_files_read_back_as_written() {
        let file = LtxFile {
            headers: vec![("trial_date".into(), "Monday, 09 Nov 2026".into()), ("num_pos_samples".into(), "2".into())],
            data: (0..2 * POS_RECORD as u8).collect(),
        };
        let bytes = file.bytes();
        assert!(bytes.starts_with(b"trial_date Monday, 09 Nov 2026\r\nnum_pos_samples 2\r\ndata_start\x00\x01"));
        assert!(bytes.ends_with(b"\r\ndata_end"));
        assert_eq!(LtxFile::parse(&bytes).unwrap(), file);
        let headers_only = LtxFile { headers: file.headers.clone(), data: Vec::new() };
        assert_eq!(LtxFile::parse(&headers_only.bytes()).unwrap(), headers_only, "nothing recorded");
        assert!(LtxFile::parse(&bytes[..bytes.len() - 3]).is_err(), "not finished");
    }

    #[test]
    fn ttl_changes_are_read_from_their_lines() {
        let file = LtxFile { headers: Vec::new(), data: b"ttl_1 0.5 1\r\nttl_3 1.25 0\r\n".to_vec() };
        let expected = vec![Change { line: 1, seconds: 0.5, up: true }, Change { line: 3, seconds: 1.25, up: false }];
        assert_eq!(changes(&file).unwrap(), expected);
        let bad = LtxFile { headers: Vec::new(), data: b"ttl_1 0.5 high\r\n".to_vec() };
        assert!(changes(&bad).unwrap_err().contains("ttl_1 0.5 high"));
    }
}
