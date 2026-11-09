//! `Read + Seek` adapter over [`SeekableSource`](folio_core::SeekableSource).

use std::io::{self, ErrorKind, Read, Seek, SeekFrom};

use folio_core::SeekableSource;

pub(crate) struct SourceReader<'a> {
    source: &'a mut dyn SeekableSource,
    pos: u64,
    len: u64,
}

impl<'a> SourceReader<'a> {
    pub(crate) fn new(source: &'a mut dyn SeekableSource) -> io::Result<Self> {
        let len = source.len()?;
        Ok(Self {
            source,
            pos: 0,
            len,
        })
    }
}

impl Read for SourceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.source.read_at(self.pos, buf)?;
        let advance =
            u64::try_from(read).map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
        self.pos = self
            .pos
            .checked_add(advance)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "read position overflow"))?;
        Ok(read)
    }
}

impl Seek for SourceReader<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(delta) => move_by(self.pos, delta)?,
            SeekFrom::End(delta) => move_by(self.len, delta)?,
        };
        if next > self.len {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "seek offset is past the end",
            ));
        }
        self.pos = next;
        Ok(self.pos)
    }
}

fn move_by(origin: u64, delta: i64) -> io::Result<u64> {
    if delta >= 0 {
        let add =
            u64::try_from(delta).map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
        origin
            .checked_add(add)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "seek overflow"))
    } else {
        let toward_zero = delta
            .checked_neg()
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "seek overflow"))?;
        let magnitude = u64::try_from(toward_zero)
            .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
        origin
            .checked_sub(magnitude)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "seek starts before the source"))
    }
}
