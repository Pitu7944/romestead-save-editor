// Interactive ratatui frontend: save picker -> edit menu -> item browser with
// a searchable item-id picker. All edits go through the same verify+backup
// path as the CLI (crate::run with quiet=true).

use crate::format;
use crate::items_db;
use crate::ops::{self, Built, ItemSpec};
use crate::{run, TARGETS};
use anyhow::{bail, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier as TMod, Style},
        text::Line,
    widgets::{Block, List, ListItem, Paragraph},
    Frame, Terminal,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn run_tui(folder: &Path) -> Result<()> {
    let files = list_char_files(folder);
    if files.is_empty() {
        bail!("no .char files found in {}", folder.display());
    }
    let mut st = State {
        folder: folder.to_owned(),
        files,
        save_sel: 0,
        menu: None,
        menu_sel: 0,
        item: None,
        picker: None,
        money_editing: false,
        money_input: String::new(),
        msg: String::new(),
        msg_ok: true,
        undo: Vec::new(),
        hdr: None,
        body: None,
    };
    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableMouseCapture
    );
    let res = app_loop(&mut terminal, &mut st);
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture
    );
    ratatui::restore();
    res
}

struct State {
    folder: PathBuf,
    files: Vec<PathBuf>,
    save_sel: usize,
    menu: Option<PathBuf>, // when Some: edit menu screen for this save
    menu_sel: usize,
    item: Option<Items>,
    picker: Option<Picker>,
    money_editing: bool,
    money_input: String,
    msg: String,
    msg_ok: bool,
    undo: Vec<(PathBuf, Vec<u8>, Vec<u8>)>, // (file, bytes before, bytes after)
    hdr: Option<Header>,           // summary of the current save
    body: Option<Rect>,            // main list area (for mouse hit-testing)
}

#[derive(Clone)]
struct Header {
    name: String,
    diff: String,
    money: u64,
}

fn read_header(path: &Path) -> Option<Header> {
    let sf = crate::read_save(path).ok()?;
    let cmo = sf
        .find(&sf.root, "CharacterModel")
        .and_then(|cm| sf.sub(cm).ok());
    let name = cmo.as_ref().and_then(|cm| {
        sf.find(cm, "Name").and_then(|e| {
            format::read7(&sf.data, e.pstart)
                .ok()
                .map(|(sid, _)| sf.name(sid as u32).to_string())
        })
    });
    let diff = cmo.as_ref().and_then(|cm| {
        sf.find(cm, "Difficulty").map(|d| {
            match format::read_u32(&sf.data, d.pstart + 1) {
                0 => "Normal",
                1 => "Softcore",
                _ => "?",
            }
            .to_string()
        })
    });
    let money = sf
        .find(&sf.root, "Money")
        .map(|m| format::read_u64(&sf.data, m.pstart))
        .unwrap_or(0);
    Some(Header {
        name: name.unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string()
        }),
        diff: diff.unwrap_or_else(|| "Normal".into()),
        money,
    })
}

fn refresh_header(st: &mut State, path: &Path) {
    st.hdr = read_header(path);
}

struct Items {
    path: PathBuf,
    array: &'static str,
    lines: Vec<String>,
    colors: Vec<Option<Color>>, // rarity color per line (None = plain)
    infos: Vec<Option<ops::SlotInfo>>,
    count: usize,
    used: usize,
    sel: usize,
    top: usize,
    input: String, // count text input while Give/Stack active
    mode: ItemMode,
}

#[derive(PartialEq, Clone, Debug)]
enum ItemMode {
    Browse,
    Give(usize, String), // slot, item id
    Stack(usize, u32),
}

struct Picker {
    all: Vec<String>,
    matches: Vec<String>,
    filter: String,
    sel: usize, // index into matches
    top: usize,
    action: PickAction,
    sort_by_name: bool, // false = rarity (default), true = alphabetical
    category: Option<String>, // id prefix; None = all
    cats: Vec<String>,        // all categories present in the DB
    favs: HashSet<String>,
    recent: Vec<String>,
}

enum PickAction {
    Give(usize), // slot being filled
}

/// Recently-given + favorited item ids, persisted as `.saveedit_recents.json`
/// inside the save folder.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Recents {
    recent: Vec<String>,
    favs: Vec<String>,
}

fn recents_path(folder: &Path) -> PathBuf {
    folder.join(".saveedit_recents.json")
}

fn load_recents(folder: &Path) -> Recents {
    std::fs::read_to_string(recents_path(folder))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_recents(folder: &Path, r: &Recents) {
    if let Ok(s) = serde_json::to_string_pretty(r) {
        let _ = std::fs::write(recents_path(folder), s);
    }
}

/// Categories = first id segments present in the DB (weapon, food, ...).
fn db_categories() -> Vec<String> {
    let mut v: Vec<String> = items_db::all()
        .iter()
        .filter_map(|d| d.id.split(':').next().map(str::to_string))
        .filter(|c| !c.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}


// --- screens ------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Flow {
    None,
    Quit,
}






fn list_char_files(folder: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("char"))
        .collect();
    v.sort();
    v
}

fn save_summary(path: &Path) -> Result<(PathBuf, String, String, u64)> {
    let sf = format::parse_file(std::fs::read(path)?)?;
    let cm = sf
        .find(&sf.root, "CharacterModel")
        .context("no CharacterModel")?;
    let cmo = sf.sub(cm)?;
    let name = match sf.find(&cmo, "Name") {
        Some(n) if n.tc == format::TC_STRING => {
            let (sid, _) = format::read7(&sf.data, n.pstart)?;
            sf.name(sid as u32).to_string()
        }
        _ => path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string(),
    };
    let diff = match sf.find(&cmo, "Difficulty") {
        Some(d) => match format::read_u32(&sf.data, d.pstart + 1) {
            0 => "Normal",
            1 => "Softcore",
            _ => "?",
        }
        .to_string(),
        None => "Normal".to_string(),
    };
    let money = sf
        .find(&sf.root, "Money")
        .map(|m| format::read_u64(&sf.data, m.pstart))
        .unwrap_or(0);
    Ok((path.to_owned(), name, diff, money))
}

