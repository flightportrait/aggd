//! Mode S and ADS-B decoding: bytes of one frame in, one `Msg` out.
//! Written from the standards' field layouts (ICAO Annex 10 vol. IV,
//! DO-260B); no state is kept here, the tracker owns it.

pub mod alt;
pub mod cpr;
pub mod encode;
pub mod uat;

/// CRC-24 over Mode S frames, generator 0x1FFF409.
const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u32) << 16;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x80_0000 != 0 { (c << 1) ^ 0xFF_F409 } else { c << 1 };
            k += 1;
        }
        t[i] = c & 0xFF_FFFF;
        i += 1;
    }
    t
}
static CRC: [u32; 256] = crc_table();

/// Remainder of the frame's data bits XOR its last three bytes: zero for
/// a clean DF17/18, the interrogator id for DF11, the address for
/// address-parity formats.
pub fn crc_residual(f: &[u8]) -> u32 {
    let n = f.len();
    let mut c = 0u32;
    for &b in &f[..n - 3] {
        c = ((c << 8) ^ CRC[(((c >> 16) as u8) ^ b) as usize]) & 0xFF_FFFF;
    }
    c ^ ((f[n - 3] as u32) << 16 | (f[n - 2] as u32) << 8 | f[n - 1] as u32)
}

/// Parity bytes for the first `n-3` bytes of `f` (encoder side).
pub fn crc_of(data: &[u8]) -> u32 {
    let mut c = 0u32;
    for &b in data {
        c = ((c << 8) ^ CRC[(((c >> 16) as u8) ^ b) as usize]) & 0xFF_FFFF;
    }
    c
}

/// Addresses that are not ICAO 24-bit addresses carry this bit.
pub const NON_ICAO: u32 = 1 << 24;

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum AddrType {
    AdsbIcao = 0,
    AdsbIcaoNt = 1,
    AdsrIcao = 2,
    TisbIcao = 3,
    Jaero = 4,
    Mlat = 5,
    Other = 6,
    ModeS = 7,
    AdsbOther = 8,
    AdsrOther = 9,
    TisbTrackfile = 10,
    TisbOther = 11,
    ModeA = 12,
    Unknown = 13,
}

impl AddrType {
    pub fn as_str(self) -> &'static str {
        match self {
            AddrType::AdsbIcao => "adsb_icao",
            AddrType::AdsbIcaoNt => "adsb_icao_nt",
            AddrType::AdsrIcao => "adsr_icao",
            AddrType::TisbIcao => "tisb_icao",
            AddrType::Jaero => "adsc",
            AddrType::Mlat => "mlat",
            AddrType::Other => "other",
            AddrType::ModeS => "mode_s",
            AddrType::AdsbOther => "adsb_other",
            AddrType::AdsrOther => "adsr_other",
            AddrType::TisbTrackfile => "tisb_trackfile",
            AddrType::TisbOther => "tisb_other",
            AddrType::ModeA => "mode_ac",
            AddrType::Unknown => "unknown",
        }
    }
}

