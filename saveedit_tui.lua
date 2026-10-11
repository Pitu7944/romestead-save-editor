-- saveedit_tui.lua — interactive frontend for saveedit.lua (Windows console)
--
-- Started via `lua saveedit.lua tui [folder]`. Requires a real console (raw
-- keys come from the embedded PowerShell helper in saveedit.lua) — otherwise
-- it exits with a clear error. Every edit goes through run(): .bak backup +
-- round-trip verification before anything is written.

local function load_core()
	local here = (arg and (arg[0] or "") or ""):match("^(.*[/\\])") or ""
	local core_path = here .. "saveedit.lua"
	if not select(1, io.open(core_path, "rb")) then core_path = "saveedit.lua" end
	SAVEEDIT_TUI_CHILD = true
	return assert(dofile(core_path))
end

local core = load_core()
local read_file, parse_data = core.read_file, core.parse_data
local find_entry, parse_object, parse_array, item_info, diff_value, read7 =
	core.find_entry, core.parse_object, core.parse_array, core.item_info, core.diff_value, core.read7
local start_key_reader, stop_key_reader = core.start_key_reader, core.stop_key_reader
local B = {
	Difficulty = core.build_difficulty_edit,
	Money = core.build_money_edit,
	give = core.build_give_edit,
	remove = core.build_remove_edit,
	stack = core.build_stack_edit,
}

-- ANSI rendering --------------------------------------------------------------
local CSI = string.char(27)
local C = {
	reset = CSI .. "[0m", dim = CSI .. "[2m", rev = CSI .. "[7m",
	title = CSI .. "[1;36m", ok = CSI .. "[32m", err = CSI .. "[31m",
	warn = CSI .. "[33m", cur = CSI .. "[42;30m",
}
local function cls() io.write(CSI .. "[2J", CSI .. "[H", CSI .. "[?25l") end
local function show_cursor() io.write(CSI .. "[?25h", C.reset) end

-- Key tokens (match the PowerShell key reader in saveedit.lua):
-- enter=13 backspace=8 esc=27 up=17 down=18 pgup=25 pgdn=26
local TK = {
	up = string.char(17), down = string.char(18), esc = string.char(27),
	enter = string.char(13), back = string.char(8),
	pgup = string.char(25), pgdn = string.char(26),
}

local function read_key(reader)
	local t = reader:read(1)
	if not t then error("console input died", 0) end
	return t
end

local function printable(t)
	return #t == 1 and t:byte(1) >= 32 and t:byte(1) < 127
end

