// Embedded game item database (extracted from decompiled SetupItems via
// tools/extract_items.py into src/items_db.json — 700 items with names,
// rarity, stats, prices).
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Deserialize, Serialize, Clone)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub stack: Option<u32>,
    #[serde(default)]
    pub tier: Option<u32>,
    #[serde(default)]
    pub rarity: String,
    #[serde(default)]
    pub price: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
    // shape-checked at runtime rather than strictly typed (varies per item)
    #[serde(default)]
    pub equip: Option<serde_json::Value>,
    #[serde(default)]
    pub usable: Option<serde_json::Value>,
    #[serde(default)]
    #[allow(dead_code)] // kept for completeness; shown once used in UI
    pub worship: Option<serde_json::Value>,
    #[serde(default)]
    #[allow(dead_code)]
    pub citizen_consumable: Option<serde_json::Value>,
}

/// Shared.Combat.DamageTypes enum order (decompiled): index into the
/// DamageTypesArrays in ItemData.
pub const DAMAGE_TYPES: [&str; 9] = [
    "True", "Slashing", "Piercing", "Bludgeoning", "Pyro", "Chloro", "Aqua", "Cosmo", "Necro",
];

pub fn all() -> &'static [ItemDef] {
    static DB: OnceLock<Vec<ItemDef>> = OnceLock::new();
    DB.get_or_init(|| {
        serde_json::from_str(include_str!("items_db.json"))
            .expect("embedded items_db.json is valid")
    })
}

pub fn get(id: &str) -> Option<&'static ItemDef> {
    all().iter().find(|d| d.id == id)
}

/// Non-zero damage ranges of a def's weapon, e.g. `["10-12"]` (one entry per
/// damage type with values). None when the item has no weapon damage.
pub fn dmg_ranges(d: &ItemDef) -> Option<Vec<String>> {
    let w = d.equip.as_ref()?.as_object()?.get("weapon")?.as_object()?;
    let mins = w.get("dmgMin").and_then(|x| x.as_array())?;
    let maxs = w.get("dmgMax").and_then(|x| x.as_array())?;
    let dmg: Vec<String> = mins
        .iter()
        .zip(maxs.iter())
        .enumerate()
        .filter(|(_, (mn, mx))| {
            mx.as_f64().unwrap_or(0.0) != 0.0 || mn.as_f64().unwrap_or(0.0) != 0.0
        })
        .take(4)
        .map(|(i, (mn, mx))| {
            let lo = mn.as_f64().unwrap_or(0.0).round();
            let hi = mx.as_f64().unwrap_or(0.0).round();
            format!(
                "{} {}-{}",
                DAMAGE_TYPES.get(i).copied().unwrap_or("?"),
                fmt_num(lo),
                fmt_num(hi)
            )
        })
        .collect();
    if dmg.is_empty() {
        None
    } else {
        Some(dmg)
    }
}

/// Charges the game would give this item on pickup
/// (`ItemInstanceModel.CalculateStats`: UsesLeft = UsesMax when UsesMax > 0).
/// None = item is not usable in-game. Foods (`Food(...)`, UsesMax=1) count as 1.
pub fn uses_left(d: &ItemDef) -> Option<u32> {
    use serde_json::Value as V;
    let u = d.usable.as_ref()?;
    match u {
        V::Object(o) if o.get("__ctor").is_some() => Some(1),
        V::Object(o) => match o.get("UsesMax").and_then(|x| x.as_u64()) {
            Some(n) if n > 0 => Some(n as u32),
            Some(_) => Some(0), // usable exists, zero charges -> empty UsableInfo
            None => None,
        },
        _ => None,
    }
}

/// One-line human summary of an item's stats (weapon damage, bonuses, price...).
pub fn describe(d: &ItemDef) -> String {
    use serde_json::Value as V;
    let mut parts: Vec<String> = Vec::new();
    if let Some(V::Object(e)) = &d.equip {
        if let Some(V::String(slot)) = e.get("slot") {
            parts.push((*slot).to_string());
        }
        if let Some(V::Object(w)) = e.get("weapon") {
            if let Some(V::String(cls)) = w.get("class") {
                parts.push(format!("weapon {}", cls.to_lowercase()));
            }
            if let Some(dmg) = dmg_ranges(d) {
                let ranges: Vec<String> = dmg
                    .iter()
                    .enumerate()
                    .map(|(_, r)| r.to_string())
                    .collect();
                parts.push(format!("dmg [{}]", ranges.join(", ")));
            }
            for (k, label) in [("swing", "sw "), ("energy", "en "), ("stun", "st ")] {
                if let Some(v) = w.get(k).and_then(|x| x.as_f64()) {
                    parts.push(format!("{}{}", label, fmt_num(v)));
                }
            }
        }
        if let Some(V::Object(sb)) = e.get("bonuses") {
            let mut names: Vec<&String> = sb.keys().collect();
            names.sort();
            for n in names {
                let b = &sb[n];
                let mut s = String::new();
                if let Some(a) = b.get("Additive").and_then(|x| x.as_f64()) {
                    s.push_str(&format!("{}{}", if a >= 0.0 { "+" } else { "" }, fmt_num(a)));
                }
                if let Some(m) = b.get("Multiplier").and_then(|x| x.as_f64()) {
                    if !s.is_empty() {
                        s.push(' ');
                    }
                    s.push_str(&format!("x{}", fmt_num(m)));
                }
                if s.is_empty() {
                    for extra in ["AdditiveMultiplier", "BaseMultiplier", "BonusMultiplier"] {
                        if let Some(a) = b.get(extra).and_then(|x| x.as_f64()) {
                            s.push_str(&format!("x{}", fmt_num(a)));
                            break;
                        }
                    }
                }
                parts.push(format!("{} {s}", n));
            }
        }
    }
    if d.stack.unwrap_or(1) > 1 {
        parts.push(format!("stack {}", d.stack.unwrap_or(1)));
    }
    if let Some(u) = &d.usable {
        match u {
            serde_json::Value::Object(o) if o.get("__ctor").is_some() => {
                let args: Vec<String> = o
                    .get("args")
                    .and_then(|a| a.as_array())
                    .map(|a| a.iter().map(fmt_json).collect())
                    .unwrap_or_default();
                parts.push(format!("{}({})", fmt_json(o.get("__ctor").unwrap()), args.join(", ")));
            }
            serde_json::Value::Object(o) => {
                if let Some(sp) = o.get("SpellId").and_then(|x| x.as_str()) {
                    parts.push(format!("usable {sp}"));
                }
            }
            _ => {}
        }
    }
    if let Some(price) = d.price {
        parts.push(format!("{price} gold"));
    }
    parts.join(" · ")
}

fn fmt_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

pub fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_loads_700_items() {
        assert!(all().len() > 600, "db has {} items", all().len());
    }

    #[test]
    fn known_items_resolve() {
        let w = get("weapon:water:sword").expect("water sword in db");
        assert_eq!(w.name, "Gladius Aquaticus");
        assert_eq!(w.rarity, "uncommon");
        let f = get("food:crab_meat").expect("crab meat in db");
        assert_eq!(f.name, "Strange crab Meat");
    }

    #[test]
    fn describe_contains_damage_and_price() {
        let w = get("weapon:water:sword").unwrap();
        let s = describe(w);
        assert!(s.contains("10"), "describe: {s}");
        assert!(s.contains("250"), "price in describe: {s}");
    }
}