/// How much a datum is trusted; a field is replaced only by an equal or
/// better source while it is fresh.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, Default)]
#[repr(u8)]
pub enum Source {
    #[default]
    Invalid = 0,
    Indirect = 1,
    ModeAc = 2,
    Sbs = 3,
    Mlat = 4,
    ModeS = 5,
    Jaero = 6,
    ModeSChecked = 7,
    Tisb = 8,
    Adsr = 9,
    Adsb = 10,
    Prio = 11,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cpr {
    pub lat: u32,
    pub lon: u32,
    pub odd: bool,
    pub surface: bool,
    /// ADS-B type code (5..=22), for NIC/Rc.
    pub tc: u8,
    /// NIC supplement B (airborne, version 2).
    pub nic_b: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Heading {
    Magnetic(f32),
    True(f32),
    /// Surface heading/track: which one is told by the operational status.
    Ground(f32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OpStatus {
    pub version: Option<u8>,
    pub nic_a: Option<bool>,
    pub nic_c: Option<bool>,
    pub nac_p: Option<u8>,
    pub sil: Option<u8>,
    pub sil_per_sample: Option<bool>,
    pub gva: Option<u8>,
    pub sda: Option<u8>,
    pub nic_baro: Option<bool>,
    /// Heading reference direction: true = magnetic north.
    pub hrd_magnetic: Option<bool>,
    /// Surface: the ground field is track (true) or heading (false).
    pub track_not_heading: Option<bool>,
}

pub const NAV_AUTOPILOT: u8 = 1;
pub const NAV_VNAV: u8 = 2;
pub const NAV_ALTHOLD: u8 = 4;
pub const NAV_APPROACH: u8 = 8;
pub const NAV_LNAV: u8 = 16;
pub const NAV_TCAS: u8 = 32;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Msg {
    pub df: u8,
    pub addr: u32,
    pub addr_type: Option<AddrType>,
    pub source: Source,
    /// ADS-B type code and subtype, 0 when not an extended squitter.
    pub tc: u8,
    pub sub: u8,
    /// Some(true): on the ground.
    pub ground: Option<bool>,
    pub alt_baro: Option<i32>,
    /// altitude in an air-to-air (DF0/16) reply; not used for the track
    pub acas_alt: Option<i32>,
    pub alt_geom: Option<i32>,
    /// Geometric minus barometric altitude, feet.
    pub geom_delta: Option<i32>,
    /// Squawk as four octal digits in hex nibbles (0x7700).
    pub squawk: Option<u16>,
    pub callsign: Option<[u8; 8]>,
    pub category: Option<u8>,
    pub gs: Option<f32>,
    pub track: Option<f32>,
    pub heading: Option<Heading>,
    pub ias: Option<u16>,
    pub tas: Option<u16>,
    pub mach: Option<f32>,
    pub baro_rate: Option<i32>,
    pub geom_rate: Option<i32>,
    pub roll: Option<f32>,
    pub track_rate: Option<f32>,
    pub cpr: Option<Cpr>,
    pub emergency: Option<u8>,
    pub alert: Option<bool>,
    pub spi: Option<bool>,
    pub nac_v: Option<u8>,
    pub nav_qnh: Option<f32>,
    pub nav_alt_mcp: Option<u32>,
    pub nav_alt_fms: Option<u32>,
    pub nav_heading: Option<f32>,
    pub nav_modes: Option<u8>,
    /// An ACAS resolution advisory report: the 56-bit field that carried it.
    pub acas_ra: Option<[u8; 7]>,
    pub op: OpStatus,
}

impl Default for Heading {
    fn default() -> Self {
        Heading::True(0.0)
    }
}

/// The 56-bit ME/MB field as one integer.
#[inline]
fn pack(m: &[u8]) -> u64 {
    let mut v = 0u64;
    for &b in &m[..7] {
        v = v << 8 | b as u64;
    }
    v
}

/// Bits `first..=last` of an ME/MB field (1-based, as the standards
/// number them).
#[inline]
fn bits(m: u64, first: u32, last: u32) -> u32 {
    ((m >> (56 - last)) & ((1u64 << (last - first + 1)) - 1)) as u32
}
#[inline]
fn bit(m: u64, n: u32) -> bool {
    m >> (56 - n) & 1 != 0
}

const CHARSET: &[u8; 64] = b"@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_ !\"#$%&'()*+,-./0123456789:;<=>?";

fn callsign(m: u64) -> Option<[u8; 8]> {
    let mut cs = [0u8; 8];
    for (i, c) in cs.iter_mut().enumerate() {
        let v = bits(m, 9 + 6 * i as u32, 14 + 6 * i as u32);
        *c = CHARSET[v as usize];
        if !(c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b' ') {
            return None;
        }
    }
    if cs == *b"        " {
        return None;
    }
    Some(cs)
}

/// Surface movement field to knots: the middle of the band the code stands for.
fn movement(m: u32) -> Option<f32> {
    Some(match m {
        1 => 0.0,
        2 => 0.0625,
        3..=8 => 0.125 + (m - 3) as f32 * 0.125 + 0.0625,
        9..=12 => 1.0 + (m - 9) as f32 * 0.25 + 0.125,
        13..=38 => 2.0 + (m - 13) as f32 * 0.5 + 0.25,
        39..=93 => 15.0 + (m - 39) as f32 + 0.5,
        94..=108 => 70.0 + (m - 94) as f32 * 2.0 + 1.0,
        109..=123 => 100.0 + (m - 109) as f32 * 5.0 + 2.5,
        124 => 175.0,
        _ => return None,
    })
}

/// A version 0 transponder states its accuracy only through the type
/// code of its position messages: (NACp, SIL) per DO-260B appendix N.
pub fn v0_accuracy(tc: u8) -> Option<(u8, u8)> {
    Some(match tc {
        5 | 9 => (11, 3),
        6 | 10 => (10, 3),
        7 | 11 => (8, 2),
        12 => (7, 2),
        13 => (6, 2),
        14 => (5, 2),
        15 => (4, 2),
        16 | 17 => (1, 2),
        20 => (11, 2),
        21 => (10, 2),
        8 | 18 | 22 => (0, 0),
        _ => return None,
    })
}

/// Decode one frame (7 or 14 bytes). `known` answers whether an address
/// is already tracked: formats whose parity is overlaid with the address
/// are only accepted for aircraft heard through a checked format.
pub fn decode(f: &[u8], known: impl Fn(u32) -> bool) -> Option<Msg> {
    decode_with(f, known, &Hints::default())
}

/// What is already known of the aircraft a frame claims to be from,
/// used to tell Comm-B registers apart: they carry no label.
#[derive(Clone, Copy, Default, Debug)]
pub struct Hints {
    pub alt_baro: Option<i32>,
    pub gs: Option<f32>,
    pub track: Option<f32>,
}

/// `decode`, with what is known of the aircraft.
pub fn decode_with(f: &[u8], known: impl Fn(u32) -> bool, hints: &Hints) -> Option<Msg> {
    if f.len() != 7 && f.len() != 14 {
        return None;
    }
    let df = f[0] >> 3;
    let long = df >= 16;
    if long != (f.len() == 14) {
        return None;
    }
    let mut m = Msg { df, ..Default::default() };
    let res = crc_residual(f);
    match df {
        0 | 4 | 5 | 16 | 20 | 21 => {
            if res == 0 || !known(res) {
                return None;
            }
            m.addr = res;
            m.source = Source::ModeS;
            m.addr_type = Some(AddrType::ModeS);
        }
        11 => {
            // The interrogator id sits in the low 7 bits of the parity.
            if res & 0xFF_FF80 != 0 {
                return None;
            }
            m.addr = (f[1] as u32) << 16 | (f[2] as u32) << 8 | f[3] as u32;
            if res != 0 && !known(m.addr) {
                return None;
            }
            m.source = Source::ModeSChecked;
            m.addr_type = Some(AddrType::ModeS);
        }
        17 | 18 => {
            if res != 0 {
                return None;
            }
            m.addr = (f[1] as u32) << 16 | (f[2] as u32) << 8 | f[3] as u32;
            m.source = Source::Adsb;
            m.addr_type = Some(AddrType::AdsbIcao);
        }
        _ => return None,
    }
    if m.addr == 0 {
        return None;
    }

    match df {
        0 | 16 => {
            // An air-to-air reply is addressed by parity alone and heard
            // by chance: enough to say the aircraft is there, not enough
            // to state its altitude over what better formats said.
            m.acas_alt = alt::ac13(((f[2] as u16) << 8 | f[3] as u16) & 0x1FFF);
        }
        4 | 20 | 5 | 21 => {
            let fs = f[0] & 7;
            match fs {
                0 => m.ground = Some(false),
                1 => m.ground = Some(true),
                2 => m.ground = Some(false),
                3 => m.ground = Some(true),
                _ => {}
            }
            m.alert = Some(matches!(fs, 2 | 3 | 4));
            m.spi = Some(matches!(fs, 4 | 5));
            let field = ((f[2] as u16) << 8 | f[3] as u16) & 0x1FFF;
            if df == 4 || df == 20 {
                m.alt_baro = alt::ac13(field);
            } else if field != 0 {
                m.squawk = Some(alt::id13(field));
            }
        }
        11 => match f[0] & 7 {
            4 => m.ground = Some(true),
            5 => m.ground = Some(false),
            _ => {}
        },
        _ => {}
    }
    if df == 17 {
        match f[0] & 7 {
            4 => m.ground = Some(true),
            5 => m.ground = Some(false),
            _ => {}
        }
        decode_es(&f[4..11], &mut m, false);
    } else if df == 18 {
        let cf = f[0] & 7;
        match cf {
            0 => {
                m.addr_type = Some(AddrType::AdsbIcaoNt);
                decode_es(&f[4..11], &mut m, false);
            }
            1 => {
                m.addr_type = Some(AddrType::AdsbOther);
                m.addr |= NON_ICAO;
                decode_es(&f[4..11], &mut m, false);
            }
            2 | 5 => {
                m.source = Source::Tisb;
                m.addr_type = Some(AddrType::TisbIcao);
                decode_es(&f[4..11], &mut m, true);
                if cf == 5 && m.addr_type == Some(AddrType::TisbIcao) {
                    m.addr_type = Some(AddrType::TisbOther);
                    m.addr |= NON_ICAO;
                }
            }
            6 => {
                m.source = Source::Adsr;
                m.addr_type = Some(AddrType::AdsrIcao);
                decode_es(&f[4..11], &mut m, true);
            }
            // Coarse TIS-B, management and reserved control fields: not an
            // aircraft's own address. readsb lists these as targets of
            // unknown kind under a non-ICAO address, and so do we.
            _ => {
                m.addr |= NON_ICAO;
                m.addr_type = Some(AddrType::Unknown);
                // only the type code is looked at: the rest is not an
                // aircraft's state
                m.tc = f[4] >> 3;
            }
        }
    } else if df == 20 || df == 21 {
        // Probed on readsb: only the altitude in the reply itself (DF20)
        // helps tell registers apart. What is remembered of the aircraft
        // does not: a reply that reads two ways is dropped in a DF21 even
        // when one reading matches the ADS-B velocity, and a reply that
        // reads one way is taken even 200 kt off it.
        let _ = hints;
        let own = Hints { alt_baro: m.alt_baro, gs: None, track: None };
        decode_commb(&f[4..11], &mut m, &own);
    }
    Some(m)
}

/// An extended squitter's 56-bit ME field. `imf_possible`: rebroadcasts
/// carry an ICAO/Mode A flag where the transponder's own messages do not.
fn decode_es(me: &[u8], m: &mut Msg, imf_possible: bool) {
    let me = pack(me);
    let tc = bits(me, 1, 5) as u8;
    m.tc = tc;
    m.sub = bits(me, 6, 8) as u8;
    let mut imf = false;
    match tc {
        1..=4 => {
            m.category = Some(((0x0E - tc) << 4) | m.sub);
            m.callsign = callsign(me);
        }
        5..=8 => {
            m.ground = Some(true);
            m.gs = movement(bits(me, 6, 12));
            if bit(me, 13) {
                m.heading = Some(Heading::Ground(bits(me, 14, 20) as f32 * 360.0 / 128.0));
            }
            m.cpr = Some(Cpr {
                lat: bits(me, 23, 39),
                lon: bits(me, 40, 56),
                odd: bit(me, 22),
                surface: true,
                tc,
                nic_b: false,
            });
        }
        0 | 9..=18 | 20..=22 => {
            let ac = bits(me, 9, 20) as u16;
            if ac != 0 {
                let a = alt::ac12(ac);
                if tc >= 20 {
                    m.alt_geom = a;
                } else {
                    m.alt_baro = a;
                }
            }
            if tc != 0 {
                if imf_possible {
                    imf = bit(me, 8);
                }
                let ss = bits(me, 6, 7);
                m.alert = Some(ss == 1 || ss == 2);
                m.spi = Some(ss == 3);
                m.cpr = Some(Cpr {
                    lat: bits(me, 23, 39),
                    lon: bits(me, 40, 56),
                    odd: bit(me, 22),
                    surface: false,
                    tc,
                    nic_b: !imf_possible && bit(me, 8),
                });
            }
        }
        19 => {
            let st = m.sub;
            if !(1..=4).contains(&st) {
                return;
            }
            if imf_possible {
                imf = bit(me, 9);
            }
            m.nac_v = Some(bits(me, 11, 13) as u8);
            let mul = if st == 2 || st == 4 { 4.0 } else { 1.0 };
            if st <= 2 {
                let ew = bits(me, 15, 24);
                let ns = bits(me, 26, 35);
                if ew != 0 && ns != 0 {
                    let mut vx = (ew - 1) as f32 * mul;
                    let mut vy = (ns - 1) as f32 * mul;
                    if bit(me, 14) {
                        vx = -vx;
                    }
                    if bit(me, 25) {
                        vy = -vy;
                    }
                    let gs = (vx * vx + vy * vy).sqrt();
                    m.gs = Some(gs);
                    if gs > 0.0 {
                        let mut t = vx.atan2(vy).to_degrees();
                        if t < 0.0 {
                            t += 360.0;
                        }
                        m.track = Some(t);
                    }
                }
            } else {
                if bit(me, 14) {
                    m.heading = Some(Heading::Magnetic(bits(me, 15, 24) as f32 * 360.0 / 1024.0));
                }
                let v = bits(me, 26, 35);
                if v != 0 {
                    let kt = ((v - 1) as f32 * mul) as u16;
                    if bit(me, 25) {
                        m.tas = Some(kt);
                    } else {
                        m.ias = Some(kt);
                    }
                }
            }
            let vr = bits(me, 38, 46);
            if vr != 0 {
                let mut r = (vr as i32 - 1) * 64;
                if bit(me, 37) {
                    r = -r;
                }
                if bit(me, 36) {
                    m.baro_rate = Some(r);
                } else {
                    m.geom_rate = Some(r);
                }
            }
            let d = bits(me, 50, 56);
            if d != 0 {
                let mut delta = (d as i32 - 1) * 25;
                if bit(me, 49) {
                    delta = -delta;
                }
                m.geom_delta = Some(delta);
            }
        }
        28 => {
            if m.sub == 2 {
                let mut b = [0u8; 7];
                b.copy_from_slice(&me.to_be_bytes()[1..]);
                m.acas_ra = Some(b);
            }
            if m.sub == 1 {
                m.emergency = Some(bits(me, 9, 11) as u8);
                let id = bits(me, 12, 24) as u16;
                if id != 0 {
                    m.squawk = Some(alt::id13(id));
                }
            }
        }
        29 => {
            // subtype is two bits here
            let st = bits(me, 6, 7);
            if st == 1 {
                m.sub = 1;
                let a = bits(me, 10, 20);
                if a != 0 {
                    let ft = (a - 1) * 32;
                    if bit(me, 9) {
                        m.nav_alt_fms = Some(ft);
                    } else {
                        m.nav_alt_mcp = Some(ft);
                    }
                }
                let q = bits(me, 21, 29);
                if q != 0 {
                    m.nav_qnh = Some(800.0 + (q - 1) as f32 * 0.8);
                }
                if bit(me, 30) {
                    m.nav_heading = Some(bits(me, 31, 39) as f32 * 180.0 / 256.0);
                }
                m.op.nac_p = Some(bits(me, 40, 43) as u8);
                m.op.nic_baro = Some(bit(me, 44));
                m.op.sil = Some(bits(me, 45, 46) as u8);
                if bit(me, 47) {
                    let mut n = 0;
                    if bit(me, 48) {
                        n |= NAV_AUTOPILOT;
                    }
                    if bit(me, 49) {
                        n |= NAV_VNAV;
                    }
                    if bit(me, 50) {
                        n |= NAV_ALTHOLD;
                    }
                    if bit(me, 52) {
                        n |= NAV_APPROACH;
                    }
                    if bit(me, 54) {
                        n |= NAV_LNAV;
                    }
                    if bit(me, 53) {
                        n |= NAV_TCAS;
                    }
                    m.nav_modes = Some(n);
                }
                if imf_possible {
                    imf = bit(me, 51);
                }
            }
        }
        31 => {
            let st = m.sub;
            if st > 1 {
                return;
            }
            let v = bits(me, 41, 43) as u8;
            m.op.version = Some(v);
            if v == 0 {
                return;
            }
            m.op.nic_a = Some(bit(me, 44));
            m.op.nac_p = Some(bits(me, 45, 48) as u8);
            m.op.sil = Some(bits(me, 51, 52) as u8);
            m.op.hrd_magnetic = Some(bit(me, 54));
            if st == 0 {
                m.op.nic_baro = Some(bit(me, 53));
            } else {
                // bit 53 on the surface: 1 = the heading is reported, 0 = the track
                m.op.track_not_heading = Some(!bit(me, 53));
                m.op.nic_c = Some(bit(me, 20));
                m.nac_v = Some(bits(me, 17, 19) as u8);
            }
            if v >= 2 {
                m.op.sil_per_sample = Some(bit(me, 55));
                m.op.sda = Some(bits(me, 31, 32) as u8);
                if st == 0 {
                    m.op.gva = Some(bits(me, 49, 50) as u8);
                }
            }
            if imf_possible {
                imf = bit(me, 56);
            }
        }
        _ => {}
    }
    if imf {
        m.addr |= NON_ICAO;
        m.addr_type = Some(match m.source {
            Source::Tisb => AddrType::TisbOther,
            _ => AddrType::AdsrOther,
        });
    }
}

/// Comm-B replies do not say which register they carry. Each candidate
/// register must be well formed (a field marked absent is all zeros,
/// values are possible), and then agree with what else is known: Mach
/// and indicated airspeed with each other at the aircraft's altitude,
/// ground speed and track with what ADS-B reports. Ambiguity that is
/// left decodes to nothing.
fn decode_commb(mb: &[u8], m: &mut Msg, hints: &Hints) {
    let mb = pack(mb);
    if mb == 0 {
        return;
    }
    // BDS 3,0: ACAS resolution advisory, tagged by its first byte; an
    // advisory has at least one advisory bit or says it has ended.
    if mb >> 48 == 0x30 && (bits(mb, 9, 22) != 0 || bit(mb, 27)) && bits(mb, 29, 30) != 3 {
        let mut b = [0u8; 7];
        b.copy_from_slice(&mb.to_be_bytes()[1..]);
        m.acas_ra = Some(b);
        return;
    }
    // BDS 2,0: aircraft identification, tagged by its first byte.
    if mb >> 48 == 0x20 {
        if let Some(cs) = callsign(mb) {
            m.callsign = Some(cs);
        }
        return;
    }
    // BDS 1,0 and 1,7 (capability reports) are tagged or mostly zero and
    // carry nothing shown; do not mistake them for flight data.
    if mb >> 48 == 0x10 || bits(mb, 29, 56) == 0 {
        return;
    }
    let c40 = bds40(mb);
    let c50 = bds50(mb);
    let c60 = bds60(mb);
    // Whether the 6,0 reading's Mach and airspeed agree at the reply's
    // altitude; None when it carries none.
    let fit60 = c60.as_ref().and_then(|c| match (c.ias, c.mach, hints.alt_baro) {
        (Some(ias), Some(mach), Some(alt)) => Some((cas_from_mach(mach as f64, alt as f64) - ias as f64).abs() < 25.0),
        _ => None,
    });
    // A reply that reads only one way is that register, whether or not
    // it fits. One that reads two ways is settled by the altitude or
    // not at all.
    let pick = match (c40.is_some(), c50.is_some(), c60.is_some()) {
        (true, false, false) => 40,
        (false, true, false) => 50,
        (false, false, true) => 60,
        (false, true, true) => match fit60 {
            Some(true) => 60,
            Some(false) => 50,
            None => 0,
        },
        (true, _, true) if fit60 == Some(true) => 60,
        _ => 0,
    };
    match pick {
        40 => {
            let a = c40.unwrap();
            m.nav_alt_mcp = a.0;
            m.nav_alt_fms = a.1;
            m.nav_qnh = a.2;
            m.nav_modes = a.3;
        }
        50 => {
            let b = c50.unwrap();
            m.roll = b.roll;
            m.track = b.track;
            m.gs = b.gs;
            m.track_rate = b.track_rate;
            m.tas = b.tas;
        }
        60 => {
            let c = c60.unwrap();
            m.heading = c.heading.map(Heading::Magnetic);
            m.ias = c.ias;
            m.mach = c.mach;
            m.baro_rate = c.baro_rate;
            m.geom_rate = c.inertial_rate;
        }
        _ => {}
    }
}

/// Calibrated airspeed (knots) for a Mach number at a pressure altitude
/// (feet), in the standard atmosphere.
pub fn cas_from_mach(mach: f64, alt_ft: f64) -> f64 {
    const P0: f64 = 101_325.0;
    let p = if alt_ft < 36_089.0 {
        P0 * (1.0 - 6.875_59e-6 * alt_ft).powf(5.255_88)
    } else {
        22_632.0 * (-(alt_ft - 36_089.0) / 20_806.0).exp()
    };
    let qc = p * ((1.0 + 0.2 * mach * mach).powf(3.5) - 1.0);
    661.47 * (5.0 * ((qc / P0 + 1.0).powf(2.0 / 7.0) - 1.0)).sqrt()
}

type Bds40 = (Option<u32>, Option<u32>, Option<f32>, Option<u8>);

fn bds40(mb: u64) -> Option<Bds40> {
    if bits(mb, 40, 47) != 0 || bits(mb, 52, 53) != 0 {
        return None;
    }
    let field = |status: u32, first: u32, last: u32| -> Result<Option<u32>, ()> {
        let v = bits(mb, first, last);
        if bit(mb, status) {
            Ok(Some(v))
        } else if v != 0 {
            Err(())
        } else {
            Ok(None)
        }
    };
    let mcp = field(1, 2, 13).ok()?;
    let fms = field(14, 15, 26).ok()?;
    let baro = field(27, 28, 39).ok()?;
    if mcp.is_none() && fms.is_none() && baro.is_none() {
        return None;
    }
    let ok_alt = |a: Option<u32>| a.map_or(true, |v| (31..=3125).contains(&v)); // 500..50000 ft
    if !ok_alt(mcp) || !ok_alt(fms) {
        return None;
    }
    let qnh = baro.map(|v| 800.0 + v as f32 * 0.1);
    if qnh.map_or(false, |q| !(900.0..=1100.0).contains(&q)) {
        return None;
    }
    let modes = if bit(mb, 48) {
        let mut n = 0;
        if bit(mb, 49) {
            n |= NAV_VNAV;
        }
        if bit(mb, 50) {
            n |= NAV_ALTHOLD;
        }
        if bit(mb, 51) {
            n |= NAV_APPROACH;
        }
        Some(n)
    } else {
        if bits(mb, 49, 51) != 0 {
            return None;
        }
        None
    };
    if !bit(mb, 54) && bits(mb, 55, 56) != 0 {
        return None;
    }
    Some((mcp.map(|v| v * 16), fms.map(|v| v * 16), qnh, modes))
}

/// A field with a status bit: None when marked absent (and then its
/// bits must be zero, or the register is not this one).
fn opt_signed(mb: u64, status: u32, first: u32, last: u32) -> Result<Option<i32>, ()> {
    let raw = bits(mb, first, last);
    if !bit(mb, status) {
        return if raw == 0 { Ok(None) } else { Err(()) };
    }
    let w = last - first + 1;
    // the first bit of the field is its sign
    Ok(Some(if raw >> (w - 1) != 0 { raw as i32 - (1 << w) } else { raw as i32 }))
}
fn opt_unsigned(mb: u64, status: u32, first: u32, last: u32) -> Result<Option<u32>, ()> {
    let raw = bits(mb, first, last);
    if !bit(mb, status) {
        return if raw == 0 { Ok(None) } else { Err(()) };
    }
    Ok(Some(raw))
}

struct Bds50 {
    roll: Option<f32>,
    track: Option<f32>,
    gs: Option<f32>,
    track_rate: Option<f32>,
    tas: Option<u16>,
}

fn bds50(mb: u64) -> Option<Bds50> {
    let roll = opt_signed(mb, 1, 2, 11).ok()?.map(|v| v as f32 * 45.0 / 256.0);
    let track = opt_signed(mb, 12, 13, 23).ok()?.map(|v| (v as f32 * 90.0 / 512.0).rem_euclid(360.0));
    let gs = opt_unsigned(mb, 24, 25, 34).ok()?.map(|v| v as f32 * 2.0);
    let track_rate = opt_signed(mb, 35, 36, 45).ok()?.map(|v| v as f32 * 8.0 / 256.0);
    let tas = opt_unsigned(mb, 46, 47, 56).ok()?.map(|v| v * 2);
    // roll, track and ground speed come from the same computer: a
    // register with none of the three, or without any speed, is not it
    // (probed: readsb takes a reply without track rate, not one without
    // roll or without airspeed)
    if roll.is_none() || track.is_none() || gs.is_none() || tas.is_none() {
        return None;
    }
    if roll.map_or(false, |r| r.abs() > 50.0) {
        return None;
    }
    if gs.map_or(false, |g| !(40.0..=700.0).contains(&g)) || tas.map_or(false, |t| !(40..=700).contains(&t)) {
        return None;
    }
    if let (Some(g), Some(t)) = (gs, tas) {
        if (g - t as f32).abs() > 200.0 {
            return None;
        }
    }
    Some(Bds50 { roll, track, gs, track_rate, tas: tas.map(|t| t as u16) })
}

struct Bds60 {
    heading: Option<f32>,
    ias: Option<u16>,
    mach: Option<f32>,
    baro_rate: Option<i32>,
    inertial_rate: Option<i32>,
}

fn bds60(mb: u64) -> Option<Bds60> {
    let heading = opt_signed(mb, 1, 2, 12).ok()?.map(|v| (v as f32 * 90.0 / 512.0).rem_euclid(360.0));
    let ias = opt_unsigned(mb, 13, 14, 23).ok()?;
    let mach = opt_unsigned(mb, 24, 25, 34).ok()?.map(|v| v as f32 * 2.048 / 512.0);
    let baro_rate = opt_signed(mb, 35, 36, 45).ok()?.map(|v| v * 32);
    let inertial_rate = opt_signed(mb, 46, 47, 56).ok()?.map(|v| v * 32);
    if heading.is_none() || ias.is_none() || mach.is_none() {
        return None;
    }
    if ias.map_or(false, |v| !(40..=500).contains(&v)) || mach.map_or(false, |v| !(0.05..=1.0).contains(&v)) {
        return None;
    }
    if baro_rate.map_or(false, |v| v.abs() > 6000) || inertial_rate.map_or(false, |v| v.abs() > 6000) {
        return None;
    }
    if let (Some(b), Some(i)) = (baro_rate, inertial_rate) {
        if (b - i).abs() > 2000 {
            return None;
        }
    }
    Some(Bds60 { heading, ias: ias.map(|v| v as u16), mach, baro_rate, inertial_rate })
}

/// Navigation integrity category and containment radius (metres, 0 when
/// unknown) for a position message.
pub fn nic_rc(c: &Cpr, version: u8, nic_a: bool, nic_c: bool) -> (u8, u32) {
    let b = c.nic_b;
    match c.tc {
        5 | 9 | 20 => (11, 8), // 7.5 m, shown rounded
        6 | 10 | 21 => (10, 25),
        7 => {
            if version == 2 && nic_a && !nic_c {
                (9, 75)
            } else {
                (8, 186)
            }
        }
        8 => match (version == 2, nic_a, nic_c) {
            (true, true, true) => (7, 371),
            (true, true, false) => (6, 556),
            (true, false, true) => (6, 1112),
            _ => (0, 0),
        },
        11 => {
            if (version == 2 && nic_a && b) || (version == 1 && nic_a) {
                (9, 75)
            } else {
                (8, 186)
            }
        }
        12 => (7, 371),
        13 => {
            if version == 2 {
                match (nic_a, b) {
                    (false, true) => (6, 556),
                    (false, false) => (6, 926),
                    (true, true) => (6, 1112),
                    _ => (6, 926),
                }
            } else if version == 1 {
                if nic_a {
                    (6, 1112)
                } else {
                    (6, 926)
                }
            } else {
                (6, 926)
            }
        }
        14 => (5, 1852),
        15 => (4, 3704),
        16 => {
            if (version == 2 && nic_a && b) || (version == 1 && nic_a) {
                (3, 7408)
            } else {
                (2, 14816)
            }
        }
        17 => (1, 37040),
        _ => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn ident() {
        let m = decode(&hex("8D4840D6202CC371C32CE0576098"), |_| false).unwrap();
        assert_eq!(m.addr, 0x4840D6);
        assert_eq!(&m.callsign.unwrap(), b"KLM1023 ");
    }

    #[test]
    fn airborne_pair() {
        let e = decode(&hex("8D40621D58C382D690C8AC2863A7"), |_| false).unwrap();
        let o = decode(&hex("8D40621D58C386435CC412692AD6"), |_| false).unwrap();
        assert_eq!(e.alt_baro, Some(38000));
        let (ce, co) = (e.cpr.unwrap(), o.cpr.unwrap());
        assert!(!ce.odd && co.odd);
        let (lat, lon) = cpr::global_airborne(ce.lat, ce.lon, co.lat, co.lon, false).unwrap();
        assert!((lat - 52.2572).abs() < 1e-3, "{lat}");
        assert!((lon - 3.91937).abs() < 1e-3, "{lon}");
    }

    #[test]
    fn velocity() {
        let m = decode(&hex("8D485020994409940838175B284F"), |_| false).unwrap();
        assert!((m.gs.unwrap() - 159.2).abs() < 0.1);
        assert!((m.track.unwrap() - 182.88).abs() < 0.01);
        assert_eq!(m.baro_rate.or(m.geom_rate), Some(-832));
        let m = decode(&hex("8DA05F219B06B6AF189400CBC33F"), |_| false).unwrap();
        assert_eq!(m.tas, Some(375));
        assert!((m.heading.map(|h| match h { Heading::Magnetic(v) => v, _ => -1.0 }).unwrap() - 243.98).abs() < 0.01);
    }

    #[test]
    fn commb_registers() {
        let known = |a: u32| a == 0xABCDEF;
        let alt = alt::ac13_encode(36000);
        let m = decode(&encode::long_ap(20, alt, 0xABCDEF, encode::bds50(-12.0, 250.0, 440, 0.5, 450)), known).unwrap();
        assert_eq!((m.addr, m.alt_baro), (0xABCDEF, Some(36000)));
        assert!((m.roll.unwrap() + 12.0).abs() < 0.2, "{:?}", m.roll);
        assert!((m.track.unwrap() - 250.0).abs() < 0.2, "{:?}", m.track);
        assert_eq!((m.gs, m.tas), (Some(440.0), Some(450)));
        let m = decode(&encode::long_ap(20, alt, 0xABCDEF, encode::bds60(271.0, 280, 0.78, -640, -608)), known).unwrap();
        assert!(matches!(m.heading, Some(Heading::Magnetic(h)) if (h - 271.0).abs() < 0.2), "{:?}", m.heading);
        assert_eq!((m.ias, m.baro_rate, m.geom_rate), (Some(280), Some(-640), Some(-608)));
        assert!((m.mach.unwrap() - 0.78).abs() < 0.004);
        let m = decode(&encode::long_ap(21, alt::id13_encode(0x2345), 0xABCDEF, encode::bds40(36000, 36000, 1013.2)), known).unwrap();
        assert_eq!((m.squawk, m.nav_alt_mcp, m.nav_alt_fms), (Some(0x2345), Some(36000), Some(36000)));
        assert!((m.nav_qnh.unwrap() - 1013.2).abs() < 0.05);
        let m = decode(&encode::long_ap(20, alt, 0xABCDEF, encode::bds20(b"DLH4AB  ")), known).unwrap();
        assert_eq!(&m.callsign.unwrap(), b"DLH4AB  ");
        // an unknown address is noise
        assert!(decode(&encode::long_ap(20, alt, 0x123456, encode::bds20(b"DLH4AB  ")), known).is_none());
    }

    #[test]
    fn commb_from_a_live_network() {
        let known = |a: u32| a == 0xABDEC9;
        // heading, IAS 253, Mach 0.832, barometric rate; no inertial rate
        let m = decode(&hex("A0001998D239FB343FF000651AA0"), known).unwrap();
        assert_eq!((m.addr, m.alt_baro, m.ias), (0xABDEC9, Some(40000), Some(253)));
        assert!((m.mach.unwrap() - 0.832).abs() < 0.001 && m.baro_rate == Some(-64) && m.geom_rate.is_none(), "{m:?}");
        assert!((cas_from_mach(0.832, 40000.0) - 253.0).abs() < 2.0);
        // roll, track, ground speed, true airspeed from the same aircraft
        let m = decode(&hex("A0001998FFD98D3E4004EE73EFAE"), known).unwrap();
        assert!(m.roll.is_some() && m.gs.is_some() && m.tas.is_some() && m.ias.is_none(), "{m:?}");
        // selected altitude 40000 ft, 1013.3 hPa
        let m = decode(&hex("A0001998CE200030AA01463EFCDB"), known).unwrap();
        assert_eq!(m.nav_alt_mcp, Some(40000));
    }

    #[test]
    fn synthetic_position_decodes_to_what_went_in() {
        let fr = encode::synthetic_position(0x4B1234, 44.0, 10.0, Some(31000), Some((430.0, 95.0)), None);
        assert_eq!(fr.len(), 3);
        let (e, o, v) = (decode(&fr[0], |_| false).unwrap(), decode(&fr[1], |_| false).unwrap(), decode(&fr[2], |_| false).unwrap());
        assert_eq!((e.df, e.addr, e.alt_baro, e.source), (18, 0x4B1234, Some(31000), Source::Tisb));
        let (ce, co) = (e.cpr.unwrap(), o.cpr.unwrap());
        let (lat, lon) = cpr::global_airborne(ce.lat, ce.lon, co.lat, co.lon, true).unwrap();
        assert!((lat - 44.0).abs() < 1e-4 && (lon - 10.0).abs() < 1e-4);
        assert!((v.gs.unwrap() - 430.0).abs() < 1.0 && (v.track.unwrap() - 95.0).abs() < 0.3);
        assert!(v.baro_rate.is_none() && v.geom_rate.is_none());
    }

    #[test]
    fn roundtrip_encoders() {
        let f = encode::ident(0xABCDEF, b"TEST123 ", 0xA3);
        let m = decode(&f, |_| false).unwrap();
        assert_eq!(&m.callsign.unwrap(), b"TEST123 ");
        assert_eq!(m.category, Some(0xA3));
        let f = encode::velocity(0xABCDEF, 450.0, 270.0, -1280, Some(250));
        let m = decode(&f, |_| false).unwrap();
        assert!((m.gs.unwrap() - 450.0).abs() < 1.0);
        assert!((m.track.unwrap() - 270.0).abs() < 0.2);
        assert_eq!(m.baro_rate, Some(-1280));
        assert_eq!(m.geom_delta, Some(250));
        let f0 = encode::airborne_position(0xABCDEF, 45.5, 9.25, 35000, false, 11);
        let f1 = encode::airborne_position(0xABCDEF, 45.5, 9.25, 35000, true, 11);
        let (a, b) = (decode(&f0, |_| false).unwrap(), decode(&f1, |_| false).unwrap());
        assert_eq!(a.alt_baro, Some(35000));
        let (ca, cb) = (a.cpr.unwrap(), b.cpr.unwrap());
        let (lat, lon) = cpr::global_airborne(ca.lat, ca.lon, cb.lat, cb.lon, true).unwrap();
        assert!((lat - 45.5).abs() < 1e-4 && (lon - 9.25).abs() < 1e-4);
        let f = encode::df4(0xABCDEF, 12000, false);
        let m = decode(&f, |a| a == 0xABCDEF).unwrap();
        assert_eq!((m.addr, m.alt_baro), (0xABCDEF, Some(12000)));
        let f = encode::df5(0xABCDEF, 0x7700);
        assert_eq!(decode(&f, |a| a == 0xABCDEF).unwrap().squawk, Some(0x7700));
    }
}
