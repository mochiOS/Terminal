//! Bounded, lossless partial-write handling for a nonblocking PTY master.
use std::{
    collections::VecDeque,
    io::{self, Write},
};
const CAPACITY: usize = 64 * 1024;
#[derive(Default)]
pub(crate) struct PendingInput(VecDeque<u8>);
impl PendingInput {
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
    pub(crate) fn send(&mut self, bytes: &[u8], writer: &mut impl Write) -> io::Result<()> {
        self.enqueue(bytes)?;
        self.flush(writer)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_and_blocked_writes_preserve_order_and_reject_overflow_atomically() {
        struct Writer {
            out: Vec<u8>,
            blocked: bool,
        }
        impl Write for Writer {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                if self.blocked {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let n = b.len().min(3);
                self.out.extend_from_slice(&b[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut pending = PendingInput::default();
        let bytes: Vec<u8> = (0..100).collect();
        pending.enqueue(&bytes).unwrap();
        assert!(pending.enqueue(&vec![0; CAPACITY]).is_err());
        let mut writer = Writer {
            out: vec![],
            blocked: true,
        };
        pending.flush(&mut writer).unwrap();
        assert!(writer.out.is_empty());
        writer.blocked = false;
        for _ in 0..20 {
            pending.flush(&mut writer).unwrap();
        }
        assert_eq!(writer.out, bytes);
    }
}
