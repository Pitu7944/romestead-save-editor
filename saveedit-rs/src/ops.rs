// High-level edits: difficulty, money, give/remove/stack. Each returns
// (Edits, message, verification spec) and everything goes through run().

use crate::format::*;
use crate::items_db;
use crate::keys::{insert_field, KeyBuilder};
use anyhow::{bail, ensure, Context, Result};

pub enum Wants {
    Difficulty(u32), // 0 = Normal, 1 = Softcore
    Money(u64),             // new value
    Item(ItemSpec),         // give
    Remove(&'static str, usize), // target slot
    Stack(&'static str, usize, u32), // target slot new count
    Order(&'static str, Vec<String>), // non-null item ids in array order
}

pub struct ItemSpec {
    pub target: &'static str, // Inventory / Equipment / SecondaryEquipment
    pub id: String,           // BaseDataId
    pub count: u32,           // StackCount
    pub slot: Option<usize>,  // 1-based; None = first free slot
}

pub struct Built {
    pub edits: Edits,
    pub msg: String,
    pub verify: Wants,
}

fn diff_payload(v: u32) -> Vec<u8> {
    let mut p = vec![TC_INT32];
    p.extend(put_u32(v));
    p
}

fn no_change(msg: String) -> Result<Built> {
    Err(crate::NoChange(msg).into())
}

/// One queued builder for batch operations (merge, batch-give). Built against
/// whatever the save looks like when the step runs.
pub type Step = Box<dyn FnOnce(&SaveData) -> Result<Built>>;

pub fn build_difficulty(sf: &SaveData, softcore: bool) -> Result<Built> {
    let want = if softcore { 1u32 } else { 0 };
    let payload = diff_payload(want);
    let cm = sf
        .find(&sf.root, "CharacterModel")
        .context("CharacterModel field not found in root object")?;
    let cmo = sf.sub(cm)?;
    let cur = sf.find(&cmo, "Difficulty");

    match cur {
        Some(d) => {
            ensure!(d.plen == 5, "Difficulty present but unexpected length {}", d.plen);
            let now = read_u32(&sf.data, d.pstart + 1);
            if now == want {
                return no_change(format!(
                    "already {}",
                    if softcore { "Softcore" } else { "Normal" }
                ));
            }
            Ok(Built {
                edits: vec![(d.pstart, 5, payload)],
                msg: format!(
                    "changing Difficulty to {}",
                    if softcore { "Softcore" } else { "Normal" }
                ),
                verify: Wants::Difficulty(want),
            })
        }
        None => {
            if !softcore {
                return no_change("already Normal (field absent)".to_string());
            }
            let mut kb = KeyBuilder::new();
            let name_id = kb.ensure(sf, "Difficulty");
            let mut edits = kb.finalize(sf);
            let entry = vec![TC_ENUM].into_iter().chain(write7(name_id as u64))
                .chain(write7(5)).chain(payload.clone()).collect();
            insert_field(sf, &cmo, Some(cm), &mut edits, entry)?;
            Ok(Built {
                edits,
                msg: "inserting Difficulty field (Softcore)".to_string(),
                verify: Wants::Difficulty(want),
            })
        }
    }
}

pub fn build_money(sf: &SaveData, amount: u64) -> Result<Built> {
    let payload = put_u64(amount);
    match sf.find(&sf.root, "Money") {
        Some(m) => {
            ensure!(m.tc == TC_UINT64 && m.plen == 8, "Money field has unexpected layout");
            let now = read_u64(&sf.data, m.pstart);
            if now == amount {
                return no_change(format!("Money already {}", amount));
            }
            Ok(Built {
                edits: vec![(m.pstart, 8, payload)],
                msg: format!("changing Money to {}", amount),
                verify: Wants::Money(amount),
            })
        }
        None => {
            if amount == 0 {
                return no_change("Money already 0 (field absent)".to_string());
            }
            let mut kb = KeyBuilder::new();
            let name_id = kb.ensure(sf, "Money");
            let mut edits = kb.finalize(sf);
            let entry = vec![TC_UINT64].into_iter().chain(write7(name_id as u64))
                .chain(write7(8)).chain(payload.clone()).collect();
            insert_field(sf, &sf.root, None, &mut edits, entry)?;
            Ok(Built {
                edits,
                msg: format!("inserting Money field ({})", amount),
                verify: Wants::Money(amount),
            })
        }
    }
}

fn guid() -> Vec<u8> {
    let mut g = uuid::Uuid::new_v4().into_bytes().to_vec();
    g[15] = (g[15] & 0x3f) | 0x80; // RFC4122 variant bits
    g
}

/// Build a full ItemInstanceModel element as the game would serialize it:
/// [asmId][typeId][0x20][num][total][fields in declaration order]:
/// BaseDataId, Id, InventoryId, Auras (always, even empty), StackCount and —
/// when the item is usable — `UsableInfo { UsesLeft }` exactly like
/// `ItemInstanceModel.CalculateStats` produces it.
pub fn item_element(
    sf: &SaveData,
    kb: &mut KeyBuilder,
    base_id: &str,
    count: u32,
    uses_left: Option<u32>,
) -> Vec<u8> {
    let asm = kb.ensure(sf, "Shared");
    let ty = kb.ensure(sf, "Shared.Models.Items.ItemInstanceModel");
    let f_base = kb.ensure(sf, "BaseDataId");
    let v_base = kb.ensure(sf, base_id);
    let f_id = kb.ensure(sf, "Id");
    let f_inv = kb.ensure(sf, "InventoryId");
    let a_sys = kb.ensure(sf, "System.Private.CoreLib");
    let t_guid = kb.ensure(sf, "System.Guid");
    let f_auras = kb.ensure(sf, "Auras");
    let f_stack = kb.ensure(sf, "StackCount");
    let f_usable = uses_left.map(|_| kb.ensure(sf, "UsableInfo"));
    let f_uses = uses_left.map(|_| kb.ensure(sf, "UsesLeft"));

    let g = guid();

    let mut base = vec![TC_STRING];
    base.extend(write7(f_base as u64));
    let val = write7(v_base as u64);
    base.extend(write7(val.len() as u64));
    base.extend(val);

    let mut id_field = vec![TC_GUID];
    id_field.extend(write7(f_id as u64));
    id_field.extend(write7(16));
    id_field.extend_from_slice(&g);

    // InventoryId: Guid? via interface typecode: [asmId][typeId][0x25][16B]
    let mut inv_id = vec![TC_INTERFACE];
    inv_id.extend(write7(f_inv as u64));
    let mut inv_payload = write7(a_sys as u64);
    inv_payload.extend(write7(t_guid as u64));
    inv_payload.push(TC_GUID);
    inv_payload.extend_from_slice(&g);
    inv_id.extend(write7(inv_payload.len() as u64));
    inv_id.extend(inv_payload);

    let mut auras = vec![TC_ARRAY];
    auras.extend(write7(f_auras as u64));
    auras.extend(write7(1));
    auras.push(0);

    let mut stack = vec![TC_INT32];
    stack.extend(write7(f_stack as u64));
    stack.extend(write7(4));
    stack.extend(put_u32(count));

    let mut entries = base;
    entries.extend(id_field);
    entries.extend(inv_id);
    entries.extend(auras);
    entries.extend(stack);

    let mut usable = None;
    if let Some(uses) = uses_left {
        // UsableItemInstance: only UsesLeft is non-default when 0 -> empty object
        if uses > 0 {
            let mut entry = vec![TC_INT32];
            entry.extend(write7(f_uses.unwrap() as u64));
            entry.extend(write7(4));
            entry.extend(put_u32(uses));
            let mut body = write7(1); // num fields
            body.extend(write7(entry.len() as u64)); // total len
            body.extend(entry);
            let mut info = vec![TC_COMPACT_OBJECT];
            info.extend(write7(f_usable.unwrap() as u64));
            info.extend(write7(body.len() as u64));
            info.extend(body);
            usable = Some(info);
        } else {
            // all-default UsableItemInstance: object with zero entries
            let mut info = vec![TC_COMPACT_OBJECT];
            info.extend(write7(f_usable.unwrap() as u64));
            info.extend(write7(2));
            info.extend([0u8, 0u8]);
            usable = Some(info);
        }
        entries.extend(usable.unwrap());
    }

    let mut elem = write7(asm as u64);
    elem.extend(write7(ty as u64));
    elem.push(TC_COMPACT_OBJECT);
    elem.extend(write7(5 + if uses_left.is_some() { 1 } else { 0 }));
    elem.extend(write7(entries.len() as u64));
    elem.extend(entries);
    elem
}

pub fn build_give(sf: &SaveData, spec: &ItemSpec) -> Result<Built> {
    let e = sf.find(&sf.root, spec.target).context(format!(
        "{} field not found in save", spec.target
    ))?;
    let arr = parse_array_data(sf, e)?;

    let slot = match spec.slot {
        Some(s) => {
            let el = arr.elems.get(s.wrapping_sub(1))
                .context("slot out of range")?;
            ensure!(el.is_null, "slot {} is occupied", s);
            s
        }
        None => match arr.elems.iter().position(|e| e.is_null) {
            Some(i) => i + 1,
            None => bail!("{} is full — remove something first", spec.target),
        },
    };

    // usable items get UsableInfo just like the game's CalculateStats would
    let uses_left = items_db::get(&spec.id).and_then(items_db::uses_left);

    let mut kb = KeyBuilder::new();
    let elem = item_element(sf, &mut kb, &spec.id, spec.count, uses_left);
    let mut edits = kb.finalize(sf);

    // replace the 2-byte null marker (array count itself stays unchanged)
    let target = &arr.elems[slot - 1];
    let ins = elem.len() - 2;
    let new_plen = e.plen + ins;
    ensure!(
        write7(new_plen as u64).len() == e.pstart - e.plen_pos,
        "entry length width change — refusing"
    );
    edits.push((target.start, 2, elem));
    edits.push((e.plen_pos, e.pstart - e.plen_pos, write7(new_plen as u64)));
    let root_total = sf.root.total + ins;
    ensure_grow_root(sf, &mut edits, root_total)?;
    Ok(Built {
        edits,
        msg: format!("{} slot {}: added {} x{}", spec.target, slot, spec.id, spec.count),
        verify: Wants::Item(ItemSpec {
            target: spec.target,
            id: spec.id.clone(),
            count: spec.count,
            slot: Some(slot),
        }),
    })
}

pub fn build_remove(sf: &SaveData, target: &'static str, slot: usize) -> Result<Built> {
    let e = sf.find(&sf.root, target).context(format!("{} not found", target))?;
    let arr = parse_array_data(sf, e)?;
    let el = arr.elems.get(slot - 1).context("slot out of range")?;
    ensure!(!el.is_null, "slot {} is already empty", slot);
    ensure!(arr.elem_tc == TC_INTERFACE, "removing from a non-interface array is not supported");
    let oldlen = el.end_ - el.start + 1;
    let grow = 2usize as i64 - oldlen as i64; // negative

    let mut edits = vec![(el.start, oldlen, vec![0u8, 0u8])];
    let new_plen = (e.plen as i64 + grow) as usize;
    ensure!(
        write7(new_plen as u64).len() == e.pstart - e.plen_pos,
        "entry length width change on shrink — refusing"
    );
    edits.push((e.plen_pos, e.pstart - e.plen_pos, write7(new_plen as u64)));
    let root_total = (sf.root.total as i64 + grow) as usize;
    ensure_grow_root(sf, &mut edits, root_total)?;
    Ok(Built {
        edits,
        msg: format!("{} slot {}: item removed", target, slot),
        verify: Wants::Remove(target, slot),
    })
}

pub fn build_stack(sf: &SaveData, target: &'static str, slot: usize, count: u32) -> Result<Built> {
    let e = sf.find(&sf.root, target).context(format!("{} not found", target))?;
    let arr = parse_array_data(sf, e)?;
    let el = arr.elems.get(slot - 1).context("slot out of range")?;
    ensure!(!el.is_null, "slot {} is empty — nothing to stack", slot);
    let obj = sf.sub(&el.entry)?;

    let mut edits = Vec::new();
    match sf.find(&obj, "StackCount") {
        Some(s) => {
            ensure!(s.tc == TC_INT32 && s.plen == 4, "StackCount has unexpected layout");
            edits.push((s.pstart, 4, put_u32(count)));
        }
        None => {
            let mut kb = KeyBuilder::new();
            let name_id = kb.ensure(sf, "StackCount");
            let mut sub_edits = kb.finalize(sf);
            let entry = vec![TC_INT32].into_iter().chain(write7(name_id as u64))
                .chain(write7(4)).chain(put_u32(count)).collect::<Vec<u8>>();
            let ins = entry.len();
            let hdr = obj.header_bytes(obj.num + 1, obj.total + ins);
            ensure!(
                hdr.len() == obj.header_end - obj.num_start,
                "element header width change — refusing"
            );
            sub_edits.push((obj.body_end, 0, entry));
            sub_edits.push((obj.num_start, obj.header_end - obj.num_start, hdr));
            let new_plen = e.plen + ins;
            ensure!(
                write7(new_plen as u64).len() == e.pstart - e.plen_pos,
                "entry length width change — refusing"
            );
            sub_edits.push((e.plen_pos, e.pstart - e.plen_pos, write7(new_plen as u64)));
            let root_total = sf.root.total + ins;
            ensure_grow_root(sf, &mut sub_edits, root_total)?;
            edits.extend(sub_edits);
        }
    }
    Ok(Built {
        edits,
        msg: format!("{} slot {}: StackCount -> {}", target, slot, count),
        verify: Wants::Stack(target, slot, count),
    })
}

fn ensure_grow_root(sf: &SaveData, edits: &mut Edits, root_total: usize) -> Result<()> {
    let (num, p1) = read7(&sf.data, sf.root.num_start)?;
    let _ = num;
    let total_enc = write7(sf.root.total as u64);
    ensure!(
        write7(root_total as u64).len() == total_enc.len(),
        "root total width change — refusing"
    );
    edits.push((p1, total_enc.len(), write7(root_total as u64)));
    Ok(())
}

// --- sort & merge ------------------------------------------------------------

/// Sort key used everywhere: best rarity first (tier desc), unknown-tier items
/// last, then id. Returns the tier order key for an item id.
pub fn rarity_rank(id: &str) -> (bool, u32) {
    match items_db::get(id).and_then(|d| d.tier) {
        Some(t) => (false, u32::MAX - t), // low key = better rarity
        None => (true, 0),               // unknown after everything
    }
}

/// Reorder Inventory by rarity (best first, then id) and move empty slots to
/// the end. Pure permutation of same-length element ranges — no size changes.
pub fn build_sort_inventory(sf: &SaveData) -> Result<Built> {
    sort_array(sf, "Inventory")
}

fn sort_array(sf: &SaveData, target: &'static str) -> Result<Built> {
    let e = sf
        .find(&sf.root, target)
        .with_context(|| format!("{} field not found in save", target))?;
    let arr = parse_array_data(sf, e)?;
    ensure!(
        !arr.elems.is_empty(),
        "{} array is empty — nothing to sort",
        target
    );
    // occupied slots in current order with sort keys
    let mut occ: Vec<(usize, String)> = Vec::new();
    for (i, el) in arr.elems.iter().enumerate() {
        if !el.is_null {
            let info = slot_info(sf, el)
                .with_context(|| format!("{} slot {} unreadable", target, i + 1))?;
            occ.push((i, info.base_name));
        }
    }
    let mut sorted: Vec<(usize, String)> = occ.clone();
    sorted.sort_by(|a, b| rarity_rank(&a.1).cmp(&rarity_rank(&b.1)).then_with(|| a.1.cmp(&b.1)));
    // already fully sorted AND empties only at the end?
    if sorted == occ && occ.iter().enumerate().all(|(n, (i, _))| *i == n) {
        return no_change(format!("{} already sorted", target));
    }
    let wanted: Vec<String> = sorted.iter().map(|(_, id)| id.clone()).collect();

    // every null element must be exactly [0][0] so they can be re-placed
    for (i, el) in arr.elems.iter().enumerate() {
        if el.is_null {
            ensure!(
                el.end_ + 1 == el.start + 2
                    && sf.data[el.start] == 0
                    && sf.data[el.start + 1] == 0,
                "{} slot {} is empty but not in the expected 2-byte form — refusing",
                target,
                i + 1
            );
        }
    }

    // new element span: sorted occupied elements, then nulls
    let mut body: Vec<u8> = Vec::new();
    for (i, _) in &sorted {
        let el = &arr.elems[*i];
        body.extend_from_slice(&sf.data[el.start..=el.end_]);
    }
    for _ in 0..arr.count - sorted.len() {
        body.extend_from_slice(&[0u8, 0u8]);
    }
    let first = arr.elems[0].start;
    let last = arr.elems[arr.count - 1].end_;
    let span = last - first + 1;
    ensure!(body.len() == span, "element span changed size — refusing");
    let edits: Edits = vec![(first, span, body)];
    Ok(Built {
        edits,
        msg: format!(
            "{} sorted: {} items by rarity ({} empty slots now at the end)",
            target,
            sorted.len(),
            arr.count - sorted.len()
        ),
        verify: Wants::Order(target, wanted),
    })
}

/// Merge duplicate stacks across every listed target (default all three
/// arrays). Returns sequential steps; each fills the earliest slot up to the
/// item's MaxStackSize and empties the drained ones.
pub fn build_merge(sf: &SaveData, targets: &[&'static str]) -> Result<Vec<Step>> {
    let mut steps: Vec<Step> = Vec::new();
    for target in targets {
        let e = sf
            .find(&sf.root, target)
            .with_context(|| format!("{} field not found in save", target))?;
        let arr = parse_array_data(sf, e)?;
        let mut per_id: std::collections::BTreeMap<String, Vec<(usize, u32)>> =
            std::collections::BTreeMap::new();
        for (i, el) in arr.elems.iter().enumerate() {
            if let Some(info) = slot_info(sf, el) {
                per_id.entry(info.base_name).or_default().push((i + 1, info.count));
            }
        }
        for (id, mut slots) in per_id {
            if slots.len() < 2 {
                continue;
            }
            slots.sort_by_key(|(s, _)| *s);
            // cap from the game DB; without a known MaxStack we do not touch it
            let Some(cap) = items_db::get(&id).and_then(|d| d.stack) else {
                continue;
            };
            let total: u64 = slots.iter().map(|(_, c)| *c as u64).sum();
            let mut running = total;
            for (slot, cnt) in slots {
                let want = running.min(cap as u64) as u32;
                running -= want as u64;
                if want != cnt {
                    let t = *target;
                    match want {
                        0 => steps.push(Box::new(move |sf| build_remove(sf, t, slot))),
                        _ => steps.push(Box::new(move |sf| build_stack(sf, t, slot, want))),
                    }
                }
            }
        }
    }
    if steps.is_empty() {
        return Err(crate::NoChange(
            "nothing to merge (no duplicate stacks with known max)".into(),
        )
        .into());
    }
    Ok(steps)
}

// --- item arrays -------------------------------------------------------------

pub struct ArrayView {
    pub count: usize,
    pub elem_tc: u8,
    pub elems: Vec<ArrayElem>,
}

pub struct ArrayElem {
    pub start: usize, // first byte of the element
    pub end_: usize,  // last byte (inclusive)
    pub is_null: bool,
    pub entry: Entry, // for objects: the sub-object parsed from the data
}

/// Parse an object array ([7bit count][elemTc][elements...]) whose elements
/// are either interface-typed (tc 48; null = [0][0]) or plain compact objects.
pub fn parse_array_data(sf: &SaveData, e: &Entry) -> Result<ArrayView> {
    let d = &sf.data;
    let (cnt, q) = read7(d, e.pstart)?;
    let elem_tc = d[q];
    if elem_tc != TC_INTERFACE && elem_tc != TC_COMPACT_OBJECT {
        bail!("array element typecode {} not supported for editing", elem_tc);
    }
    let mut elems = Vec::new();
    let mut pos = q + 1;
    for _ in 0..cnt {
        if elem_tc == TC_INTERFACE {
            let (asm, nq) = read7(d, pos)?;
            let (tid, nq2) = read7(d, nq)?;
            if asm == 0 && tid == 0 {
                elems.push(ArrayElem {
                    start: pos,
                    end_: nq2 - 1,
                    is_null: true,
                    entry: Entry {
                        tc: 0, id: 0, name_pos: 0, plen_pos: 0, plen: 0, pstart: 0,
                    },
                });
                pos = nq2;
            } else {
                if d[nq2] != TC_COMPACT_OBJECT {
                    bail!("unexpected interface element typecode {}", d[nq2]);
                }
                let obj = parse_object(d, nq2 + 1)?;
                let pstart_val = nq2 + 1; // the element's own [num]
                elems.push(ArrayElem {
                    start: pos,
                    end_: obj.body_end - 1,
                    is_null: false,
                    entry: Entry {
                        tc: TC_COMPACT_OBJECT,
                        id: 0,
                        name_pos: 0,
                        plen_pos: 0,
                        plen: obj.total,
                        pstart: pstart_val,
                    },
                });
                pos = obj.body_end;
            }
        } else {
            let obj = parse_object(d, pos)?;
            let pstart_val = pos;
            elems.push(ArrayElem {
                start: pos,
                end_: obj.body_end - 1,
                is_null: false,
                entry: Entry {
                    tc: TC_COMPACT_OBJECT,
                    id: 0,
                    name_pos: 0,
                    plen_pos: 0,
                    plen: obj.total,
                    pstart: pstart_val,
                },
            });
            pos = obj.body_end;
        }
    }
    Ok(ArrayView {
        count: cnt as usize,
        elem_tc,
        elems,
    })
}

pub struct SlotInfo {
    pub base_name: String,
    pub count: u32,
    pub has_usable: bool,
}

pub fn slot_info(sf: &SaveData, el: &ArrayElem) -> Option<SlotInfo> {
    if el.is_null {
        return None;
    }
    let d = &sf.data;
    let obj = sf.sub(&el.entry).ok()?;
    let base = sf.find(&obj, "BaseDataId")?; // compact string -> string id
    let sid = read7(d, base.pstart).ok()?.0 as u32;
    let base_name = sf.name(sid).to_string();
    let count = sf
        .find(&obj, "StackCount")
        .map(|s| read_u32(d, s.pstart))
        .unwrap_or(1);
    let has_usable = sf.find(&obj, "UsableInfo").is_some();
    Some(SlotInfo {
        base_name,
        count,
        has_usable,
    })
}

// --- verification (re-parse the patched bytes and assert the new state) ------

pub fn verify(patched: &[u8], expects: &Wants) -> Result<()> {
    let sf = parse_file(patched.to_vec())?;
    match expects {
        Wants::Difficulty(v) => {
            let cm = sf
                .find(&sf.root, "CharacterModel")
                .context("CharacterModel missing after patch")?;
            let cmo = sf.sub(cm)?;
            let d = sf.find(&cmo, "Difficulty").context("Difficulty missing after patch")?;
            ensure!(d.plen == 5, "Difficulty payload wrong after patch");
            ensure!(
                read_u32(&sf.data, d.pstart + 1) == *v,
                "Difficulty value wrong after patch"
            );
        }
        Wants::Money(v) => {
            let m = sf.find(&sf.root, "Money").context("Money missing after patch")?;
            ensure!(m.plen == 8 && read_u64(&sf.data, m.pstart) == *v, "Money wrong after patch");
        }
        Wants::Item(spec) => {
            let e = sf_find_target(&sf, spec.target)?;
            let arr = parse_array_data(&sf, e)?;
            let el = arr.elems.get(spec.slot.unwrap() - 1).context("slot missing after patch")?;
            let info = slot_info(&sf, el).context("slot empty after patch")?;
            ensure!(info.base_name == spec.id, "item id wrong after patch");
            ensure!(info.count == spec.count, "stack count wrong after patch");
            let expect_usable = items_db::get(&spec.id).and_then(items_db::uses_left).is_some();
            if expect_usable && !info.has_usable {
                bail!("UsableInfo missing after patch");
            }
        }
        Wants::Remove(target, slot) => {
            let e = sf_find_target(&sf, target)?;
            let arr = parse_array_data(&sf, e)?;
            let el = arr.elems.get(*slot - 1).context("slot missing after patch")?;
            ensure!(el.is_null, "slot not emptied after patch");
        }
        Wants::Stack(target, slot, count) => {
            let e = sf_find_target(&sf, target)?;
            let arr = parse_array_data(&sf, e)?;
            let el = arr.elems.get(*slot - 1).context("slot missing after patch")?;
            let obj = sf.sub(&el.entry)?;
            let s = sf
                .find(&obj, "StackCount")
                .context("StackCount missing after patch")?;
            ensure!(read_u32(&sf.data, s.pstart) == *count, "StackCount wrong after patch");
        }
        Wants::Order(target, wanted) => {
            let e = sf_find_target(&sf, target)?;
            let arr = parse_array_data(&sf, e)?;
            let got: Vec<String> = arr
                .elems
                .iter()
                .filter_map(|el| slot_info(&sf, el).map(|i| i.base_name))
                .collect();
            ensure!(&got == wanted, "item order wrong after sort (got {:?})", got);
        }
    }
    Ok(())
}

fn sf_find_target<'a>(sf: &'a SaveData, target: &str) -> Result<&'a Entry> {
    sf.find(&sf.root, target).context(format!("{} missing after patch", target))
}
#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::ops::Wants;

    /// Every UsableInfo entry in a real save has the shape our give-path
    /// synthesizes: a direct CompactObject containing UsesLeft (Int32).
    #[test]
    fn real_usable_info_layout_matches_give_synthesis() {
        let data = std::fs::read("../Adolfina.char").unwrap();
        let sf = parse_file(data).unwrap();
        let mut found = 0;
        for t in ["Inventory", "Equipment", "SecondaryEquipment"] {
            let Some(e) = sf.find(&sf.root, t) else { continue };
            let arr = parse_array_data(&sf, e).unwrap();
            for el in arr.elems.iter() {
                if el.is_null {
                    continue;
                }
                let obj = sf.sub(&el.entry).unwrap();
                if let Some(u) = sf.find(&obj, "UsableInfo") {
                    found += 1;
                    assert_eq!(u.tc, TC_COMPACT_OBJECT, "UsableInfo is a plain object");
                    let sub = sf.sub(u).unwrap();
                    let uses = sf
                        .find(&sub, "UsesLeft")
                        .expect("UsableInfo must carry UsesLeft");
                    assert_eq!(uses.tc, TC_INT32);
                    assert_eq!(uses.plen, 4);
                    assert!(read_u32(&sf.data, uses.pstart) > 0);
                }
            }
        }
        assert!(found > 0, "the test save should contain usable items");
    }

    /// Gifted consumables must come out usable: same field the game writes.
    #[test]
    fn gifted_food_has_usable_info() {
        let data = std::fs::read("../Marcella.char").unwrap();
        let sf = parse_file(data).unwrap();
        let built = crate::ops::build_give(
            &sf,
            &crate::ops::ItemSpec {
                target: "Inventory",
                id: "food:crab_meat".into(),
                count: 10,
                slot: None,
            },
        )
        .unwrap();
        let patched = crate::format::apply_edits(sf.data.clone(), built.edits.clone());
        verify(&patched, &built.verify).expect("gifted food must be usable");
    }
}
