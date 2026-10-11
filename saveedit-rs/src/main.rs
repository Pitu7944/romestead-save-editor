// saveedit — Romestead .char save editor (CLI + TUI)
//
//   saveedit dump       <file>
//   saveedit difficulty <file> <softcore|normal> [outfile]
//   saveedit money      <file> <amount>          [outfile]
//   saveedit items      <file> [inventory|equipment|secondary]
//   saveedit give       <file> <itemId> [count] [target] [slot] [outfile]
//   saveedit remove     <file> <slot>   [target] [outfile]
//   saveedit stack      <file> <slot> <count> [target] [outfile]
//   saveedit tui        [folder]
//   saveedit merge/mergeall/sortinv/batchgive
//   saveedit db         [filter]
//
// Without outfile, files are modified in place — a .bak of the ORIGINAL is
// always written first, and every change is round-trip verified before write.

mod format;
mod items_db;
mod keys;
mod ops;
mod tui;

use anyhow::{bail, Context, Result};
use std::io::IsTerminal;
use format::SaveData;

/// Raised by builders when the requested state already holds.
#[derive(Debug, Clone)]
pub struct NoChange(pub String);
impl std::fmt::Display for NoChange {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for NoChange {}
use ops::{build_difficulty, build_give, build_money, build_remove, build_stack, Built};
use std::fs;
use std::path::Path;

const TARGETS: [&str; 3] = ["Inventory", "Equipment", "SecondaryEquipment"];

fn target_of(name: &str) -> Result<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "inventory" => Ok(TARGETS[0]),
        "equipment" => Ok(TARGETS[1]),
        "secondary" => Ok(TARGETS[2]),
        _ => bail!("target must be 'inventory', 'equipment' or 'secondary'"),
    }
}

pub fn read_save(path: &Path) -> Result<SaveData> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    format::parse_file(bytes).with_context(|| format!("parsing {}", path.display()))
}

/// Build a patch, apply, re-parse to verify, back up the original, write out.
/// Returns a status message (printed unless `quiet`, e.g. in the TUI).
pub fn run(
    path: &Path,
    outfile: Option<&Path>,
    build: impl FnOnce(&SaveData) -> Result<Built>,
    quiet: bool,
) -> Result<String> {
    let sf = read_save(path)?;
    let built = match build(&sf) {
        Ok(b) => b,
        Err(e) => {
            if let Some(nc) = e.downcast_ref::<NoChange>() {
                let m = format!("{}: {}", path.display(), nc.0);
                if !quiet {
                    println!("{m}");
                }
                return Ok(m);
            }
            return Err(e);
        }
    };
    let patched = format::apply_edits(sf.data.clone(), built.edits.clone());
    ops::verify(&patched, &built.verify).context("verification")?;
    fs::write(path.with_extension("bak"), &sf.data)
        .with_context(|| format!("writing backup for {}", path.display()))?;
    let out = outfile.unwrap_or(path);
    fs::write(out, &patched).with_context(|| format!("writing {}", out.display()))?;
    let msg = format!(
        "{}: {}. Backup: {}.bak. Wrote {} ({} bytes, was {}).",
        path.display(),
        built.msg,
        path.display(),
        out.display(),
        patched.len(),
        sf.data.len()
    );
    if !quiet {
        println!("{msg}");
    }
    Ok(msg)
}

// --- dumping / listing -------------------------------------------------------

/// Run a sequence of builder steps against the evolving save: each step is
/// re-parsed fresh, applied and verified. One .bak, one final write.
pub fn run_multi(
    path: &Path,
    outfile: Option<&Path>,
    build: impl FnOnce(&SaveData) -> Result<Vec<ops::Step>>,
    quiet: bool,
) -> Result<String> {
    let sf0 = read_save(path)?;
    let steps = match build(&sf0) {
        Ok(s) => s,
        Err(e) => {
            if let Some(nc) = e.downcast_ref::<NoChange>() {
                let m = format!("{}: {}", path.display(), nc.0);
                if !quiet {
                    println!("{m}");
                }
                return Ok(m);
            }
            return Err(e);
        }
    };
    let mut cur = sf0.data.clone();
    let mut msgs = Vec::new();
    for (n, step) in steps.into_iter().enumerate() {
        let sf = format::parse_file(cur.clone())
            .with_context(|| format!("re-parse after step {}", n + 1))?;
        let built = step(&sf)?;
        cur = format::apply_edits(cur, built.edits.clone());
        ops::verify(&cur, &built.verify).with_context(|| format!("verification of step {}", n + 1))?;
        msgs.push(built.msg);
    }
    fs::write(path.with_extension("bak"), &sf0.data)
        .with_context(|| format!("writing backup for {}", path.display()))?;
    let out = outfile.unwrap_or(path);
    fs::write(out, &cur).with_context(|| format!("writing {}", out.display()))?;
    let msg = format!(
        "{}: {} step(s)\n  {}.\nBackup: {}.bak. Wrote {} ({} bytes, was {}).",
        path.display(),
        msgs.len(),
        msgs.join("\n  "),
        path.display(),
        out.display(),
        cur.len(),
        sf0.data.len()
    );
    if !quiet {
        println!("{msg}");
    }
    Ok(msg)
}