/// Apply an edit through the shared run() (quiet) and surface the result as a
/// footer status. run() already wrote .bak + verified before writing.
/// The pre-edit bytes are kept on the undo stack (ctrl+z restores them).
fn apply_action(st: &mut State, path: &Path, build: impl FnOnce(&format::SaveData) -> Result<Built>) {
    let before = std::fs::read(path).ok();
    match run(path, None, build, true) {
        Ok(msg) => {
            if let Some(before) = before {
                let after = std::fs::read(path).ok().unwrap_or_default();
                if after != before {
                    st.undo.push((path.to_owned(), before, after));
                    if st.undo.len() > 32 {
                        st.undo.remove(0);
                    }
                }
            }
            st.msg = msg;
            st.msg_ok = true;
            refresh_header(st, path);
        }
        Err(e) => {
            st.msg = format!("{e:#}");
            st.msg_ok = false;
        }
    }
}

/// Restore the most recent edit (ctrl+z). Refuses when the file changed since
/// our own edit (game wrote it in the meantime).
fn undo_last(st: &mut State) {
    match st.undo.pop() {
        None => {
            st.msg = "nothing to undo".into();
            st.msg_ok = false;
        }
        Some((path, before, after)) => {
            let current = std::fs::read(&path).ok().unwrap_or_default();
            if current != after {
                st.msg = format!(
                    "cannot undo {}: file changed since the edit", path.display()
                );
                st.msg_ok = false;
                return;
            }
            match std::fs::write(&path, &before) {
                Ok(_) => {
                    st.msg = format!("undid the last edit of {}", path.display());
                    st.msg_ok = true;
                    refresh_header(st, &path);
                    if let Some(it) = st.item.as_mut() {
                        if it.path == path {
                            let arr = it.array;
                            refresh_item(st, &path, arr);
                        }
                    }
                }
                Err(e) => {
                    st.msg = format!("undo failed: {e}");
                    st.msg_ok = false;
                }
            }
        }
    }
}

// --- event loop ---------------------------------------------------------------

fn app_loop(
    term: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    st: &mut State,
) -> Result<()> {
    loop {
        term.draw(|f| draw(f, st))?;
        match event::read()? {
            Event::Key(key) => if key.kind == KeyEventKind::Press {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('z')
                {
                    undo_last(st);
                    continue;
                }
                if on_key(key.code, st)? == Flow::Quit {
                    return Ok(());
                }
            }
            Event::Mouse(m) => {
                on_mouse(m, st)?;
            }
            _ => {}
        }
    }
}

