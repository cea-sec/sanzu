/* We want hardware keycode! */
pub fn hid_code_to_hardware_keycode(keycode: u32, flags: u32) -> Option<u16> {
    // Distinguish:
    // alt left / right
    // shift / arrow
    let hw_keycode = match (keycode, flags) {
        (0x38, 0) | (0x38, 1) => Some(0x40), // left alt
        (0x38, 2) | (0x38, 3) => Some(0x6C), // right alt

        (0x2a, 0) | (0x2a, 1) => Some(0x32), // real left shift

        (0x48, 0) | (0x48, 1) => Some(0x0050), // keypad 8
        (0x48, 2) | (0x48, 3) => Some(0x006f), // arrow up

        (0x49, 0) | (0x49, 1) => Some(0x0051), // pageup / 9
        (0x49, 2) | (0x49, 3) => Some(0x0070), // pageup / 9

        // (0x4A, 0) | (0x4A, 1) => Some(0x0052), // minus keypad
        (0x4b, 0) | (0x4b, 1) => Some(0x0053), // left / 4
        (0x4b, 2) | (0x4b, 3) => Some(0x0071), // left / 4

        // (0x4c, 0) | (0x4c, 1) => Some(0x0054), // numpad middle 5
        (0x4d, 0) | (0x4d, 1) => Some(0x0055), // right / right
        (0x4d, 2) | (0x4d, 3) => Some(0x0072), // right / right

        // (0x4e, 0) | (0x4e, 1) => Some(0x0056), // keypad +
        (0x4F, 0) | (0x4F, 1) => Some(0x0057), // fin / 1 keypad
        (0x4F, 2) | (0x4F, 3) => Some(0x0073), // fin / 1 keypad

        (0x50, 0) | (0x50, 1) => Some(0x0058), // down / 2 keypad
        (0x50, 2) | (0x50, 3) => Some(0x0074), // down / 2 keypad

        (0x51, 0) | (0x51, 1) => Some(0x0059), // pagedown / 3 keypad
        (0x51, 2) | (0x51, 3) => Some(0x0075), // pagedown / 3 keypad

        (0x52, 0) | (0x52, 1) => Some(0x005a), // 0 keypad
        (0x52, 2) | (0x52, 3) => Some(0x0076), // insert

        (0x53, 0) | (0x53, 1) => Some(0x005b), // dot keypad
        (0x53, 2) | (0x53, 3) => Some(0x0077), // dot keypad

        (0x2a, 2) | (0x2a, 3) => {
            // ignore additional key hit durring arrow
            return None;
        }
        (_, _) => None,
    };

    if let Some(hw_keycode) = hw_keycode {
        return Some(hw_keycode);
    }

    let hw_keycode = match keycode {
        0x1 => 0x0009, // Esc
        0x2 => 0x000A, // Digit1
        0x3 => 0x000B, // Digit2
        0x4 => 0x000C, // Digit3
        0x5 => 0x000D, // Digit4
        0x6 => 0x000E, // Digit5
        0x7 => 0x000F, // Digit6
        0x8 => 0x0010, // Digit7
        0x9 => 0x0011, // Digit8
        0xa => 0x0012, // Digit9
        0xb => 0x0013, // Digit0
        0xc => 0x0014, // minus
        0xd => 0x0015, // equal
        0xe => 0x0016, // equal

        0x0f => 0x0017, // TAB
        0x10 => 0x0018, // KeyA
        0x11 => 0x0019, // KeyZ
        0x12 => 0x001A, // KeyE
        0x13 => 0x001B, // KeyR
        0x14 => 0x001C, // KeyT
        0x15 => 0x001D, // KeyY
        0x16 => 0x001E, // KeyU
        0x17 => 0x001F, // KeyI
        0x18 => 0x0020, // KeyO
        0x19 => 0x0021, // KeyP
        0x1A => 0x0022, // point point
        0x1B => 0x0023, // dollar
        0x1C => 0x0024, // enter

        0x1D => 0x0025, // ctrl left

        0x1e => 0x0026, // KeyQ
        0x1f => 0x0027, // KeyS
        0x20 => 0x0028, // KeyD
        0x21 => 0x0029, // KeyF
        0x22 => 0x002A, // KeyG
        0x23 => 0x002B, // KeyH
        0x24 => 0x002C, // KeyJ
        0x25 => 0x002D, // KeyK
        0x26 => 0x002E, // KeyL
        0x27 => 0x002F, // KeyM
        0x28 => 0x0030, // percent
        0x29 => 0x0031, // square
        0x2b => 0x0033, // start

        0x2a => 0x0032, // shift left

        0x2c => 0x0034, // KeyW
        0x2d => 0x0035, // KeyX
        0x2e => 0x0036, // KeyC
        0x2f => 0x0037, // KeyV
        0x30 => 0x0038, // KeyB
        0x31 => 0x0039, // KeyN
        0x32 => 0x003A, // ,
        0x33 => 0x003B, // ;
        0x34 => 0x003C, // :
        0x35 => 0x003D, // !
        0x36 => 0x003E, // shift right

        0x37 => 0x003f, // keypad *
        0x38 => 0x0040, // alt left/right

        0x39 => 0x0041, // space
        0x3a => 0x0042, // capslock

        0x3b => 0x0043, // F1
        0x3c => 0x0044, // F2
        0x3d => 0x0045, // F3
        0x3e => 0x0046, // F4
        0x3f => 0x0047, // F5
        0x40 => 0x0048, // F6
        0x41 => 0x0049, // F7
        0x42 => 0x004A, // F8
        0x43 => 0x004B, // F9
        0x44 => 0x004C, // F10

        0x45 => 0x004d, // ver num

        0x47 => 0x004f, // home / 7
        0x48 => 0x006f, // up / 8
        0x49 => 0x0070, // pageup / 9
        0x4A => 0x0052, // minus keypad
        0x4b => 0x0071, // left / 4
        0x4c => 0x0054, // numpad middle 5
        0x4d => 0x0072, // right / right
        0x4e => 0x0056, // keypad +
        0x4F => 0x0073, // fin / 1 keypad
        0x50 => 0x0074, // down / 2 keypad
        0x51 => 0x0075, // pagedown / 3 keypad
        0x52 => 0x0076, // 0 keypad
        0x53 => 0x0077, // suppr / dot keypad

        0x56 => 0x005E, // <>
        0x57 => 0x005F, // F11
        0x58 => 0x0060, // F12

        _ => return None,
    };

    Some(hw_keycode as u16)
}

