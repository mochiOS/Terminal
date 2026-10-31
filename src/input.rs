//! Terminal byte sequences, independent of the GUI event representation.
#[derive(Clone, Copy)]
pub(crate) enum Special {
    Escape,
    Delete,
    Home,
    End,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    BackTab,
}

pub(crate) fn character(value: char, control: bool, alt: bool) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    if alt {
        bytes.push(0x1b);
    }
    if control {
        let byte = match value {
            'a'..='z' => value as u8 - b'a' + 1,
            '@'..='_' => value as u8 & 31,
            ' ' => 0,
            '?' => 127,
            _ => return None,
        };
        bytes.push(byte);
    } else {
        bytes.extend_from_slice(value.encode_utf8(&mut [0; 4]).as_bytes());
    }
    Some(bytes)
}

pub(crate) fn special(key: Special, shift: bool, alt: bool, control: bool) -> Vec<u8> {
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(control);
    let (number, final_byte) = match key {
        Special::Escape => return vec![27],
        Special::BackTab => return b"\x1b[Z".to_vec(),
        Special::Up => (1, 'A'),
        Special::Down => (1, 'B'),
        Special::Right => (1, 'C'),
        Special::Left => (1, 'D'),
        Special::Home => (1, 'H'),
        Special::End => (1, 'F'),
        Special::Delete => (3, '~'),
        Special::PageUp => (5, '~'),
        Special::PageDown => (6, '~'),
    };
    if modifier != 1 {
        format!("\x1b[{number};{modifier}{final_byte}").into_bytes()
    } else if final_byte == '~' {
        format!("\x1b[{number}~").into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

pub(crate) fn text(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n").replace('\r', "\n").into_bytes()
}