/// Parse a give-manifest: lines of `<itemId> [count] [inventory|equipment|secondary]`.
pub fn manifest_steps(content: &str) -> Result<Vec<ops::Step>> {
    let mut steps: Vec<ops::Step> = Vec::new();
    for (n, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let id: String = parts
            .next()
            .with_context(|| format!("manifest line {}: missing itemId", n + 1))?
            .to_string();
        let count = match parts.next() {
            Some(s) => s
                .parse::<u32>()
                .with_context(|| format!("manifest line {}: bad count '{s}'", n + 1))?,
            None => 1,
        };
        let target = match parts.next() {
            Some(t) => target_of(t)?,
            None => TARGETS[0],
        };
        if parts.next().is_some() {
            bail!("manifest line {}: too many fields", n + 1);
        }
        if count == 0 {
            bail!("manifest line {}: count must be > 0", n + 1);
        }
        steps.push(Box::new(move |sf| {
            ops::build_give(sf, &ops::ItemSpec { target, id: id.to_string(), count, slot: None })
        }));
    }
    if steps.is_empty() {
        bail!("manifest is empty");
    }
    Ok(steps)
}

fn dump(path: &Path) -> Result<()> {
    let sf = read_save(path)?;
    println!(
        "{}: {} strings (map mode {:?}), payload at {}, root fields={}",
        path.display(),
        sf.strings.len(),
        sf.mode,
        sf.root_start,
        sf.root.entries.len()
    );
    for e in &sf.root.entries {
        println!("  root.{:<30} tc={:<3} len={}", sf.name(e.id), e.tc, e.plen);
    }
    let cm = sf
        .find(&sf.root, "CharacterModel")
        .context("CharacterModel field not found")?;
    let cmo = sf.sub(cm)?;
    println!("CharacterModel:");
    for e in &cmo.entries {
        let val = if sf.name(e.id) == "Difficulty" {
            let v = format::read_u32(&sf.data, e.pstart + 1);
            match v {
                0 => " -> 0 (Normal)".to_string(),
                1 => " -> 1 (Softcore)".to_string(),
                x => format!(" -> {} (?)", x),
            }
        } else {
            String::new()
        };
        println!("  char.{:<30} tc={:<3} len={}{}", sf.name(e.id), e.tc, e.plen, val);
    }
    match sf.find(&sf.root, "Money") {
        Some(m) => println!("  root.Money = {}", format::read_u64(&sf.data, m.pstart)),
        None => println!("  root.Money = 0 (field absent)"),
    }
    Ok(())
}

fn items(path: &Path, target: Option<&str>, json: bool) -> Result<()> {
    let sf = read_save(path)?;
    let t: &str = match target {
        Some(x) => target_of(x)?,
        None => TARGETS[0],
    };
    let e = sf
        .find(&sf.root, t)
        .with_context(|| format!("{} field not found in save", t))?;
    let arr = ops::parse_array_data(&sf, e)?;
    let used = arr.elems.iter().filter(|e| !e.is_null).count();
    if !json {
        println!("{}: {} — {} slots, {} used", path.display(), t, arr.count, used);
    }
    let colorize = std::io::stdout().is_terminal() && !json;
    if json {
        let rows: Vec<serde_json::Value> = arr
            .elems
            .iter()
            .enumerate()
            .map(|(i, el)| match ops::slot_info(&sf, el) {
                Some(info) => json_row(i + 1, &info),
                None => serde_json::json!({ "slot": i + 1, "empty": true }),
            })
            .collect();
        println!("{}", serde_json::to_string(&rows).context("json output")?);
        return Ok(());
    }
    for (i, el) in arr.elems.iter().enumerate() {
        match ops::slot_info(&sf, el) {
            Some(info) => println!("  {}", plain_row(i + 1, &info, colorize)),
            None => println!("  slot {:2}: (empty)", i + 1),
        }
    }
    Ok(())
}

