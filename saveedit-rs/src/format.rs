// Romestead .char save format (CandideSerializer, compact-strings mode).
//
// Layout: [4B magic "ceb\x01"][string map][payload]
//   string map: [7bit count][0x21 elem-typecode][count raw strings (7bit len + utf8)]
//               string id = 1-based index
//   payload:    [0x20 CompactObject][7bit numFields][7bit totalLen][field entries]
//   field entry: [typecode][7bit nameId][7bit payloadLen][payload]
//
// The game's serializer omits fields equal to the type default, so Normal
// saves have NO Difficulty field (0) and zero-cash saves have NO Money field.
// Enabling a value therefore means inserting a well-formed entry and growing
// all enclosing length headers — nothing else in the file needs to change.

use anyhow::{bail, Context, Result};

pub const TC_INT32: u8 = 3;
pub const TC_UINT64: u8 = 20;
pub const TC_COMPACT_OBJECT: u8 = 32;
pub const TC_STRING: u8 = 33;
pub const TC_GUID: u8 = 37;
pub const TC_ENUM: u8 = 38; // NullableEnum: payload [3][int32]
pub const TC_INTERFACE: u8 = 48; // null=[0][0], value=[asmId][typeId][tc][value]
pub const TC_ARRAY: u8 = 49;

/// Byte edits over ORIGINAL file coordinates: (start, oldlen, new bytes).
pub type Edits = Vec<(usize, usize, Vec<u8>)>;

#[derive(Debug, Clone)]
pub struct Entry {
    pub tc: u8,
    pub id: u32,         // string-map id of the field name (1-based)
    pub name_pos: usize, // first byte of the nameId encoding
    pub plen_pos: usize, // first byte of the payloadLen encoding
    pub plen: usize,     // payload byte length
    pub pstart: usize,   // first payload byte
}

/// A serialized object: [num][total][entries...] starting at `num_start`.
#[derive(Debug, Clone)]
pub struct Obj {
    pub entries: Vec<Entry>,
    pub num_start: usize,
    pub header_end: usize, // == body_start (one past the header)
    pub body_end: usize,   // one past the last entry byte
    pub num: usize,
    pub total: usize,
}

impl Obj {
    /// [num][total] bytes as one span (may re-encode into fewer/more bytes).
    pub fn header_span(&self) -> std::ops::Range<usize> {
        self.num_start..self.header_end
    }

    pub fn header_bytes(&self, num: usize, total: usize) -> Vec<u8> {
        let mut v = write7(num as u64);
        v.extend(write7(total as u64));
        v
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MapMode {
    Once,
    None,
    Per,
}

#[derive(Debug, Clone)]
pub struct SaveData {
    pub data: Vec<u8>,
    pub strings: Vec<String>,
    pub mode: MapMode,
    pub root_start: usize, // position of the root 0x20 typecode byte
    pub root: Obj,
}

impl SaveData {
    pub fn name(&self, id: u32) -> &str {
        match self.strings.get((id - 1) as usize) {
            Some(s) => s.as_str(),
            None => "?",
        }
    }

    pub fn find<'a>(&self, o: &'a Obj, field: &str) -> Option<&'a Entry> {
        o.entries.iter().find(|e| self.name(e.id) == field)
    }

    pub fn sub(&self, e: &Entry) -> Result<Obj> {
        parse_object(&self.data, e.pstart)
    }
}

// --- low-level primitives ---------------------------------------------------

pub fn read7(data: &[u8], mut p: usize) -> Result<(u64, usize)> {
    let (mut val, mut shift) = (0u64, 0u32);
    loop {
        let b = *data.get(p).context("truncated file")? as u64;
        p += 1;
        val += (b & 0x7f) << shift;
        if b < 0x80 {
            break;
        }
        shift += 7;
    }
    Ok((val, p))
}

pub fn write7(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut b = (n % 0x80) as u8;
        n /= 0x80;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
    out
}

pub fn read_u32(data: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(data[p..p + 4].try_into().unwrap())
}

pub fn put_u32(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

pub fn read_u64(data: &[u8], p: usize) -> u64 {
    u64::from_le_bytes(data[p..p + 8].try_into().unwrap())
}

pub fn put_u64(v: u64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// Parse one object body: [num][total][entries...]; `pos` = first byte of num.
pub fn parse_object(data: &[u8], pos: usize) -> Result<Obj> {
    let (num, p1) = read7(data, pos)?;
    let (total, p2) = read7(data, p1)?;
    let mut entries = Vec::new();
    let mut cur = p2;
    for _ in 0..num {
        let tc = *data.get(cur).context("truncated file (entry typecode)")?;
        let (id, q) = read7(data, cur + 1)?;
        let (plen, pstart) = read7(data, q)?;
        entries.push(Entry {
            tc,
            id: id as u32,
            name_pos: cur + 1,
            plen_pos: q,
            plen: plen as usize,
            pstart,
        });
        cur = pstart + plen as usize;
    }
    if cur - p2 != total as usize {
        bail!(
            "object body mismatch: expected total {}, got {}",
            total,
            cur - p2
        );
    }
    Ok(Obj {
        entries,
        num_start: pos,
        header_end: p2,
        body_end: cur,
        num: num as usize,
        total: total as usize,
    })
}

/// Parse the whole file (string map + payload); the payload must consume the
/// file exactly, which is how we reject a wrong map-mode interpretation.
pub fn parse_file(data: Vec<u8>) -> Result<SaveData> {
    if data.len() < 4 || &data[0..3] != b"ceb" {
        bail!("not a .char file (bad magic)");
    }
    if data[3] != 1 {
        bail!("non-compact save format not supported");
    }
    let (count, q) = read7(&data, 4)?;

    for &(mode, start) in
        &[(MapMode::Once, q + 1), (MapMode::None, q), (MapMode::Per, q)]
    {
        if let Ok(sd) = attempt(&data, count as u64, start, mode) {
            return Ok(sd);
        }
    }
    bail!("could not parse string map / payload (unknown layout)");

    fn attempt(data: &[u8], count: u64, start: usize, mode: MapMode) -> Result<SaveData> {
        let mut strings = Vec::new();
        let mut pos = start;
        for _ in 0..count {
            if mode == MapMode::Per {
                if data[pos] != TC_STRING {
                    bail!("map desync");
                }
                pos += 1;
            }
            let (len, r) = read7(data, pos)?;
            strings.push(String::from_utf8(data[r..r + len as usize].to_vec())?);
            pos = r + len as usize;
        }
        if data[pos] != TC_COMPACT_OBJECT {
            bail!("payload does not start with CompactObject");
        }
        let root = parse_object(data, pos + 1)?;
        if root.body_end != data.len() {
            bail!("root object does not consume the file");
        }
        Ok(SaveData {
            data: data.to_vec(),
            strings,
            mode,
            root_start: pos,
            root,
        })
    }
}

/// Apply byte edits sorted descending by start (no-op when out of range).
pub fn apply_edits(mut data: Vec<u8>, mut edits: Edits) -> Vec<u8> {
    edits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    for (start, oldlen, new) in edits {
        data.splice(start..start + oldlen, new);
    }
    data
}