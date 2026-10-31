//! Bounded, lossless partial-write handling for a nonblocking PTY master.
use std::{
    collections::VecDeque,
    io::{self, Write},
};
const CAPACITY: usize = 64 * 1024;
#[derive(Default)]
pub(crate) struct PendingInput(VecDeque<u8>);
impl PendingInput {
    pub(crate) fn enqueue(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > CAPACITY - self.0.len() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "terminal input queue is full",
            ));
        }
        self.0.extend(bytes);
        Ok(())
    }
    pub(crate) fn flush(&mut self, writer: &mut impl Write) -> io::Result<()> {
        for _ in 0..4 {
            let (first, _) = self.0.as_slices();
            if first.is_empty() {
                return Ok(());
            }
            let bytes = &first[..first.len().min(4096)];
            match writer.write(bytes) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "PTY input closed")),
                Ok(count) => {
                    self.0.drain(..count);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
