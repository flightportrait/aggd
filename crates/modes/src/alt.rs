//! Altitude and identity code fields.

/// The 13-bit identity field (C1 A1 C2 A2 C4 A4 X B1 D1 B2 D2 B4 D4) as
/// four octal digits in hex nibbles: squawk 7700 is 0x7700.
pub fn id13(f: u16) -> u16 {
    const MAP: [(u16, u16); 12] = [
        (0x1000, 0x0010),
        (0x0800, 0x1000),
        (0x0400, 0x0020),
        (0x0200, 0x2000),
        (0x0100, 0x0040),
        (0x0080, 0x4000),
        (0x0020, 0x0100),
        (0x0010, 0x0001),
        (0x0008, 0x0200),
        (0x0004, 0x0002),
        (0x0002, 0x0400),
        (0x0001, 0x0004),
    ];
    MAP.iter().filter(|(i, _)| f & i != 0).fold(0, |a, (_, o)| a | o)
}

/// Inverse of `id13`.
pub fn id13_encode(squawk: u16) -> u16 {
    const MAP: [(u16, u16); 12] = [
        (0x1000, 0x0010),
        (0x0800, 0x1000),
        (0x0400, 0x0020),
        (0x0200, 0x2000),
        (0x0100, 0x0040),
        (0x0080, 0x4000),
        (0x0020, 0x0100),
        (0x0010, 0x0001),
        (0x0008, 0x0200),
        (0x0004, 0x0002),
        (0x0002, 0x0400),
        (0x0001, 0x0004),
    ];
    MAP.iter().filter(|(_, o)| squawk & o != 0).fold(0, |a, (i, _)| a | i)
}

/// A Gillham-coded Mode C reply (in `id13` form) to hundreds of feet.
fn mode_c(a: u16) -> Option<i32> {
    let a = a as u32;
    if a & 0xFFFF_8889 != 0 || a & 0xF0 == 0 {
        return None;
    }
    let mut one = 0u32;
    if a & 0x0010 != 0 {
        one ^= 7;
    }
    if a & 0x0020 != 0 {
        one ^= 3;
    }
    if a & 0x0040 != 0 {
        one ^= 1;
    }
    if one & 5 == 5 {
        one ^= 2;
    }
    if one > 5 {
        return None;
    }
    let mut five = 0u32;
    for (bit, x) in [
        (0x0002, 0xFF),
        (0x0004, 0x7F),
        (0x1000, 0x3F),
        (0x2000, 0x1F),
        (0x4000, 0x0F),
        (0x0100, 0x07),
        (0x0200, 0x03),
        (0x0400, 0x01),
    ] {
        if a & bit != 0 {
            five ^= x;
        }
    }
    if five & 1 != 0 {
        one = 6 - one;
    }
    Some(five as i32 * 5 + one as i32 - 13)
}

/// 13-bit altitude code (DF0/4/16/20), feet.
pub fn ac13(ac: u16) -> Option<i32> {
    if ac == 0 {
        return None;
    }
    if ac & 0x0040 != 0 {
        // metric altitude: not used in practice
        return None;
    }
    if ac & 0x0010 != 0 {
        let n = ((ac & 0x1F80) >> 2) | ((ac & 0x0020) >> 1) | (ac & 0x000F);
        return Some(n as i32 * 25 - 1000);
    }
    let h = mode_c(id13(ac))?;
    if h < -12 {
        return None;
    }
    Some(h * 100)
}

/// 12-bit altitude code (extended squitter), feet.
pub fn ac12(ac: u16) -> Option<i32> {
    if ac == 0 {
        return None;
    }
    if ac & 0x010 != 0 {
        let n = ((ac & 0x0FE0) >> 1) | (ac & 0x000F);
        return Some(n as i32 * 25 - 1000);
    }
    let n = ((ac & 0x0FC0) << 1) | (ac & 0x003F);
    let h = mode_c(id13(n))?;
    if h < -12 {
        return None;
    }
    Some(h * 100)
}

/// Encode feet with 25 ft resolution.
pub fn ac13_encode(ft: i32) -> u16 {
    let n = ((ft + 1000) / 25) as u16;
    ((n << 2) & 0x1F80) | ((n << 1) & 0x0020) | (n & 0x000F) | 0x0010
}
pub fn ac12_encode(ft: i32) -> u16 {
    let n = ((ft + 1000) / 25) as u16;
    ((n << 1) & 0x0FE0) | (n & 0x000F) | 0x0010
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        for ft in [-1000, 0, 2500, 38000, 45025] {
            assert_eq!(ac13(ac13_encode(ft)), Some(ft));
            assert_eq!(ac12(ac12_encode(ft)), Some(ft));
        }
        for s in [0x7700u16, 0x1200, 0x2345, 0x7777, 0x0001] {
            assert_eq!(id13(id13_encode(s)), s);
        }
    }
}