/// Click to select (click again to activate), wheel scrolls.
fn on_mouse(m: crossterm::event::MouseEvent, st: &mut State) -> Result<Flow> {
    match m.kind {
        MouseEventKind::ScrollUp => {
            route_key(KeyCode::Up, st)?; // returns Flow
        }
        MouseEventKind::ScrollDown => {
            route_key(KeyCode::Down, st)?;
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let body = st.body.unwrap_or_default();
            let y = m.row.saturating_sub(body.y); // includes the 1-row list border
            if st.picker.is_some() {
                if let Some(p) = st.picker.as_mut() {
                    let idx = p.top + (y as usize).saturating_sub(1);
                    if idx < p.matches.len() {
                        if idx == p.sel {
                            return on_picker_key(KeyCode::Enter, st); // second click picks
                        }
                        p.sel = idx;
                        clamp_pick_window(p);
                    }
                }
            } else if st.item.is_some() {
                if let Some(it) = st.item.as_mut() {
                    let idx = it.top + (y as usize).saturating_sub(1);
                    if idx < it.count {
                        it.sel = idx;
                    }
                }
            } else if st.menu.is_some() {
                // edit menu: 5 rows, click selects, second click activates
                let row = (y as usize).saturating_sub(1);
                if row == st.menu_sel {
                    on_root_key(KeyCode::Enter, st)?;
                } else {
                    st.menu_sel = row.min(4);
                }
            } else {
                // save picker: click selects, click again opens the menu
                let row = (y as usize).saturating_sub(1);
                if row < st.files.len() {
                    if row == st.save_sel {
                        on_root_key(KeyCode::Enter, st)?;
                    } else {
                        st.save_sel = row;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(Flow::None)
}

fn route_key(code: KeyCode, st: &mut State) -> Result<Flow> {
    if st.picker.is_some() {
        return on_picker_key(code, st);
    }
    if st.item.is_some() {
        return on_items_key(code, st);
    }
    on_root_key(code, st)
}

// --- drawing -------------------------------------------------------------------

fn layout(f: &mut Frame) -> [Rect; 4] {
    let v: Vec<Rect> = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // title
            Constraint::Length(1),  // character summary
            Constraint::Min(1),     // list body
            Constraint::Length(1),  // footer / status
        ])
        .split(f.area())
        .into_iter()
        .copied()
        .collect();
    let r: [Rect; 4] = v.try_into().unwrap();
    r
}

/// Split the list body to make room for an item detail pane (right on wide
/// terminals, bottom on narrow ones).
fn split_detail(body: Rect) -> [Rect; 2] {
    let v: Vec<Rect> = if body.width >= 90 {
        Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(body)
            .into_iter()
            .copied()
            .collect()
    } else {
        Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(body)
            .into_iter()
            .copied()
            .collect()
    };
    v.try_into().unwrap()
}

/// Summary line of the open save (menu / items screen) or folder stats.
fn summary_line(st: &State) -> String {
    match &st.hdr {
        Some(h) => format!("{} · {} · {} g", h.name, h.diff, h.money),
        _ => format!("{} saves · game DB: {} items", st.files.len(), items_db::all().len()),
    }
}

/// Full stats panel for an item def (rendered in the detail pane).
fn detail_lines(def: &items_db::ItemDef) -> Vec<Line<'static>> {
    use ratatui::text::Span;
    use serde_json::Value as V;
    let mut out: Vec<Line<'static>> = Vec::new();
    let name = def.name.clone();
    out.push(Line::from(vec![
        Span::styled(name, Style::new().fg(rarity_color(&def.rarity)).add_modifier(TMod::BOLD)),
    ]));
    let mut kv = |k: &str, v: String| {
        out.push(Line::from(vec![
            Span::styled(format!("{k:<10}"), Style::new().fg(Color::DarkGray)),
            Span::raw(v),
        ]));
    };
    kv("id", def.id.clone());
    kv("rarity", format!(
        "{} (tier {})",
        def.rarity,
        def.tier.map(|t| t.to_string()).unwrap_or_else(|| "-".into())
    ));
    if let Some(s) = def.stack {
        kv("stack", s.to_string());
    }
    if let Some(icon) = &def.icon {
        kv("icon", icon.clone());
    }
    if let Some(V::Object(e)) = &def.equip {
        if let Some(V::String(slot)) = e.get("slot") {
            kv("slot", slot.clone());
        }
        if let Some(V::Object(w)) = e.get("weapon") {
            if let Some(V::String(cls)) = w.get("class") {
                kv("weapon", cls.clone());
            }
            if let Some(dmg) = items_db::dmg_ranges(def) {
                kv("damage", dmg.join(", "));
            }
            for (k, label) in [("swing", "swing"), ("knockback", "knockback"),
                               ("energy", "energy"), ("stun", "stun")] {
                if let Some(v) = w.get(k).and_then(|x| x.as_f64()) {
                    kv(label, items_db::fmt_num(v));
                }
            }
        }
        if let Some(V::Object(b)) = e.get("bonuses") {
            let names: Vec<&String> = b.keys().collect();
            if !names.is_empty() {
                for n in names {
                    let val = &b[n];
                    let parts: Vec<String> = ["Additive", "Multiplier",
                        "AdditiveMultiplier", "BaseMultiplier", "BonusMultiplier"]
                        .iter()
                        .filter_map(|k| {
                            val.get(*k)
                                .and_then(|x| x.as_f64())
                                .map(|x| items_db::fmt_num(x))
                        })
                        .collect();
                    kv("bonus", format!("{n}: {parts:?}"));
                }
            }
        }
    }
    if let Some(price) = def.price {
        kv("price", format!("{price} gold"));
    }
    match &def.usable {
        Some(V::Object(o)) if o.get("__ctor").is_some() => {
            let args: Vec<String> = o
                .get("args")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().map(|x| items_db::fmt_num(x.as_f64().unwrap_or(0.0))).collect())
                .unwrap_or_default();
            let ctor = o.get("__ctor").and_then(|x| x.as_str()).unwrap_or("?");
            kv("usable", format!("{ctor}({})", args.join(", ")));
        }
        Some(V::Object(o)) => {
            if let Some(sp) = o.get("SpellId").and_then(|x| x.as_str()) {
                kv("usable", format!("spell {sp}"));
            }
        }
        _ => {}
    }
    if let Some(ds) = &def.description {
        kv("", ds.clone());
    }
    out
}

fn draw(f: &mut Frame, st: &mut State) {
    if st.picker.is_some() {
        draw_picker(f, st);
        return;
    }
    let (title, lines, colors, sel, top, _footer, detail_open) = match (&st.item, &st.picker) {
        (_, Some(_)) => unreachable!("item picker is rendered by draw_picker above"),
        (Some(it), None) => (
            format!(
                "{} — {}   {}/{} used{}",
                it.path.display(),
                it.array,
                it.used,
                it.count,
                match &it.mode {
                    ItemMode::Browse => String::new(),
                    ItemMode::Give(slot, id) =>
                        format!("   giving {} at slot {} count: {}_", id, slot, it.input),
                    ItemMode::Stack(slot, _) =>
                        format!("   stack slot {} count: {}_", slot, it.input),
                }
            ),
            it.lines.clone(),
            Option::Some(it.colors.clone()),
            it.sel,
            it.top,
            footer_for_items(it),
            true,
        ),
        (None, None) => {
            let (title, lines, sel): (String, Vec<String>, usize) = if let Some(f) = &st.menu {
                (menu_title(st), menu_lines(f), st.menu_sel)
            } else {
                (saves_title(&st.folder), saves_lines(st), st.save_sel)
            };
            (title, lines, Option::None, sel, 0usize,
             "enter select | esc back/quit".to_string(), false)
        }
    };
    let [head, sum, body, foot] = layout(f);
    f.render_widget(Paragraph::new(title.as_str()).style(Style::new().fg(Color::Cyan).add_modifier(TMod::BOLD)), head);
    f.render_widget(
        {
            let line = summary_line(st);
            Paragraph::new(line).style(Style::new().fg(Color::DarkGray))
        },
        sum,
    );
    st.body = Some(body);
    let mut items = Vec::new();
    for (i, l) in lines.iter().enumerate().skip(top) {
        let mut style = Style::default();
        if let Some(c) = colors.as_ref().and_then(|v| v.get(i)).copied().flatten() {
            style = style.fg(c);
        }
        if i == sel {
            style = style.bg(Color::Cyan).fg(Color::Black).add_modifier(TMod::BOLD);
        }
        items.push(ListItem::new(Line::styled(format!(" {l}"), style)));
    }
    match detail_open {
        true => {
            // item browser: split list + detail
            let [list_rect, det_rect] = split_detail(body);
            f.render_widget(List::new(items).block(Block::bordered().title(title.as_str())), list_rect);
            let det: Vec<Line<'static>> = st
                .item
                .as_ref()
                .and_then(|it| it.infos.get(it.sel))
                .and_then(|o| o.as_ref())
                .and_then(|info| items_db::get(&info.base_name))
                .map(detail_lines)
                .unwrap_or_default();
            f.render_widget(
                Paragraph::new(det).block(Block::bordered().title("Details")),
                det_rect,
            );
        }
        false => {
            f.render_widget(List::new(items).block(Block::bordered().title(title.as_str())), body);
        }
    }
    let msg_area = match (&st.msg_ok, &st.msg) {
        (true, m) if !m.is_empty() => Paragraph::new(m.clone()).style(Style::new().fg(Color::Green)),
        (false, m) => Paragraph::new(m.clone()).style(Style::new().fg(Color::Red)),
        _ => {
            let (_, _, _, _, footer, _) = match (&st.item, &st.picker) {
                (_, Some(_)) => (0, 0, 0, 0, "type filter | enter pick | esc cancel".to_string(), ()),
                (Some(it), None) => (0, 0, 0, 0, footer_for_items(it), ()),
                (None, None) => (0, 0, 0, 0, "enter select | esc back/quit".to_string(), ()),
            };
            Paragraph::new(footer).style(Style::new().fg(Color::DarkGray))
        }
    };
    f.render_widget(msg_area, foot);
}

/// Pick-item screen with rarity colors, game names, favorites/recents,
/// category + sort toggles, a detail pane and stats footer.
fn draw_picker(f: &mut Frame, st: &mut State) {
    let p = st.picker.as_ref().unwrap();
    let [head, sum, body, foot] = layout(f);
    let title = format!(
        "pick  cat:{}  sort:{}  filter: {}_",
        p.category.clone().unwrap_or_else(|| "all".into()),
        if p.sort_by_name { "name" } else { "rarity" },
        p.filter
    );
    f.render_widget(
        Paragraph::new(title.as_str())
            .style(Style::new().fg(Color::Cyan).add_modifier(TMod::BOLD)),
        head,
    );
    f.render_widget(
        Paragraph::new(summary_line(st)).style(Style::new().fg(Color::DarkGray)),
        sum,
    );
    let [list_rect, det_rect] = split_detail(body);
    st.body = Some(list_rect);
    let mut items = Vec::new();
    for (i, id) in p.matches.iter().enumerate().skip(p.top) {
        let def = items_db::get(id);
        let pin = if p.favs.contains(id) {
            "★ "
        } else if p.recent.iter().any(|r| r == id) {
            "· "
        } else {
            "  "
        };
        let text = match def {
            Some(d) => {
                let dmg = items_db::dmg_ranges(d)
                    .map(|r| format!(" · dmg {}", r.join("/")))
                    .unwrap_or_default();
                format!("{pin}{}  [{}]  {}{}", d.name, d.rarity, d.id, dmg)
            }
            None => format!("{pin}{id}"),
        };
        let mut style = def
            .map(|d| Style::new().fg(rarity_color(&d.rarity)))
            .unwrap_or_default();
        if i == p.sel {
            style = style.bg(Color::Cyan).fg(Color::Black).add_modifier(TMod::BOLD);
        }
        items.push(ListItem::new(text).style(style));
    }
    f.render_widget(
        List::new(items).block(Block::bordered().title("Pick item")),
        list_rect,
    );
    let det: Vec<Line<'static>> = p
        .matches
        .get(p.sel)
        .and_then(|id| items_db::get(id))
        .map(detail_lines)
        .unwrap_or_default();
    f.render_widget(
        Paragraph::new(det).block(Block::bordered().title("Details")),
        det_rect,
    );
    let stats = p
        .matches
        .get(p.sel)
        .and_then(|id| items_db::get(id))
        .map(|d| {
            let s = items_db::describe(d);
            let mut s = if s.is_empty() {
                d.rarity.clone()
            } else {
                format!("[{}] {s}", d.rarity)
            };
            if let Some(ds) = &d.description {
                s.push_str(&format!(" · \"{ds}\""));
            }
            s
        })
        .unwrap_or_default();
    let footer = if stats.is_empty() {
        "tab: sort | F2: favorite | F3: category | enter pick | esc cancel".to_string()
    } else {
        format!("tab: sort | F2: fav | F3: cat | enter pick | esc cancel | {stats}")
    };
    f.render_widget(
        Paragraph::new(footer).style(Style::new().fg(Color::Yellow)),
        foot,
    );
}

fn footer_for_items(it: &Items) -> String {
    match &it.mode {
        ItemMode::Browse =>
            "arrows move | g give at slot | r remove | c set stack | esc back".into(),
        ItemMode::Give(sl, id) =>
            format!("giving {id} at slot {sl}: type count (enter apply, esc cancel)"),
        ItemMode::Stack(sl, _) =>
            format!("stack slot {sl}: type count (enter apply, esc cancel)"),
    }
}

// --- key handling ---------------------------------------------------------------

fn on_key(code: KeyCode, st: &mut State) -> Result<Flow> {
    if st.picker.is_some() {
        return on_picker_key(code, st);
    }
    if st.item.is_some() {
        return on_items_key(code, st);
    }
    on_root_key(code, st)
}

fn on_root_key(code: KeyCode, st: &mut State) -> Result<Flow> {
    if st.money_editing {
        match code {
            KeyCode::Esc => st.money_editing = false,
            KeyCode::Backspace => {
                st.money_input.pop();
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if st.money_input.len() < 15 {
                    st.money_input.push(c);
                }
            }
            KeyCode::Enter => {
                st.money_editing = false;
                if let Ok(amount) = st.money_input.parse::<u64>() {
                    let file = st.menu.clone().unwrap();
                    apply_action(st, &file, move |sf| ops::build_money(sf, amount));
                } else {
                    st.msg = "not a number".into();
                    st.msg_ok = false;
                }
            }
            _ => {}
        }
        return Ok(Flow::None);
    }
    match code {
        KeyCode::Char('q') => return Ok(Flow::Quit),
        KeyCode::Up => {
            if st.menu.is_none() {
                st.save_sel = st.save_sel.saturating_sub(1);
            } else {
                st.menu_sel = st.menu_sel.saturating_sub(1);
            }
        }
        KeyCode::Down => {
            if st.menu.is_none() {
                if st.save_sel + 1 < st.files.len() {
                    st.save_sel += 1;
                }
            } else if st.menu_sel + 1 < 5 {
                st.menu_sel += 1;
            }
        }
                    KeyCode::Enter => {
                if let Some(file) = st.menu.clone() {
                    menu_action(&file, st)?;
                } else {
                    let f = st.files.get(st.save_sel).cloned().unwrap_or_default();
                    refresh_header(st, &f);
                    st.menu = Some(f);
                }
            }
        KeyCode::Esc => {
            if st.menu.take().is_none() {
                return Ok(Flow::Quit);
            }
        }
        _ => {}
    }
    Ok(Flow::None)
}

fn menu_action(file: &PathBuf, st: &mut State) -> Result<()> {
    match st.menu_sel {
        0 => {
            // toggle difficulty: current state from fresh summary
            let sf = crate::read_save(file)?;
            let cmo = sf
                .sub(sf.find(&sf.root, "CharacterModel").context("no CharacterModel")?)?;
            let want_soft = sf
                .find(&cmo, "Difficulty")
                .map(|d| format::read_u32(&sf.data, d.pstart + 1) == 0)
                .unwrap_or(true);
            apply_action(st, file, move |sf| ops::build_difficulty(sf, want_soft));
        }
        1 => {
            st.money_editing = true;
            st.money_input = money_of(file).to_string();
        }
        2..=4 => {
            let array = TARGETS[st.menu_sel - 2];
            st.item = Some(open_items(file, array)?);
        }
        _ => {}
    }
    Ok(())
}

// money screen input happens here too (simple in-menu text mode)
fn money_of(file: &PathBuf) -> u64 {
    let Ok(sf) = crate::read_save(file) else { return 0 };
    sf.find(&sf.root, "Money")
        .map(|m| format::read_u64(&sf.data, m.pstart))
        .unwrap_or(0)
}

fn open_items(path: &Path, array: &'static str) -> Result<Items> {
    let sf = crate::read_save(path)?;
    let e = sf
        .find(&sf.root, array)
        .with_context(|| format!("{} field not found", array))?;
    let arr = ops::parse_array_data(&sf, e)?;
    let infos: Vec<Option<ops::SlotInfo>> =
        arr.elems.iter().map(|el| ops::slot_info(&sf, el)).collect();
    let used = infos.iter().filter(|o| o.is_some()).count();
    let lines: Vec<String> = infos
        .iter()
        .enumerate()
        .map(|(i, o)| match o {
            Some(info) => {
                let def = items_db::get(&info.base_name);
                let dmg = def
                    .and_then(items_db::dmg_ranges)
                    .map(|r| format!("  dmg {}", r.join("/")))
                    .unwrap_or_default();
                format!(
                    "slot {:2} | {:<34} x{:<4}{}{}",
                    i + 1,
                    info.base_name,
                    info.count,
                    match def {
                        Some(d) => format!("  [{}]{} {}", d.rarity, dmg, d.name),
                        None => String::new(),
                    },
                    if info.has_usable { "  [usable]" } else { "" }
                )
            }
            None => format!("slot {:2} | (empty)", i + 1),
        })
        .collect();
    let colors: Vec<Option<Color>> = infos
        .iter()
        .map(|o| {
            o.as_ref()
                .and_then(|info| items_db::get(&info.base_name))
                .map(|d| rarity_color(&d.rarity))
            // None for empty slots / unknown ids
        })
        .collect();
    Ok(Items {
        path: path.to_owned(),
        array,
        lines,
        colors,
        infos,
        count: arr.count,
        used,
        sel: 0,
        top: 0,
        input: String::new(),
        mode: ItemMode::Browse,
    })
}

fn on_items_key(code: KeyCode, st: &mut State) -> Result<Flow> {
    // snapshot everything needed, then act on owned data so st stays free
    struct Snap {
        path: PathBuf,
        array: &'static str,
        mode: ItemMode,
        sel_slot: usize,
        input: String,
        count: usize,
        occupied: bool,
        cur_count: u32,
    }
    let snap = {
        let it = st.item.as_ref().unwrap();
        Snap {
            path: it.path.clone(),
            array: it.array,
            mode: it.mode.clone(),
            sel_slot: it.sel + 1,
            input: it.input.clone(),
            count: it.count,
            occupied: it.infos.get(it.sel).is_some_and(|o| o.is_some()),
            cur_count: it
                .infos
                .get(it.sel)
                .and_then(|o| o.as_ref().map(|i| i.count))
                .unwrap_or(1),
        }
    };

    let _ = &snap;
    // handled below with direct match for clarity
    match snap.mode.clone() {
        ItemMode::Browse => match code {
            KeyCode::Esc => st.item = None,
            KeyCode::Up => {
                if let Some(it) = st.item.as_mut() {
                    it.sel = it.sel.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(it) = st.item.as_mut() {
                    it.sel = (it.sel + 1).min(snap.count.saturating_sub(1));
                }
            }
            KeyCode::Char('g') => {
                if !snap.occupied {
                    let rec = load_recents(&st.folder);
                    st.picker = Some(Picker {
                        all: picker_ids(&st.folder),
                        matches: Vec::new(),
                        filter: String::new(),
                        sel: 0,
                        top: 0,
                        action: PickAction::Give(snap.sel_slot),
                        sort_by_name: false,
                        category: None,
                        cats: db_categories(),
                        favs: rec.favs.into_iter().collect(),
                        recent: rec.recent,
                    });
                    if let Some(p) = st.picker.as_mut() {
                        refilter(p);
                    }
                } else {
                    st.msg = "slot is occupied — remove it first, or move onto an empty slot to give"
                        .into();
                    st.msg_ok = false;
                }
            }
            KeyCode::Char('r') => {
                if snap.occupied {
                    let slot = snap.sel_slot;
                    let arr = snap.array;
                    apply_action(st, &snap.path, move |sf| ops::build_remove(sf, arr, slot));
                } else {
                    st.msg = "slot is empty".into();
                    st.msg_ok = false;
                }
                refresh_item(st, &snap.path, snap.array);
            }
            KeyCode::Char('c') => {
                if snap.occupied {
                    if let Some(it) = st.item.as_mut() {
                        it.mode = ItemMode::Stack(snap.sel_slot, snap.cur_count);
                        it.input = snap.cur_count.to_string();
                    }
                } else {
                    st.msg = "slot is empty".into();
                    st.msg_ok = false;
                }
            }
            _ => {}
        },
        ItemMode::Give(s, id) => match code {
            KeyCode::Esc => {
                if let Some(it) = st.item.as_mut() {
                    it.mode = ItemMode::Browse;
                    it.input.clear();
                }
            }
            KeyCode::Backspace => {
                if let Some(it) = st.item.as_mut() {
                    it.input.pop();
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if let Some(it) = st.item.as_mut() {
                    if it.input.len() < 12 {
                        it.input.push(c);
                    }
                }
            }
            KeyCode::Enter => {
                let n: u32 = snap.input.parse().unwrap_or(0);
                if n == 0 {
                    st.msg = "count must be > 0".into();
                    st.msg_ok = false;
                    return Ok(Flow::None);
                }
                let id2 = id.clone();
                apply_action(st, &snap.path, move |sf| {
                    ops::build_give(
                        sf,
                        &ItemSpec {
                            target: snap.array,
                            id: id2,
                            count: n,
                            slot: Some(s),
                        },
                    )
                });
                refresh_item(st, &snap.path, snap.array);
                if let Some(it) = st.item.as_mut() {
                    it.mode = ItemMode::Browse;
                    it.input.clear();
                }
            }
            _ => {}
        },
        ItemMode::Stack(s, _) => match code {
            KeyCode::Esc => {
                if let Some(it) = st.item.as_mut() {
                    it.mode = ItemMode::Browse;
                    it.input.clear();
                }
            }
            KeyCode::Backspace => {
                if let Some(it) = st.item.as_mut() {
                    it.input.pop();
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if let Some(it) = st.item.as_mut() {
                    if it.input.len() < 12 {
                        it.input.push(c);
                    }
                }
            }
            KeyCode::Enter => {
                let n: u32 = snap.input.parse().unwrap_or(0);
                if n == 0 {
                    st.msg = "count must be > 0".into();
                    st.msg_ok = false;
                    return Ok(Flow::None);
                }
                apply_action(st, &snap.path, move |sf| {
                    ops::build_stack(sf, snap.array, s, n)
                });
                refresh_item(st, &snap.path, snap.array);
                if let Some(it) = st.item.as_mut() {
                    it.mode = ItemMode::Browse;
                    it.input.clear();
                }
            }
            _ => {}
        },
    }
    Ok(Flow::None)
}

// --- screen data builders ------------------------------------------------------

fn saves_title(folder: &Path) -> String {
    format!("Romestead Save Editor — {}", folder.display())
}

fn saves_lines(st: &State) -> Vec<String> {
    st.files
        .iter()
        .filter_map(|p| save_summary(p).ok())
        .map(|(path, name, diff, money)| {
            format!(
                "{:<12} {:<9} money {:<8}   {}",
                name,
                diff,
                money,
                path.file_name().and_then(|x| x.to_str()).unwrap_or("?")
            )
        })
        .collect()
}

fn menu_title(st: &State) -> String {
    match &st.menu {
        Some(f) => format!(
            "{}  —  {}",
            f.file_name().and_then(|x| x.to_str()).unwrap_or("?"),
            save_summary(f)
                .map(|(_, n, d, m)| format!("{} {}   money {}", n, d, m))
                .unwrap_or_else(|_| "(unreadable)".to_string())
        ),
        None => String::new(),
    }
}

fn menu_lines(file: &PathBuf) -> Vec<String> {
    let sf = match crate::read_save(file) {
        Ok(sf) => sf,
        Err(_) => return vec!["(unreadable save)".into()],
    };
    let cmo = match sf.find(&sf.root, "CharacterModel") {
        Some(cm) => sf.sub(cm).unwrap_or_else(|_| sf.root.clone()),
        None => sf.root.clone(),
    };
    let diff = sf.find(&cmo, "Difficulty").map(|d| format::read_u32(&sf.data, d.pstart + 1));
    let diff_s = match diff {
        None => "Normal",
        Some(0) => "Normal",
        Some(1) => "Softcore",
        _ => "?",
    };
    let money = sf
        .find(&sf.root, "Money")
        .map(|m| format::read_u64(&sf.data, m.pstart))
        .unwrap_or(0);
    vec![
        format!(
            "difficulty: {:<8} (enter toggles to {})",
            diff_s,
            if diff_s == "Softcore" { "Normal" } else { "Softcore" }
        ),
        format!("money: {:<9}   (enter to set a new amount)", money),
        "inventory   (enter to browse)".to_string(),
        "equipment   (enter to browse)".to_string(),
        "secondary equipment".to_string(),
    ]
}

/// All item ids seen in any .char of the folder; the picker offers these and
/// also accepts any typed id.
fn known_ids(folder: &Path) -> Vec<String> {
    let mut ids = std::collections::HashSet::new();
    for f in list_char_files(folder) {
        if let Ok(sf) = crate::read_save(&f) {
            for t in crate::TARGETS {
                if let Some(e) = sf.find(&sf.root, t) {
                    if let Ok(arr) = ops::parse_array_data(&sf, e) {
                        for el in arr.elems {
                            if let Some(info) = ops::slot_info(&sf, &el) {
                                ids.insert(info.base_name);
                            }
                        }
                    }
                }
            }
        }
    }
    let mut v: Vec<String> = ids.into_iter().collect();
    v.sort();
    v
}

/// Everything the item picker offers: the full embedded game database plus
/// any ids seen in the local saves that the DB doesn't know. Sorted by
/// rarity (legendary first), unknown-tier and save-only ids at the end.
fn picker_ids(folder: &Path) -> Vec<String> {
    let mut ids: Vec<String> = items_db::all().iter().map(|d| d.id.clone()).collect();
    ids.extend(
        known_ids(folder)
            .into_iter()
            .filter(|id| items_db::get(id).is_none()),
    );
    ids.dedup();
    ids.sort_by(|a, b| {
        let da = items_db::get(a);
        let db = items_db::get(b);
        // unknown-tier and save-only ids sort last, best rarity first
        let ka = (da.is_none(), std::cmp::Reverse(da.and_then(|d| d.tier)), a);
        let kb = (db.is_none(), std::cmp::Reverse(db.and_then(|d| d.tier)), b);
        ka.cmp(&kb)
    });
    ids
}

fn rarity_color(r: &str) -> Color {
    match r {
        "trash" => Color::DarkGray,
        "common" => Color::White,
        "uncommon" => Color::Green,
        "rare" => Color::Blue,
        "epic" => Color::Magenta,
        "legendary" => Color::Yellow,
        "exceptional" => Color::Cyan,
        _ => Color::Gray,
    }
}

fn refresh_item(st: &mut State, path: &Path, array: &'static str) {
    if let Ok(nu) = open_items(path, array) {
        if let Some(cur) = st.item.as_mut() {
            let sel = cur.sel.min(nu.count.saturating_sub(1));
            *cur = nu;
            cur.sel = sel;
        }
    }
}

fn on_picker_key(code: KeyCode, st: &mut State) -> Result<Flow> {
    if code == KeyCode::Enter {
        let (id, slot) = {
            let p = st.picker.as_ref().unwrap();
            let id = p
                .matches
                .get(p.sel)
                .cloned()
                .context("no item matches")?;
            let slot = match &p.action {
                PickAction::Give(s) => *s,
            };
            (id, slot)
        };
        // remember as recently given (dedup + cap, persisted)
        let folder = st.folder.clone();
        if let Some(p) = st.picker.as_mut() {
            p.recent.retain(|r| r != &id);
            p.recent.insert(0, id.clone());
            p.recent.truncate(20);
            save_recents(&folder, &Recents {
                recent: p.recent.clone(),
                favs: p.favs.iter().cloned().collect(),
            });
        }
        st.picker = None;
        if let Some(it) = st.item.as_mut() {
            it.mode = ItemMode::Give(slot, id);
            it.input = "1".to_string();
        }
        return Ok(Flow::None);
    }
    match code {
        KeyCode::Tab => {
            let p = st.picker.as_mut().unwrap();
            p.sort_by_name = !p.sort_by_name;
            refilter(p);
        }
        KeyCode::F(2) => {
            let folder = st.folder.clone();
            let p = st.picker.as_mut().unwrap();
            if let Some(id) = p.matches.get(p.sel).cloned() {
                if p.favs.contains(&id) {
                    p.favs.remove(&id);
                } else {
                    p.favs.insert(id.clone());
                    p.recent.retain(|r| r != &id);
                }
                save_recents(&folder, &Recents {
                    recent: p.recent.clone(),
                    favs: p.favs.iter().cloned().collect(),
                });
                refilter(p);
            }
        }
        KeyCode::F(3) => {
            let p = st.picker.as_mut().unwrap();
            p.category = match &p.category {
                None => p.cats.first().cloned(),
                Some(c) => {
                    let i = p.cats.iter().position(|x| x == c).unwrap_or(0);
                    if i + 1 < p.cats.len() {
                        Some(p.cats[i + 1].clone())
                    } else {
                        None // wrap back to "all"
                    }
                }
            };
            refilter(p);
        }
        KeyCode::Char(c) => {
            let p = st.picker.as_mut().unwrap();
            p.filter.push(c);
            refilter(p);
        }
        KeyCode::Backspace => {
            let p = st.picker.as_mut().unwrap();
            p.filter.pop();
            refilter(p);
        }
        KeyCode::Up => {
            let p = st.picker.as_mut().unwrap();
            p.sel = p.sel.saturating_sub(1);
            clamp_pick_window(p);
        }
        KeyCode::Down => {
            let p = st.picker.as_mut().unwrap();
            if p.matches.len() > 0 && p.sel + 1 < p.matches.len() {
                p.sel += 1;
                clamp_pick_window(p);
            }
        }
        KeyCode::Esc => st.picker = None,
        _ => {}
    }
    Ok(Flow::None)
}

fn refilter(p: &mut Picker) {
    let f = p.filter.to_lowercase();
    p.matches = p
        .all
        .iter()
        .filter(|id| {
            if let Some(c) = &p.category {
                if !id.starts_with(&format!("{c}:")) {
                    return false;
                }
            }
            id.to_lowercase().contains(&f)
                || items_db::get(id)
                    .map(|d| d.name.to_lowercase().contains(&f))
                    .unwrap_or(false)
        })
        .cloned()
        .collect();
    // pinned layers: favorites first, then recents, then everything else;
    // within a layer sorted by name or rarity per the current mode
    let prio = |id: &str| -> u8 {
        if p.favs.contains(id) {
            0
        } else if p.recent.iter().any(|r| r == id) {
            1
        } else {
            2
        }
    };
    p.matches.sort_by(|a, b| {
        prio(a)
            .cmp(&prio(b))
            .then_with(|| {
                if p.sort_by_name {
                    let na = items_db::get(a).map(|d| d.name.to_lowercase());
                    let nb = items_db::get(b).map(|d| d.name.to_lowercase());
                    na.cmp(&nb).then_with(|| a.cmp(b))
                } else {
                    crate::ops::rarity_rank(a).cmp(&crate::ops::rarity_rank(b)).then_with(|| a.cmp(b))
                }
            })
    });
    p.sel = 0;
    p.top = 0;
}

fn clamp_pick_window(p: &mut Picker) {
    const H: usize = 18;
    if p.sel < p.top {
        p.top = p.sel;
    }
    if p.sel >= p.top + H {
        p.top = p.sel - H + 1;
    }
}

#[cfg(test)]
mod tui_tests {
    use super::*;
    use crate::ops::ItemSpec;

    fn state_folder() -> PathBuf {
        PathBuf::from("..")
    }

    #[test]
    fn g_on_empty_slot_opens_picker() {
        let path = PathBuf::from("../Adolfina.char");
        let items = open_items(&path, "Inventory").unwrap();
        let free = items.infos.iter().position(|o| o.is_none()).unwrap();
        let mut st = test_state(&path);
        st.item = Some(items);
        st.save_sel = 0;
        // move selection to a free slot
        if let Some(it) = st.item.as_mut() {
            it.sel = free;
        }
        let res = on_items_key(KeyCode::Char('g'), &mut st).unwrap();
        assert_eq!(res, Flow::None);
        let p = st.picker.as_ref().expect("g should open the picker");
        assert!(!p.matches.is_empty(), "picker matches empty!");
        println!("picker open: {} ids, filter='{}'", p.matches.len(), p.filter);
    }

    #[test]
    fn g_on_occupied_slot_gives_feedback() {
        let path = PathBuf::from("../Adolfina.char");
        let mut st = test_state(&path);
        st.item = Some(open_items(&path, "Inventory").unwrap());
        // slot 1 occupied (selection starts at 0 = slot 1)
        on_items_key(KeyCode::Char('g'), &mut st).unwrap();
        assert!(st.picker.is_none(), "picker must NOT open on occupied slot");
        assert!(!st.msg_ok, "must show an error message");
        assert!(!st.msg.is_empty());
    }

    #[test]
    fn full_give_flow_via_picker() {
        let path = PathBuf::from("../Adolfina.char");
        let mut st = test_state(&path);
        // copy to temp so we don't touch the real save
        let tmp = std::env::temp_dir().join("se_give_flow.char");
        std::fs::copy(&path, &tmp).unwrap();
        st.item = Some(open_items(&tmp, "Inventory").unwrap());
        let free = st.item.as_ref().unwrap().infos.iter().position(|o| o.is_none()).unwrap();
        st.item.as_mut().unwrap().sel = free;
        on_items_key(KeyCode::Char('g'), &mut st).unwrap(); // opens picker
        st.picker.as_mut().unwrap().filter = "water:sword".into();
        refilter(st.picker.as_mut().unwrap());
        on_picker_key(KeyCode::Enter, &mut st).unwrap(); // picks + goes to Give mode
        assert!(matches!(st.item.as_ref().unwrap().mode, ItemMode::Give (_, _)));
        st.item.as_mut().unwrap().input = "3".to_string();
        on_items_key(KeyCode::Enter, &mut st).unwrap(); // applies
        assert!(st.msg_ok, "apply failed: {}", st.msg);
        // item must now be listed at that slot
        let items = open_items(&tmp, "Inventory").unwrap();
        assert_eq!(items.infos[free].as_ref().unwrap().base_name, "weapon:water:sword");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn picker_sort_toggle_and_category() {
        // temp folder so repo recents don't pollute the ordering assertions
        let folder = std::env::temp_dir().join("se_sort_test");
        std::fs::create_dir_all(&folder).ok();
        std::fs::copy("../Adolfina.char", folder.join("Adolfina.char")).unwrap();
        let path = folder.join("Adolfina.char");
        let mut st = test_state(&path);
        st.folder = folder.clone();
        st.item = Some(open_items(&path, "Inventory").unwrap());
        let free = st.item.as_ref().unwrap().infos.iter().position(|o| o.is_none()).unwrap();
        st.item.as_mut().unwrap().sel = free;
        on_items_key(KeyCode::Char('g'), &mut st).unwrap();
        // default order: rarity desc, best first
        {
            let p = st.picker.as_ref().unwrap();
            let first = items_db::get(&p.matches[0]).expect("first match in db");
            let (best_extra, best_rank) = crate::ops::rarity_rank(&p.matches[0]);
            assert!(!best_extra);
            assert_eq!(best_rank, u32::MAX - 6, "tier 6 sorts first");
            let _ = first;
        }
        // Tab toggles to name sort
        on_picker_key(KeyCode::Tab, &mut st).unwrap();
        {
            let p = st.picker.as_ref().unwrap();
            assert!(p.sort_by_name);
        }
        // F3 cycles into the first category, and only that prefix remains
        on_picker_key(KeyCode::F(3), &mut st).unwrap();
        {
            let p = st.picker.as_ref().unwrap();
            let cat = p.category.clone().expect("category set");
            assert!(p.matches.iter().all(|id| id.starts_with(&format!("{cat}:"))));
        }
        std::fs::remove_file(folder.join("Adolfina.char")).ok();
        std::fs::remove_file(recents_path(&folder)).ok();
    }

    #[test]
    fn picker_favorites_and_recents_persist() {
        let folder = std::env::temp_dir().join("se_favs_test");
        std::fs::create_dir_all(&folder).ok();
        // seed a save copy so picker has content
        std::fs::copy("../Adolfina.char", folder.join("Adolfina.char")).unwrap();
        let mut st = test_state(&Path::new(""));
        st.folder = folder.clone();
        st.files = list_char_files(&folder);
        st.item = Some(open_items(&st.files[0], "Inventory").unwrap());
        let free = st.item.as_ref().unwrap().infos.iter().position(|o| o.is_none()).unwrap();
        st.item.as_mut().unwrap().sel = free;
        on_items_key(KeyCode::Char('g'), &mut st).unwrap();
        st.picker.as_mut().unwrap().filter = "coal".into();
        refilter(st.picker.as_mut().unwrap());
        // F2 favorites the selection
        on_picker_key(KeyCode::F(2), &mut st).unwrap();
        let p0 = st.picker.as_ref().unwrap();
        let favs = st.picker.as_ref().unwrap().favs.clone();
        assert!(!favs.is_empty());
        // picker closed + recent recorded on Enter
        on_picker_key(KeyCode::Enter, &mut st).unwrap();
        let rec = load_recents(&folder);
        assert!(!rec.recent.is_empty(), "recent must persist");
        assert!(rec.favs.iter().any(|f| favs.contains(f)), "favs must persist");
        // fresh picker must pin the favorite first
        st.item = Some(open_items(&st.files[0], "Inventory").unwrap());
        let free = st.item.as_ref().unwrap().infos.iter().position(|o| o.is_none()).unwrap();
        st.item.as_mut().unwrap().sel = free;
        on_items_key(KeyCode::Char('g'), &mut st).unwrap();
        let p = st.picker.as_ref().unwrap();
        assert!(!p.matches.is_empty());
        assert!(p.favs.contains(&p.matches[0]), "favorite must sort first");
        std::fs::remove_file(folder.join("Adolfina.char")).ok();
        std::fs::remove_file(recents_path(&folder)).ok();
    }

    #[test]
    fn undo_restores_previous_state() {
        let src = PathBuf::from("../Adolfina.char");
        let tmp = std::env::temp_dir().join("se_undo.char");
        std::fs::copy(&src, &tmp).unwrap();
        let mut st = test_state(&src);
        st.item = Some(open_items(&tmp, "Inventory").unwrap());
        let free = st.item.as_ref().unwrap().infos.iter().position(|o| o.is_none()).unwrap();
        st.item.as_mut().unwrap().sel = free;
        // apply a give
        apply_action(&mut st, &tmp, |sf| {
            crate::ops::build_give(sf, &ItemSpec { target: "Inventory", id: "material:gold_bar".into(), count: 7, slot: None })
        });
        assert!(st.msg_ok);
        let items = open_items(&tmp, "Inventory").unwrap();
        assert!(items.infos.iter().flatten().any(|i| i.base_name == "material:gold_bar"));
        // ctrl+z equivalent
        undo_last(&mut st);
        assert!(st.msg_ok, "undo failed: {}", st.msg);
        let items = open_items(&tmp, "Inventory").unwrap();
        assert!(!items.infos.iter().flatten().any(|i| i.base_name == "material:gold_bar"));
        std::fs::remove_file(&tmp).ok();
    }

    fn test_state(_p: &Path) -> State {
        State {
            folder: state_folder(),
            files: vec![],
            save_sel: 0,
            menu: None,
            menu_sel: 0,
            item: None,
            picker: None,
            money_editing: false,
            money_input: String::new(),
            msg: String::new(),
            msg_ok: true,
            undo: Vec::new(),
            hdr: None,
            body: None,
        }
    }
}
