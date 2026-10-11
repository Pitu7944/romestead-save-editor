// String-map key management + generic field-entry insertion.

use crate::format::*;
use anyhow::*;
use std::collections::HashMap;

/// Collects string-map keys wanted by a builder, then emits one count-bump
/// edit + one append edit for all new keys at once (ids stay sequential).
pub struct KeyBuilder {
    reserved: HashMap<String, u32>,
    added: u64,
    appended: Vec<u8>, // bytes to insert right after the last map key
}

impl KeyBuilder {
    pub fn new() -> Self {
        Self {
            reserved: HashMap::new(),
            added: 0,
            appended: Vec::new(),
        }
    }

    /// Get the string-id for `name`, reserving a new slot when needed.
    pub fn ensure(&mut self, sf: &SaveData, name: &str) -> u32 {
        if let Some(id) = self.reserved.get(name) {
            return *id;
        }
        if let Some(i) = sf.strings.iter().position(|s| s == name) {
            let id = (i + 1) as u32;
            self.reserved.insert(name.to_string(), id);
            return id;
        }
        let id = (sf.strings.len() as u64 + self.added + 1) as u32;
        self.reserved.insert(name.to_string(), id);
        self.added += 1;
        // "per" mode writes a 0x21 typecode before EVERY element; the leading
        // 0x21 in "once"/"none" modes belongs to the list header (or nothing).
        if sf.mode == MapMode::Per {
            self.appended.push(TC_STRING);
        }
        let enc = write7(name.len() as u64);
        let mut chunk = enc;
        chunk.extend_from_slice(name.as_bytes());
        self.appended.extend(chunk);
        id
    }

    /// Emit count-bump + map-append edits; call once after all `ensure`.
    pub fn finalize(&mut self, sf: &SaveData) -> Edits {
        if self.added == 0 {
            return Vec::new();
        }
        let n = self.added;
        self.added = 0;
        let (count, q) = read7(&sf.data, 4).unwrap();
        let appended = std::mem::take(&mut self.appended);
        vec![
            (4, (q - 4) as usize, write7(count + n)),
            (sf.root_start, 0, appended),
        ]
    }
}

/// Insert a pre-built field entry at the end of `parent`'s body and grow the
/// containers: parent header, the parent's own entry length in its enclosing
/// object (`outer`), and the root total.
pub fn insert_field(
    sf: &SaveData,
    parent: &Obj,
    outer: Option<&Entry>,
    edits: &mut Edits,
    entry_bytes: Vec<u8>,
) -> Result<()> {
    let ins = entry_bytes.len();
    if outer.is_some() {
        // the parent's header is itself inside a length-prefixed payload
        let _ = sf;
    }
    let hdr = write7((parent.num + 1) as u64)
        .into_iter()
        .chain(write7((parent.total + ins) as u64))
        .collect::<Vec<u8>>();
    ensure!(
        hdr.len() == parent.header_end - parent.num_start,
        "object header width change — refusing (save layout newer than this tool)"
    );
    edits.push((parent.body_end, 0, entry_bytes));
    edits.push((parent.num_start, parent.header_end - parent.num_start, hdr));

    if let Some(o) = outer {
        let new_len = o.plen + ins;
        ensure!(
            write7(new_len as u64).len() == o.pstart - o.plen_pos,
            "entry length width change — refusing"
        );
        edits.push((o.plen_pos, o.pstart - o.plen_pos, write7(new_len as u64)));
        let root_total = sf.root.total + ins;
        let (_, p1) = read7(&sf.data, sf.root.num_start)?;
        let root_total_enc = write7(sf.root.total as u64);
        ensure!(
            write7(root_total as u64).len() == root_total_enc.len(),
            "root total width change — refusing"
        );
        edits.push((p1, root_total_enc.len(), write7(root_total as u64)));
    }
    Ok(())
}
