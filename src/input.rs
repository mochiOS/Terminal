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

pub(crate) fn function(n: u8, shift: bool, alt: bool, control: bool) -> Option<Vec<u8>> {
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(control);
    if (1..=4).contains(&n) {
        let suffix = char::from(b'P' + n - 1);
        Some(
            if modifier == 1 {
                format!("\x1bO{suffix}")
            } else {
                format!("\x1b[1;{modifier}{suffix}")
            }
            .into_bytes(),
        )
    } else {
        let number = match n {
            5 => 15,
            6 => 17,
            7 => 18,
            8 => 19,
            9 => 20,
            10 => 21,
            11 => 23,
            12 => 24,
            _ => return None,
        };
        Some(
            if modifier == 1 {
                format!("\x1b[{number}~")
            } else {
                format!("\x1b[{number};{modifier}~")
            }
            .into_bytes(),
        )
    }
}
pub(crate) fn insert(shift: bool, alt: bool, control: bool) -> Vec<u8> {
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(control);
    if modifier == 1 {
        b"\x1b[2~".to_vec()
    } else {
        format!("\x1b[2;{modifier}~").into_bytes()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_control_and_function_keys_are_not_ui_commands() {
        assert_eq!(character('c', true, false).unwrap(), [3]);
        assert_eq!(function(1, false, false, false).unwrap(), b"\x1bOP");
        assert_eq!(function(12, true, false, true).unwrap(), b"\x1b[24;6~");
        assert_eq!(special(Special::Up, false, true, true), b"\x1b[1;7A");
        assert!(function(13, false, false, false).is_none());
    }
}