/* We want hardware keycode! */
pub fn hardware_keycode_to_hid_code(hw_keycode: u32) -> Option<(u16, bool)> {
    let (keycode, extended) = match hw_keycode {
        0x0009 => (0x01, false), // Esc
        0x000A => (0x02, false), // Digit1
        0x000B => (0x03, false), // Digit2
        0x000C => (0x04, false), // Digit3
        0x000D => (0x05, false), // Digit4
        0x000E => (0x06, false), // Digit5
        0x000F => (0x07, false), // Digit6
        0x0010 => (0x08, false), // Digit7
        0x0011 => (0x09, false), // Digit8
        0x0012 => (0x0a, false), // Digit9
        0x0013 => (0x0b, false), // Digit0
        0x0014 => (0x0c, false), // minus
        0x0015 => (0x0d, false), // equal
        0x0016 => (0x0e, false), // equal

        0x0017 => (0x0f, false), // TAB
        0x0018 => (0x10, false), // KeyA
        0x0019 => (0x11, false), // KeyZ
        0x001A => (0x12, false), // KeyE
        0x001B => (0x13, false), // KeyR
        0x001C => (0x14, false), // KeyT
        0x001D => (0x15, false), // KeyY
        0x001E => (0x16, false), // KeyU
        0x001F => (0x17, false), // KeyI
        0x0020 => (0x18, false), // KeyO
        0x0021 => (0x19, false), // KeyP
        0x0022 => (0x1A, false), // point point
        0x0023 => (0x1B, false), // dollar
        0x0024 => (0x1C, false), // enter

        0x0025 => (0x1D, false), // ctrl left

        0x0026 => (0x1e, false), // KeyQ
        0x0027 => (0x1f, false), // KeyS
        0x0028 => (0x20, false), // KeyD
        0x0029 => (0x21, false), // KeyF
        0x002A => (0x22, false), // KeyG
        0x002B => (0x23, false), // KeyH
        0x002C => (0x24, false), // KeyJ
        0x002D => (0x25, false), // KeyK
        0x002E => (0x26, false), // KeyL
        0x002F => (0x27, false), // KeyM
        0x0030 => (0x28, false), // percent
        0x0031 => (0x29, false), // square
        0x0033 => (0x2b, false), // start

        0x0032 => (0x2a, false), // shift left

        0x0034 => (0x2c, false), // KeyW
        0x0035 => (0x2d, false), // KeyX
        0x0036 => (0x2e, false), // KeyC
        0x0037 => (0x2f, false), // KeyV
        0x0038 => (0x30, false), // KeyB
        0x0039 => (0x31, false), // KeyN
        0x003A => (0x32, false), // ,
        0x003B => (0x33, false), // ;
        0x003C => (0x34, false), // :
        0x003D => (0x35, false), // !
        0x003E => (0x36, false), // shift right

        0x003f => (0x37, false), // keypad *
        0x0040 => (0x38, false), // alt left/right

        0x0041 => (0x39, false), // space

        0x0042 => (0x3A, false), // capslock

        0x0043 => (0x3b, false), // F1
        0x0044 => (0x3c, false), // F2
        0x0045 => (0x3d, false), // F3
        0x0046 => (0x3e, false), // F4
        0x0047 => (0x3f, false), // F5
        0x0048 => (0x40, false), // F6
        0x0049 => (0x41, false), // F7
        0x004A => (0x42, false), // F8
        0x004B => (0x43, false), // F9
        0x004C => (0x44, false), // F10

        0x004d => (0x45, false), // ver num

        0x004f => (0x47, false), // home / 7
        0x0052 => (0x4A, false), // minus keypad
        0x0054 => (0x4c, false), // numpad middle 5
        0x0056 => (0x4e, false), // keypad +

        0x005E => (0x56, false), // <>
        0x005F => (0x57, false), // F11
        0x0060 => (0x58, false), // F12

        0x6C => (0x38, true), // right alt

        0x50 => (0x48, false), // keypad 8
        0x51 => (0x49, false), // pageup / 9
        0x53 => (0x4b, false), // left / 4
        0x55 => (0x4d, false), // right / right
        0x57 => (0x4F, false), // fin / 1 keypad
        0x58 => (0x50, false), // down / 2 keypad
        0x59 => (0x51, false), // pagedown / 3 keypad
        0x5a => (0x52, false), // 0 keypad
        0x5b => (0x53, false), // dot keypad

        0x6f => (0x48, true), // arrow up
        0x70 => (0x49, true), // pageup / 9
        0x71 => (0x4b, true), // left / 4
        0x72 => (0x4d, true), // right / right
        0x73 => (0x4F, true), // fin / 1 keypad
        0x74 => (0x50, true), // down / 2 keypad
        0x75 => (0x51, true), // pagedown / 3 keypad
        0x76 => (0x52, true), // insert
        0x77 => (0x53, true), // dot keypad

        _ => return None,
    };
    Some((keycode as u16, extended))
}
