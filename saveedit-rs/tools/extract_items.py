#!/usr/bin/env python3
"""Extract the Romestead item database from decompiled C# SetupItems code.

Reads Shared.Data.Items/SharedDataSetup.cs (ilspycmd output) and emits
src/items_db.json, embedded into the Rust binary via include_str!.

Handles three creation styles found in that file:
  items.Add(new ItemData { ... })   inline blocks
  FoodItem("food:...", ..., ...)    helper calls (foods)
"""
import json
import os
import re

SRC = os.environ.get("ROMESTEAD_ITEM_SETUP_CS",
    "C:/Users/Labmed/AppData/Local/Temp/dec2/Shared.Data.Items/SharedDataSetup.cs")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src", "items_db.json")

HDR = "items.Add(new ItemData"
ITEM_TIER = {0: "trash", 1: "common", 2: "uncommon", 3: "rare",
             4: "epic", 5: "legendary", 6: "exceptional"}


def find_blocks(text, header):
    out, start = [], 0
    while True:
        i = text.find(header, start)
        if i < 0:
            break
        j = text.index("{", i)
        depth, k = 1, j + 1
        while depth:
            if text[k] == "{":
                depth += 1
            elif text[k] == "}":
                depth -= 1
            k += 1
        out.append(text[j + 1:k - 1])
        start = k
    return out


def match_delim(s, i, open_c="{", close_c="}"):
    """Index of the delimiter matching s[i] (which must be open_c), or -1."""
    depth = 0
    k = i
    while k < len(s):
        c = s[k]
        if c == open_c:
            depth += 1
        elif c == close_c:
            depth -= 1
            if depth == 0:
                return k
        k += 1
    return -1


def split_top(args):
    """Split on top-level commas, aware of (), [], {}, quotes and C# generics
    (`Dictionary<string, T>` — angle brackets count as depth)."""
    parts, depth, cur = [], 0, ""
    in_str = False
    prev = ""
    for c in args:
        if in_str:
            cur += c
            if c == '"':
                in_str = False
            continue
        if c == '"':
            in_str = True
            cur += c
        elif c in "([{":
            depth += 1
            cur += c
        elif c in ")]}":
            depth -= 1
            cur += c
        elif c == "<" and (prev.isalnum() or prev == "_"):
            depth += 1  # generics like new Dictionary<string, T>(...)
            cur += c
        elif c == ">" and depth > 0 and (prev.isalnum() or prev == "_"):
            depth -= 1
            cur += c
        elif c == "," and depth == 0:
            parts.append(cur)
            cur = ""
            prev = c
            continue
        else:
            cur += c
        if not c.isspace():
            prev = c
    if cur.strip():
        parts.append(cur)
    return parts


def eval_scalar(expr):
    e = expr.strip()
    if e == "null":
        return None
    while e.endswith(","):
        e = e[:-1]
    if e.startswith('"') and e.endswith('"'):
        return e[1:-1].encode().decode("unicode_escape", errors="ignore")
    if e in ("true", "false"):
        return e == "true"
    m = re.fullmatch(r"MathHelper\.ToRadians\(([\d.]+)f?\)", e)
    if m:
        return round(float(m.group(1)) * 3.14159265358979 / 180.0, 6)
    m = re.fullmatch(r"-?[\d.]+(f|uL|L|u)?", e)
    if m:
        raw = e.rstrip("fLu").rstrip("f")
        x = float(raw) if "." in raw else int(raw)
        return x
    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.]*", e):
        return e.split(".")[-1]
    return e


