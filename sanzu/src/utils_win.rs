/// Transform PS2 code to hardware keycode
/// PS2 codes:
/// https://commons.wikimedia.org/wiki/File:Ps2_de_keyboard_scancode_set_1.svg
pub fn windows_scancode_to_hardware_keycode(keycode: u32, flags: u32) -> Option<u16> {
    if flags & 1 == 0 {
        return match keycode {
            // numpad
            0x52 => Some(0x5a), // 0 insert
            0x4f => Some(0x57), // 1 fin
            0x50 => Some(0x58), // 2 down
            0x51 => Some(0x59), // 3 pagedn
            0x4b => Some(0x53), // 4 left
            0x4c => Some(0x54), // 5
            0x4d => Some(0x55), // 6 right
            0x47 => Some(0x4f), // 7 home
            0x48 => Some(0x50), // 8 up
            0x49 => Some(0x51), // 9 pageup

            0x37 => Some(0x3f), // *
            0x4a => Some(0x52), // -
            0x4e => Some(0x56), // +

            //0x37 => Some(0x6b), // print screen
            0x46 => Some(0x4e), // scroll lock
            0x45 => Some(0x7f), // pause

            0x38 => Some(0x40), // left alt

            0x1D => Some(0x25), // ctrl left
            0x1 => Some(0x09),  // Esc
            0x2 => Some(0x0A),  // Digit1
            0x3 => Some(0x0B),  // Digit2
            0x4 => Some(0x0C),  // Digit3
            0x5 => Some(0x0D),  // Digit4
            0x6 => Some(0x0E),  // Digit5
            0x7 => Some(0x0F),  // Digit6
            0x8 => Some(0x10),  // Digit7
            0x9 => Some(0x11),  // Digit8
            0xa => Some(0x12),  // Digit9
            0xb => Some(0x13),  // Digit0
            0xc => Some(0x14),  // minus
            0xd => Some(0x15),  // equal
            0xe => Some(0x16),  // backspace

            0x0f => Some(0x17), // TAB
            0x10 => Some(0x18), // KeyA
            0x11 => Some(0x19), // KeyZ
            0x12 => Some(0x1A), // KeyE
            0x13 => Some(0x1B), // KeyR
            0x14 => Some(0x1C), // KeyT
            0x15 => Some(0x1D), // KeyY
            0x16 => Some(0x1E), // KeyU
            0x17 => Some(0x1F), // KeyI
            0x18 => Some(0x20), // KeyO
            0x19 => Some(0x21), // KeyP
            0x1A => Some(0x22), // point point
            0x1B => Some(0x23), // dollar
            0x1C => Some(0x24), // enter

            0x1e => Some(0x26), // KeyQ
            0x1f => Some(0x27), // KeyS
            0x20 => Some(0x28), // KeyD
            0x21 => Some(0x29), // KeyF
            0x22 => Some(0x2A), // KeyG
            0x23 => Some(0x2B), // KeyH
            0x24 => Some(0x2C), // KeyJ
            0x25 => Some(0x2D), // KeyK
            0x26 => Some(0x2E), // KeyL
            0x27 => Some(0x2F), // KeyM
            0x28 => Some(0x30), // percent
            0x29 => Some(0x31), // square
            0x2b => Some(0x33), // start

            0x2a => Some(0x32), // shift left

            0x2c => Some(0x34), // KeyW
            0x2d => Some(0x35), // KeyX
            0x2e => Some(0x36), // KeyC
            0x2f => Some(0x37), // KeyV
            0x30 => Some(0x38), // KeyB
            0x31 => Some(0x39), // KeyN
            0x32 => Some(0x3A), // ,
            0x33 => Some(0x3B), // ;
            0x34 => Some(0x3C), // :
            0x35 => Some(0x3D), // !

            0x39 => Some(0x41), // space
            0x3a => Some(0x42), // capslock

            0x3b => Some(0x43), // F1
            0x3c => Some(0x44), // F2
            0x3d => Some(0x45), // F3
            0x3e => Some(0x46), // F4
            0x3f => Some(0x47), // F5
            0x40 => Some(0x48), // F6
            0x41 => Some(0x49), // F7
            0x42 => Some(0x4A), // F8
            0x43 => Some(0x4B), // F9
            0x44 => Some(0x4C), // F10

            0x53 => Some(0x5b), // suppr / dot keypad

            0x56 => Some(0x5E), // <>
            0x57 => Some(0x5F), // F11
            0x58 => Some(0x60), // F12

            _ => None,
        };
    } else {
        return match keycode {
            // cursor
            0x4b => Some(0x71), // left
            0x4d => Some(0x72), // right
            0x48 => Some(0x6f), // up
            0x50 => Some(0x74), // down

            // numpad
            0x35 => Some(0x6a), // /
            0x1c => Some(0x24), // enter

            0x52 => Some(0x76), // insert
            0x53 => Some(0x77), // suppr

            0x47 => Some(0x6e), // home
            0x4F => Some(0x73), // end

            0x49 => Some(0x70), // pageup
            0x51 => Some(0x75), // pagedn

            0x38 => Some(0x6C), // right alt
            0x1D => Some(0x69), // ctrl right
            0x36 => Some(0x3E), // shift right

            0x45 => Some(0x4d), // vernum

            0x5b => Some(0x85), // win gauche
            0x5c => Some(0x86), // win droit
            0x5d => Some(0x87), // menu

            _ => None,
        };
    }
}

/// Convert hardware keycode to ps2 code
pub fn hardware_keycode_to_windows_scancode(hw_keycode: u32) -> Option<(u16, bool)> {
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
        0x004e => (0x46, false), // scroll lock

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
