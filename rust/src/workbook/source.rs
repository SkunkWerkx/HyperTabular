//! Where the container's bytes come from. A [`Sheet`](crate::workbook::Sheet) is independent of
//! its [`Workbook`](crate::workbook::Workbook) — it owns its own reader over the same bytes — so a
//! source must be able to hand out a second, independent reader: a byte-slice cursor
//! clones, a file reopens by path.

use std::fs::File;
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A seekable byte source that can produce an independent second reader.
pub trait Source: Read + Seek + Sized {
    /// A fresh reader over the same bytes, positioned at the start, independent of `self`.
    fn reopen(&self) -> io::Result<Self>;
}

impl Source for Cursor<&[u8]> {
    fn reopen(&self) -> io::Result<Self> {
        Ok(Cursor::new(self.get_ref()))
    }
}

impl Source for Cursor<Arc<[u8]>> {
    fn reopen(&self) -> io::Result<Self> {
        Ok(Cursor::new(Arc::clone(self.get_ref())))
    }
}

/// A file source that reopens by path (a `File::try_clone` would share the offset).
pub struct FileSource {
    path: PathBuf,
    file: BufReader<File>,
}

impl FileSource {
    /// Opens `path` with a 64 KiB read buffer.
    pub fn open(path: impl AsRef<Path>) -> io::Result<FileSource> {
        let path = path.as_ref().to_path_buf();
        let file = BufReader::with_capacity(64 * 1024, File::open(&path)?);
        Ok(FileSource { path, file })
    }

    /// The path this source reads.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Read for FileSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for FileSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

impl Source for FileSource {
    fn reopen(&self) -> io::Result<Self> {
        FileSource::open(&self.path)
    }
}