def parse_value(val_s):
    """Parse any initializer value: scalar, `new T{...}`, `new T(args)`,
    `new Dict(args) {...}` (dictionary) or method call `Food(...)`."""
    val_s = val_s.strip()
    if val_s.startswith("new "):
        inner = val_s[4:].lstrip()
        brace, paren = inner.find("{"), inner.find("(")
        if brace != -1 and (paren == -1 or brace < paren):
            typename = inner[:brace].strip().split(".")[-1]
            k = match_delim(inner, brace)
            sub = parse_assignments(inner[brace + 1:k] if k > 0 else inner[brace + 1:])
            sub["__type"] = typename
            return sub
        if paren != -1:
            tn = inner[:paren].strip().split(".")[-1]
            args_end = match_delim(inner, paren, "(", ")")
            if args_end < 0:
                args_end = len(inner)
            argv = [eval_scalar(x) for x in split_top(inner[paren + 1:args_end])]
            val = {"__ctor": tn, "args": argv}
            rest = inner[args_end + 1:].lstrip()
            if "Dictionary" in inner[:paren] and rest.startswith("{"):
                k = match_delim(rest, 0)
                return parse_dict_init(rest[1:k] if k > 0 else rest[1:])
            if rest.startswith("{"):
                k = match_delim(rest, 0)
                sub = parse_assignments(rest[1:k])
                sub["__type"] = val["__ctor"]
                return sub
            return val
        return eval_scalar(val_s)
    m = re.match(r"^([A-Za-z_][A-Za-z0-9_.]*)\(", val_s)
    if m and val_s.endswith(")"):
        callee = m.group(1)
        if "." in callee:
            return eval_scalar(val_s)  # qualified member access, not a call
        argv = [eval_scalar(x) for x in split_top(val_s[m.end():-1])]
        return {"__ctor": callee, "args": argv}
    return eval_scalar(val_s)


def parse_dict_init(body):
    """Dictionary initializer body: entries shaped `{ "key", value }` or
    `["key"] = value`."""
    out = {}
    i = 0
    while i < len(body):
        if body[i] == "{":
            k = match_delim(body, i)
            if k < 0:
                break
            parts = [p.strip() for p in split_top(body[i + 1:k])]
            if len(parts) >= 2:
                key = eval_scalar(parts[0])
                if key is not None:
                    out[key] = parse_value(parts[1])
            i = k + 1
        elif body.startswith("[", i):
            j = body.find("]", i)
            m = re.match(r'^\[(.+?)\]\s*=\s*', body[i:], re.S)
            if not m or j < 0:
                i += 1
                continue
            key = eval_scalar(m.group(1))
            rest = body[i + m.end():].strip()
            # value ends at next top-level ", {" group or end
            k = match_delim(body, i + m.end()) if rest.startswith("{") else -1
            if k < 0:
                seg = split_top(rest)
                rest = seg[0] if seg else rest
            val = parse_value(rest.rstrip(" ,"))
            if key is not None:
                out[key] = val
            i = i + m.end() + (k + 1 if k >= 0 else len(rest))
        else:
            i += 1
    return out


def parse_assignments(body):
    """Parse `Key = value` / `["key"] = value` pairs at top level."""
    out = {}
    for p in split_top(body):
        p = p.strip()
        if not p or "=" not in p:
            continue
        dm = re.match(r'^\["(.+)"\]\s*=\s*([\s\S]*)$', p)
        if dm:
            key, val_s = dm.group(1), dm.group(2)
        else:
            m2 = re.match(r'^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*([\s\S]*)$', p)
            if not m2:
                continue
            key, val_s = m2.group(1), m2.group(2).strip()
        val = parse_value(val_s)
        if isinstance(val, str) and len(key) > 40:
            continue
        out[key] = val
    return out