-- Line input on the CURRENT line (CR clears + retypes). Returns string, nil on esc.
local function read_line(reader, prompt, initial, numeric_only, extra_ok)
	initial = initial or ""
	local s = initial
	while true do
		io.write("\r", CSI .. "[2K", C.title, prompt, C.reset, s, C.cur, " ", C.reset)
		local t = read_key(reader)
		if t == TK.enter then
			io.write("\n")
			return s
		elseif t == TK.esc then
			io.write("\n")
			return nil
		elseif t == TK.back then
			s = s:sub(1, #s - 1)
		elseif printable(t) then
			local b = t:byte(1)
			if not numeric_only or (b >= 48 and b <= 57) then
				if not extra_ok or (extra_ok:find(t, 1, true)) then
					if #s < 60 then s = s .. t end
				end
			end
		end
	end
end

-- Data helpers ----------------------------------------------------------------

local function collect_char_files(folder)
	folder = folder:gsub("[/\\]+$", "")
	local files = {}
	local out = io.popen('cmd /c dir /b "' .. folder .. '\\*.char" 2>NUL', "r")
	if out then
		for line in out:lines() do
			local name = line:match("^%s*(.-)%s*$")
			if name and name:lower():match("%.char$") then
				files[#files + 1] = folder .. "\\" .. name
			end
		end
		out:close()
	end
	return files
end

local function get_array(path, arrayName)
	local data = read_file(path)
	local pf = parse_data(data)
	local e = find_entry(pf.root, arrayName, pf.strings)
	assert(e, arrayName .. " field not found in save")
	return data, pf, parse_array(data, e)
end

local function infos_el(data, pf, el)
	if el.is_null then return nil end
	return item_info(data, pf, el)
end

local function read_money(data, pf)
	local m = find_entry(pf.root, "Money", pf.strings)
	if not m then return 0 end
	local v = 0
	for i = 7, 0, -1 do v = v * 256 + data:byte(m.pstart + i) end
	return v
end

local function char_name(data, pf)
	local cm = find_entry(pf.root, "CharacterModel", pf.strings)
	if not cm then return "?" end
	local cmo = parse_object(data, cm.pstart)
	local n = find_entry(cmo, "Name", pf.strings)
	if n and n.tc == 34 then
		return pf.strings[read7(data, n.pstart)] or "?"
	end
	return "?"
end

local function char_difficulty(data, pf)
	local cm = find_entry(pf.root, "CharacterModel", pf.strings)
	assert(cm)
	local cmo = parse_object(data, cm.pstart)
	local d = find_entry(cmo, "Difficulty", pf.strings)
	if not d then return "Normal" end
	local v = diff_value(data, d)
	return v == 0 and "Normal" or (v == 1 and "Softcore" or ("value " .. v))
end

local function save_line(path)
	local ok, line = pcall(function()
		local data = read_file(path)
		local pf = parse_data(data)
		return string.format("%-14s %-9s money %-10d",
			char_name(data, pf), char_difficulty(data, pf), read_money(data, pf))
	end)
	return ok and line or "(unreadable save)"
end

-- Apply an edit through run(); returns (success, message).
local function apply(file, build, verify)
	local ok, ret = pcall(function()
		return run(file, nil, function(pf, data) return build(pf, data) end, verify, true)
	end)
	if ok and ret then return true, ret end
	return false, "ERROR: " .. tostring(ret or "?")
end

-- Print a note and wait for any key (used after quick actions).
local function read_key_ok(reader, note)
	io.write(note, C.dim, "press any key to continue", C.reset)
	read_key(reader)
end

-- Windowed vertical list: arrows/pgup/pgdn move, enter -> index, esc -> nil.
local function list_pick(reader, title, lines, footer)
	local sel, top, window = 1, 1, 24
	while true do
		local buf = { CSI .. "[2J", CSI .. "[H", C.title, title, C.reset, "\n\n" }
		for i = top, math.min(#lines, top + window - 1) do
			buf[#buf + 1] = (i == sel and C.rev .. " > " or "   ") .. lines[i] .. C.reset .. "\n"
		end
		buf[#buf + 1] = "\n" .. C.dim .. (footer or "enter select | esc back") .. C.reset .. "\n"
		io.write(table.concat(buf))
		io.flush()
		local t = read_key(reader)
		if t == TK.up then
			if sel > 1 then sel = sel - 1; if sel < top then top = sel end end
		elseif t == TK.down then
			if sel < #lines then
				sel = sel + 1
				if sel > top + window - 1 then top = sel - window + 1 end
			end
		elseif t == TK.pgup then
			sel = math.max(1, sel - window); top = math.max(1, sel)
		elseif t == TK.pgdn then
			sel = math.min(#lines, sel + window); top = math.max(1, sel)
		elseif t == TK.enter then
			return sel
		elseif t == TK.esc then
			return nil
		end
	end
end

-- Item id picker: unique ids found across all saves in the folder.
-- Type-to-filter; enter selects, esc cancels.
local function pick_item_id(reader, folder, cache)
	if not cache.done then
		cache.done = true
		cache.ids = {}
		cache.seen_ids = {}
		for _, f in ipairs(collect_char_files(folder)) do
			pcall(function()
				local data = read_file(f)
				local pf = parse_data(data)
				for _, t in ipairs({ "Inventory", "Equipment", "SecondaryEquipment" }) do
					local e = find_entry(pf.root, t, pf.strings)
					if e then
						for _, el in ipairs(parse_array(data, e).elements) do
							local info = item_info(data, pf, el)
							if info and info.base_name ~= "?" and not cache.seen_ids[info.base_name] then
								cache.seen_ids[info.base_name] = true
								cache.ids[#cache.ids + 1] = info.base_name
							end
						end
					end
				end
			end)
		end
		table.sort(cache.ids)
	end

	local filter, sel = "", 1
	while true do
		local matches = {}
		for _, id in ipairs(cache.ids) do
			if filter == "" or id:find(filter, 1, true) then matches[#matches + 1] = id end
		end
		if sel > #matches then sel = math.max(1, #matches) end
		local buf = { CSI .. "[2J", CSI .. "[H", C.title, "Pick item id", C.reset,
			C.dim, "   type to filter | backspace edits | enter places item | esc cancels", C.reset, "\n\n",
			C.ok, filter, C.cur, "_", C.reset, "\n\n" }
		for i = 1, math.min(#matches, 24) do
			buf[#buf + 1] = (i == sel and C.rev .. " > " or "   ") .. matches[i] .. C.reset .. "\n"
		end
		if #matches == 0 then
			buf[#buf + 1] = C.warn .. "  no id matches '" .. filter .. "'" .. C.reset .. "\n"
		end
		io.write(table.concat(buf))
		io.flush()
		local t = read_key(reader)
		if t == TK.enter and #matches > 0 then
			return matches[sel]
		elseif t == TK.esc then
			return nil
		elseif t == TK.back then
			filter = filter:sub(1, #filter - 1)
			sel = 1
		elseif printable(t) then
			filter = filter .. t
			sel = 1
		end
	end
end

-- Give flow for an empty slot. Returns message or nil.
local function give_at(reader, file, arrayName, slot, folder, cache)
	io.write("\n")
	local id = read_line(reader, "item id (empty = pick from list): ", "", false, "-:")
	if id == nil then return nil end
	if id == "" then
		local picked = pick_item_id(reader, folder, cache)
		if not picked then return nil end
		id = picked
	end
	local cnt_s = read_line(reader, "stack count (enter = 1): ", "1", true)
	if cnt_s == nil then return nil end
	local count = tonumber(cnt_s) or 1
	local ok, msg = apply(file, function(pf, data)
		return B.give(pf, data, arrayName, id, count, slot) end, core.verify_item)
	return msg, ok
end

-- Item browser for one array. Returns nil = back / "quit".
local function items_screen(reader, file, arrayName, folder, cache)
	local sel, top, msg, ok = 1, 1, nil, true
	local window = 26
	while true do
		local data, pf, arr = get_array(file, arrayName)
		if sel > #arr.elements then sel = #arr.elements end
		if sel < top then top = sel end
		if sel >= top + window then top = sel - window + 1 end

		local used = 0
		for _, el in ipairs(arr.elements) do if not el.is_null then used = used + 1 end end
		local buf = { CSI .. "[2J", CSI .. "[H", C.title,
			file:match("[^/\\]+$"), C.reset, " — ", C.title, arrayName, C.reset,
			C.dim, string.format("   (%d/%d used)   esc back", used, arr.count), C.reset, "\n\n" }
		if msg then
			buf[#buf + 1] = (ok and C.ok or C.err) .. msg .. C.reset .. "\n\n"
			msg, ok = nil, true
		end
		for i = top, math.min(#arr.elements, top + window) do
			local info = infos_el(data, pf, arr.elements[i])
			local txt = info and
				string.format("slot %2d | %-34s x%-4d%s", i, info.base_name, info.count,
					info.has_usable and "  [usable]" or "") or
				string.format("slot %2d | (empty)", i)
			buf[#buf + 1] = (i == sel and C.rev .. " > " or "   ") .. txt .. C.reset .. "\n"
		end
		buf[#buf + 1] = "\n" .. C.dim ..
			"arrows move | g give at slot | r remove | c set stack | esc back" .. C.reset .. "\n"
		io.write(table.concat(buf))
		io.flush()

		local t = read_key(reader)
		if t == TK.esc then return nil end

		if t == TK.up or t == TK.down or t == TK.pgup or t == TK.pgdn then
			local d = (t == TK.up and -1) or (t == TK.down and 1) or (t == TK.pgup and -window)
				or window
			sel = math.max(1, math.min(#arr.elements, sel + d))
			if sel < top then top = sel end
			if sel > top + window - 1 then top = sel - window + 1 end
		elseif t == "g" then
			if arr.elements[sel].is_null then
				msg, ok = give_at(reader, file, arrayName, sel, folder, cache)
				if msg == nil then msg, ok = "cancelled", false end
			else
				msg, ok = "ERROR: slot " .. sel .. " is occupied — remove first", false
			end
		elseif t == "r" then
			if infos_el(data, pf, arr.elements[sel]) then
				msg, ok = select(2, apply(file, function(p2, d2)
					return B.remove(p2, d2, arrayName, sel) end, core.verify_item))
			else
				msg, ok = "slot " .. sel .. " is already empty", false
			end
		elseif t == "c" then
			local info = infos_el(data, pf, arr.elements[sel])
			if not info then
				msg, ok = "ERROR: slot " .. sel .. " is empty", false
			else
				io.write("\n")
				local cs = read_line(reader, "new stack count: ", tostring(info.count or 1), true)
				if cs then
					local n = tonumber(cs)
					if n and n > 0 then
						msg, ok = select(2, apply(file, function(p2, d2)
							return B.stack(p2, d2, arrayName, sel, n) end, core.verify_item))
					else
						msg, ok = "ERROR: count must be > 0", false
					end
				else
					msg, ok = "cancelled", false
				end
			end
		end
	end
end

-- Edit menu for one save. Returns nil (back) / "quit".
local function edit_menu(reader, file, folder, cache)
	local names = { "Inventory", "Equipment", "SecondaryEquipment" }
	while true do
		local data = read_file(file)
		local pf = parse_data(data)
		local diff = char_difficulty(data, pf)
		local lines = {
			"Toggle difficulty      now: " .. diff,
			"Set money              now: " .. read_money(data, pf),
			"Inventory",
			"Equipment",
			"SecondaryEquipment",
		}
		local sel = list_pick(reader, file:match("[^/\\]+$") .. "  " .. save_line(file), lines,
			"enter select | esc back to saves")
		if sel == nil then return nil end
		if sel == 1 then
			local want_soft = diff ~= "Softcore"
			local okmsg = select(2, apply(file, function(p2, d2)
				return B.Difficulty(p2, d2, want_soft) end, core.verify_difficulty))
			io.write("\n")
			read_key_ok(reader, "\n" .. (okmsg or "difficulty switched") .. "\n")
		elseif sel == 2 then
			io.write("\n")
			local s = read_line(reader, "money (enter keeps, -0 clears): ",
				tostring(read_money(data, pf)), true)
			if s then
				local amount = tonumber(s)
				if amount then
					local okmsg = select(2, apply(file, function(p2, d2)
						return B.Money(p2, d2, math.floor(math.max(0, amount))) end, core.verify_money))
					read_key_ok(reader, "\n" .. (okmsg or "money set") .. "\n")
				else
					read_key_ok(reader, "\nERROR: not a number: '" .. s .. "'\n")
				end
			end
		else
			local r = items_screen(reader, file, names[sel - 2], folder, cache)
			if r == "quit" then return "quit" end
		end
	end
end

-- Main: save picker then edit menus.
function main(folder)
	local reader, m_in, m_out = start_key_reader()
	if not reader then
		io.stderr:write("TUI needs a real Windows console + PowerShell for key input.\n" ..
			"Use the CLI subcommands instead, e.g.: lua saveedit.lua dump Marcella.char\n")
		os.exit(2)
	end

	local ok, err = pcall(function()
		local cache = { done = false, ids = {}, seen_ids = {} }
		while true do
			local files = collect_char_files(folder)
			if #files == 0 then error("no .char files found in folder " .. folder, 0) end
			local lines = {}
			for i, f in ipairs(files) do
				lines[i] = string.format("%-18s %s", f:match("[^/\\]+$"), save_line(f))
			end
			local sel = list_pick(reader, "Romestead Save Editor — " .. folder, lines,
				"enter edit save | esc quit")
			if sel == nil then return end
			local r = edit_menu(reader, files[sel], folder, cache)
			if r == "quit" then return end
		end
	end)

	local tmp = os.getenv("TEMP") or "."
	local ps1 = tmp:gsub("[/\\]+$", "") .. "\\saveedit_restore.ps1"
	local f = io.open(ps1, "wb")
	if f then
		f:write("param($im,$om)\n" ..
			"Add-Type -TypeDefinition @'\n" ..
			"using System;\nusing System.Runtime.InteropServices;\n" ..
			"public static class W2 {\n" ..
			'  [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr GetStdHandle(int h);\n' ..
			'  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetConsoleMode(IntPtr h, uint m);\n' ..
			"}\n'@\n" ..
			"[W2]::SetConsoleMode([W2]::GetStdHandle(-10), [uint32]$im)\n" ..
			"[W2]::SetConsoleMode([W2]::GetStdHandle(-11), [uint32]$om)\n")
		f:close()
		os.execute('powershell -NoProfile -ExecutionPolicy Bypass -File "' .. ps1 .. '" ' ..
			tostring(m_in) .. " " .. tostring(m_out) .. " 2>NUL 1>NUL")
	end
	stop_key_reader(reader)
	show_cursor()
	if not ok then
		io.stderr:write("TUI: " .. tostring(err) .. "\n")
		os.exit(1)
	end
end

return { main = main }