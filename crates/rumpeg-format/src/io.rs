//! Media I/O abstraction (FFmpeg `AVIOContext` analogue).

use rumpeg_util::{Error, Result};
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Read + seek capability required by demuxers.
pub trait IoReader: Read + Seek + Send {}
impl<T: Read + Seek + Send> IoReader for T {}

/// Write + seek capability required by muxers.
pub trait IoWriter: Write + Seek + Send {}
impl<T: Write + Seek + Send> IoWriter for T {}

/// Owned media I/O handle.
pub enum MediaIo {
    /// Filesystem file.
    File(File),
    /// In-memory buffer.
    Memory(Cursor<Vec<u8>>),
}

impl MediaIo {
    /// Open a file for reading.
    pub fn open_read(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::File(File::open(path)?))
    }

    /// Create/truncate a file for writing.
    pub fn open_write(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::File(File::create(path)?))
    }

    /// Wrap an owned byte buffer.
    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self::Memory(Cursor::new(data))
    }

    /// Current length if known.
    pub fn len(&mut self) -> Result<u64> {
        let pos = self.stream_position()?;
        let end = self.seek(SeekFrom::End(0))?;
        self.seek(SeekFrom::Start(pos))?;
        Ok(end)
    }

    /// `true` if empty.
    pub fn is_empty(&mut self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
}

impl Read for MediaIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::File(f) => f.read(buf),
            Self::Memory(c) => c.read(buf),
        }
    }
}

impl Write for MediaIo {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::File(f) => f.write(buf),
            Self::Memory(c) => c.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::File(f) => f.flush(),
            Self::Memory(c) => c.flush(),
        }
    }
}

impl Seek for MediaIo {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::File(f) => f.seek(pos),
            Self::Memory(c) => c.seek(pos),
        }
    }
}

/// Read exactly `n` bytes or fail.
pub fn read_exact_vec(r: &mut dyn IoReader, n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    match r.read_exact(&mut buf) {
        Ok(()) => Ok(buf),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(Error::Eof),
        Err(e) => Err(Error::Io(e)),
    }
}