fn json_row(slot: usize, info: &ops::SlotInfo) -> serde_json::Value {
    let def = items_db::get(&info.base_name);
    let dmg = def.and_then(items_db::dmg_ranges);
    serde_json::json!({
        "slot": slot,
        "id": info.base_name,
        "count": info.count,
        "usable": info.has_usable,
        "name": def.map(|d| d.name.clone()).unwrap_or_default(),
        "rarity": def.map(|d| d.rarity.clone()).unwrap_or_default(),
        "damage": dmg,
    })
}

fn plain_row(slot: usize, info: &ops::SlotInfo, colorize: bool) -> String {
    let def = items_db::get(&info.base_name);
    let extra = match def {
        Some(d) => {
            let dmg = items_db::dmg_ranges(d)
                .map(|r| format!(" dmg {}", r.join("/")))
                .unwrap_or_default();
            format!(" [{}]{} {}", color(&d.rarity, colorize), dmg, d.name)
        }
        None => String::new(),
    };
    format!(
        "slot {:2}: {} x{}{}{}",
        slot,
        info.base_name,
        info.count,
        if info.has_usable { " (usable)" } else { "" },
        extra
    )
}

/// ANSI color for a rarity when stdout is a terminal.
fn color(rarity: &str, colorize: bool) -> String {
    if !colorize {
        return rarity.to_string();
    }
    let code = match rarity {
        "trash" => 90,
        "common" => 37,
        "uncommon" => 32,
        "rare" => 94,
        "epic" => 35,
        "legendary" => 93,
        "exceptional" => 36,
        _ => 90,
    };
    format!("[{code}m{rarity}[0m")
}

// --- CLI ---------------------------------------------------------------------

const USAGE_LINES: [&str; 12] = [
    "saveedit dump       <file>",
    "saveedit difficulty <file> <softcore|normal> [outfile]",
    "saveedit money      <file> <amount>            [outfile]",
    "saveedit items      <file> [inventory|equipment|secondary]",
    "saveedit give       <file> <itemId> [count] [target] [slot] [outfile]",
    "saveedit remove     <file> <slot>   [target] [outfile]",
    "saveedit stack      <file> <slot> <count> [target] [outfile]",
    "saveedit merge      <file> [target] [outfile]",
    "saveedit mergeall   <file> [outfile]",
    "saveedit sortinv    <file> [outfile]",
    "saveedit batchgive <file> <manifest> [outfile]",
    "saveedit db         [filter]      (search the game item database)",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = dispatch(&args) {
        eprintln!("error: {:#}", e);
        std::process::exit(1);
    }
}