def parse_fooditem_calls(text):
    """Parse the `FoodItem("id", ...)` helper calls (foods)."""
    out = []
    idx = 0
    while True:
        m = re.compile(r"^\s+FoodItem\(", re.M).search(text, idx)
        if not m:
            break
        p = text.index("(", m.start())
        end = match_delim(text, p, "(", ")")
        if end < 0:
            break
        a = [x.strip() for x in split_top(text[p + 1:end])]
        idx = end + 1
        if len(a) < 9:
            continue
        it = {
            "id": eval_scalar(a[0]),
            "name": eval_scalar(a[1]),
            "icon": eval_scalar(a[2]),
            "usable": parse_value(a[3]),
            "price": eval_scalar(a[6]),
            "stack": eval_scalar(a[7]),
            "tier": eval_scalar(a[8]),
        }
        if eval_scalar(a[4]) is not None:
            it["citizen_consumable"] = parse_value(a[4])
        if eval_scalar(a[5]) is not None:
            it["description"] = eval_scalar(a[5])
        if len(a) >= 11 and eval_scalar(a[10]) is not None:
            w = parse_value(a[10])
            if isinstance(w, dict):
                args = w.get("args") or []
                it["worship"] = {
                    "godId": args[0] if len(args) > 0 else None,
                    "value": args[1] if len(args) > 1 else None,
                }
        out.append(it)
    return out


def rarity_name(tier):
    if tier is None:
        return "unknown"
    return ITEM_TIER.get(tier, "exceptional")


def extract_item(block):
    d = parse_assignments(block)
    item = {
        "id": d.get("Id"),
        "name": d.get("Name"),
        "icon": d.get("Icon"),
        "stack": d.get("MaxStackSize"),
        "tier": d.get("Tier"),
    }
    if not item["id"] or not isinstance(item["id"], str):
        return None
    item["rarity"] = rarity_name(item["tier"])
    eq = d.get("Equippable")
    if isinstance(eq, dict):
        e = {}
        if eq.get("EquipmentType"):
            e["slot"] = eq["EquipmentType"]
        ws = eq.get("WeaponStats")
        if isinstance(ws, dict):
            w = {}
            cls = ws.get("Class")
            w["class"] = cls.get("Type") if isinstance(cls, dict) else cls
            for k, out_key in [("SwingTimer", "swing"), ("BaseKnockback", "knockback"),
                               ("EnergyCost", "energy"), ("StunPower", "stun")]:
                if k in ws and ws[k] is not None:
                    w[out_key] = ws[k]
            dr = ws.get("DamageRanges")
            if isinstance(dr, dict):
                w["dmgMin"] = (dr.get("MinDamage") or {}).get("args")
                w["dmgMax"] = (dr.get("MaxDamage") or {}).get("args")
            e["weapon"] = w
        ss = eq.get("ShieldStats")
        if isinstance(ss, dict):
            e["shield"] = {k: v for k, v in ss.items() if not k.startswith("__")}
        sb = eq.get("StatBonuses")
        if isinstance(sb, dict):
            e["bonuses"] = {k: v for k, v in sb.items() if not k.startswith("__")}
        item["equip"] = e
    tr = d.get("Tradable")
    if isinstance(tr, dict) and tr.get("Price") is not None:
        item["price"] = tr["Price"]
    us = d.get("Usable")
    if isinstance(us, dict):
        item["usable"] = {k: v for k, v in us.items() if not k.startswith("__")}
    return item


def main():
    text = open(SRC, encoding="utf-8", errors="ignore").read()
    items, seen = [], set()
    for b in find_blocks(text, HDR):
        it = extract_item(b)
        if it and it["id"] not in seen:
            seen.add(it["id"])
            items.append(it)
    for it in parse_fooditem_calls(text):
        if it["id"] and it["id"] not in seen:
            seen.add(it["id"])
            it["rarity"] = rarity_name(it["tier"])
            items.append(it)
    items.sort(key=lambda x: (
        int(x["tier"]) if isinstance(x.get("tier"), (int, float)) else 99,
        str(x["id"]),
    ))
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump(items, f, ensure_ascii=False, separators=(",", ":"))
    rarities = {}
    for it in items:
        rarities[it["rarity"]] = rarities.get(it["rarity"], 0) + 1
    print(f"{len(items)} items -> {os.path.abspath(OUT)}")
    print("rarity counts:", rarities)
    print("sample:", json.dumps(items[10] if len(items) > 10 else items[0], indent=1))


if __name__ == "__main__":
    main()