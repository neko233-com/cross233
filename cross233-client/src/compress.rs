use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use std::io::{self, Read, Write};

pub struct CompressWriter<W: Write> {
    inner: DeflateEncoder<W>,
}

impl<W: Write> CompressWriter<W> {
    pub fn new(w: W) -> Self {
        Self {
            inner: DeflateEncoder::new(w, Compression::default()),
        }
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.inner.finish()
    }
}

impl<W: Write> Write for CompressWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub struct CompressReader<R: Read> {
    inner: DeflateDecoder<R>,
}

impl<R: Read> CompressReader<R> {
    pub fn new(r: R) -> Self {
        Self {
            inner: DeflateDecoder::new(r),
        }
    }
}

impl<R: Read> Read for CompressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}