fn dispatch(args: &[String]) -> Result<()> {
    let usage = |e: &str| -> Result<()> {
        eprintln!("saveedit — edit Romestead .char saves");
        eprintln!("\nusage:");
        eprintln!("  saveedit tui        [folder]");
        for l in USAGE_LINES {
            eprintln!("  {l}");
        }
        if !e.is_empty() {
            eprintln!("\nunknown/missing: {e}");
        }
        Ok(())
    };
    match args.get(0).map(String::as_str) {
        None => usage(""),
        Some("tui") => tui::run_tui(Path::new(
            args.get(1).map(String::as_str).unwrap_or("."),
        )),
        Some("dump") => dump(Path::new(arg_file(args)?)),
        Some("items") => {
            let json = args.iter().any(|a| a == "--json");
            let target = args
                .get(2)
                .map(String::as_str)
                .filter(|t| *t != "--json");
            items(Path::new(arg_file(args)?), target, json)
        }
        Some("difficulty") => {
            let want = args
                .get(2)
                .context("missing difficulty (softcore|normal)")?;
            let softcore = match want.as_str() {
                "softcore" => true,
                "normal" => false,
                x => bail!("difficulty must be 'softcore' or 'normal', got '{x}'"),
            };
            run_at(args, 3, move |sf| build_difficulty(sf, softcore))
        }
        Some("enable") => run_at(args, 2, |sf| build_difficulty(sf, true)),
        Some("money") => {
            let amount: u64 = args
                .get(2)
                .context("missing amount")?
                .parse()
                .context("amount must be a non-negative integer")?;
            run_at(args, 3, move |sf| build_money(sf, amount))
        }
        Some("give") => {
            let id = args.get(2).context("missing itemId")?.clone();
            let count = match args.get(3) {
                Some(s) => s.parse::<u32>().context("count must be an integer")?,
                None => 1,
            };
            let target: Option<&'static str> = match args.get(4) {
                Some(t) => Some(target_of(t)?),
                None => None,
            };
            let slot = match args.get(5) {
                Some(s) => Some(s.parse::<usize>().context("slot must be a number")?),
                None => None,
            };
            run_at(args, 6, move |sf| {
                build_give(
                    sf,
                    &ops::ItemSpec {
                        target: target.unwrap_or(TARGETS[0]),
                        id: id.clone(),
                        count,
                        slot,
                    },
                )
            })
        }
        Some("remove") => {
            let slot = args
                .get(2)
                .context("missing slot")?
                .parse::<usize>()
                .context("slot must be a number")?;
            let target: Option<&'static str> = match args.get(3) {
                Some(t) => Some(target_of(t)?),
                None => None,
            };
            run_at(args, 4, move |sf| {
                build_remove(sf, target.unwrap_or(TARGETS[0]), slot)
            })
        }
        Some("stack") => {
            let slot = args
                .get(2)
                .context("missing slot")?
                .parse::<usize>()
                .context("slot must be a number")?;
            let count = args
                .get(3)
                .context("missing count")?
                .parse::<u32>()
                .context("count must be an integer")?;
            let target: Option<&'static str> = match args.get(4) {
                Some(t) => Some(target_of(t)?),
                None => None,
            };
            run_at(args, 5, move |sf| {
                build_stack(sf, target.unwrap_or(TARGETS[0]), slot, count)
            })
        }
        Some("db") => {
            // whitespace-separated tokens: every token must match id or name
            let toks: Vec<String> = args
                .get(1)
                .filter(|s| s.as_str() != "--json")
                .map(|s| {
                    s.to_lowercase()
                        .split_whitespace()
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            let items = items_db::all();
            let json = args.iter().any(|a| a == "--json");
            let hits: Vec<_> = items
                .iter()
                .filter(|d| {
                    toks.iter().all(|f| {
                        d.id.to_lowercase().contains(f)
                            || d.name.to_lowercase().contains(f)
                    })
                })
                .collect();
            if json {
                serde_json::to_writer(&mut std::io::stdout(), &hits).ok();
                println!();
                return Ok(());
            }
            let colorize = std::io::stdout().is_terminal();
            for d in &hits {
                let desc = items_db::describe(d);
                let desc = if desc.is_empty() { "-".to_string() } else { desc };
                let tier = d.tier.map(|t| t.to_string()).unwrap_or_else(|| "?".into());
                let icon = d.icon.as_deref().unwrap_or("-");
                println!(
                    "{:<34} {:<30} {:<12} {:<5} {:<22} {}",
                    d.id, d.name, color(&d.rarity, colorize), tier, icon, desc
                );
            }
            println!(
                "\n{} of {} items{}",
                hits.len(),
                items.len(),
                if toks.is_empty() { "" } else { " matched" }
            );
            Ok(())
        }
        Some("sortinv") => run_at(args, 2, ops::build_sort_inventory),
        Some("merge") => {
            let t: Vec<&'static str> = match args.get(2).map(String::as_str) {
                None => vec![TARGETS[0]],
                Some(s) => match target_of(s)? {
                    "all" => bail!("use a specific target, 'inventory' by default"),
                    t => vec![t],
                },
            };
            run_multi_at(args, 3, move |sf| ops::build_merge(sf, &t))
        }
        Some("mergeall") => {
            run_multi_at(args, 2, |sf| ops::build_merge(sf, &TARGETS))
        }
        Some("batchgive") | Some("give-file") => {
            let mf = args.get(2).context("missing manifest file path")?.clone();
            let content = fs::read_to_string(&mf)
                .with_context(|| format!("reading manifest {}", mf))?;
            let steps = manifest_steps(&content)?;
            let n = steps.len();
            let run: Result<Vec<ops::Step>> = Ok::<_, anyhow::Error>(steps);
            run_multi_at(args, 3, move |_| run)
                .with_context(|| format!("{} step(s) from manifest {}", n, mf))?;
            Ok(())
        }
        Some(other) => usage(other),
    }
}

fn arg_file(args: &[String]) -> Result<&str> {
    Ok(args.get(1).context("missing file path")?.as_str())
}

/// Build a patch, apply, verify, back up, write — with the outfile position
/// fixed per command (None = in place).
fn run_at(
    args: &[String],
    out_idx: usize,
    build: impl FnOnce(&SaveData) -> Result<Built>,
) -> Result<()> {
    let file = Path::new(arg_file(args)?);
    let out = args.get(out_idx).map(|s| Path::new(s).to_path_buf());
    run(file, out.as_deref(), build, false)?;
    Ok(())
}

fn run_multi_at(
    args: &[String],
    out_idx: usize,
    build: impl FnOnce(&SaveData) -> Result<Vec<ops::Step>>,
) -> Result<()> {
    let file = Path::new(arg_file(args)?);
    let out = args.get(out_idx).map(|s| Path::new(s).to_path_buf());
    run_multi(file, out.as_deref(), build, false)?;
    Ok(())
}

