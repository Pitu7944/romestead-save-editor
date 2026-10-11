-- saveedit.lua — edit Romestead .char saves (difficulty, money)
--
-- Requires Lua 5.3+ (written/tested on 5.4). No dependencies.
--
--   lua saveedit.lua dump       <file>
--   lua saveedit.lua difficulty <file> <softcore|normal> [outfile]
--   lua saveedit.lua money      <file> <amount>         [outfile]
--   lua saveedit.lua enable     <file> [outfile]        -- alias: difficulty softcore
--
-- outfile defaults to the input file. A .bak of the ORIGINAL save is always
-- written next to the input before anything is modified.
--
-- Save format (CandideSerializer, compact-strings mode, magic "ceb\x01"):
--   [4B magic][string map][payload]
--   string map:  [7bit count][0x21 elem-typecode][count raw strings (7bit len + utf8)]
--                string id = its index in the list (1-based)
--   payload:     [0x20 CompactObject][7bit numFields][7bit totalLen][field entries]
--   field entry: [typecode][7bit nameId][7bit payloadLen][payload]
--   Difficulty (enum): typecode 38 (NullableEnum), payload [3 (Int32)][int32 LE]
--     0 = Normal, 1 = Softcore. Lives in the CharacterModel sub-object.
--   Money: typecode 20 (UInt64), payload = 8 bytes LE. Lives in the root object.
--
-- Key trick: CandideSerializer omits fields whose value equals the type default
-- (GenericTypeHandler.Serialize). Difficulty.Normal=0 and Money=0 are therefore
-- ABSENT from most saves. Enabling them means inserting properly-formed entries,
-- adding the field name to the string map, and growing all enclosing length
-- headers. The game's deserializer is fully self-describing per field (length
-- prefixes), so appended entries are safe even if newer save versions add
-- unknown fields.

local LSL = 0x80 -- 7-bit int continuation bit

local function read7(data, p)
	local val, shift = 0, 0
	while true do
		local b = data:byte(p)
		if not b then error("truncated file at byte " .. p) end
		p = p + 1
		val = val + (b % LSL) * 2 ^ shift
		if b < LSL then break end
		shift = shift + 7
	end
	return val, p
end

local function write7(n)
	local out = {}
	while true do
		local b = n % LSL
		n = math.floor(n / LSL)
		if n > 0 then b = b + LSL end
		out[#out + 1] = string.char(b)
		if n == 0 then break end
	end
	return table.concat(out)
end

local function u32(v)
	assert(v >= 0 and v < 2 ^ 32, "value out of UInt32 range")
	return string.char(v % 256, math.floor(v / 256) % 256,
		math.floor(v / 65536) % 256, math.floor(v / 16777216) % 256)
end

local function u64(v)
	assert(v >= 0 and v < 2 ^ 53, "value out of range (keep it below 2^53)")
	local lo = v % 2 ^ 32
	local hi = math.floor(v / 2 ^ 32)
	assert(hi < 65536, "value out of range")
	return string.char(lo % 256, math.floor(lo / 256) % 256,
			math.floor(lo / 65536) % 256, math.floor(lo / 16777216) % 256,
			hi % 256, math.floor(hi / 256) % 256, math.floor(hi / 65536) % 256,
			math.floor(hi / 16777216) % 256)
end

local function read_file(path)
	local f = assert(io.open(path, "rb"))
	local data = f:read("a")
	f:close()
	assert(data and #data > 0, "empty file: " .. path)
	return data
end

local function write_file(path, data)
	local f = assert(io.open(path, "wb"))
	f:write(data)
	f:close()
end

local MAGIC = "ceb"
local DIFF_ENUM_TC = 38 -- NullableEnum
local SOFTCORE_PAYLOAD = "\3\1\0\0\0" -- payload [Int32][Softcore=1]

-- Parse one object: [num][total][entries]; pos = first byte of num.
local function parse_object(data, pos)
	local num, p = read7(data, pos)
	local total, p2 = read7(data, p)
	local obj = {
		num = num, total = total,
		num_start = pos, total_start = p, total_end = p2 - 1,
		body_start = p2, entries = {},
	}
	local cur = p2
	for _ = 1, num do
		local tc = data:byte(cur)
		local name_pos = cur + 1
		local id, q = read7(data, name_pos)
		local plen, r = read7(data, q)
		obj.entries[#obj.entries + 1] = {
			tc = tc, id = id, name_pos = name_pos, plen_pos = q, plen = plen,
			pstart = r, pend = r + plen - 1,
		}
		cur = r + plen
	end
	obj.body_end = cur
	assert(cur - obj.body_start == total,
		string.format("object body mismatch: expected total %d, got %d", total, cur - obj.body_start))
	return obj
end

-- Returns {..., strings, mode("once"|"none"|"per"), root_start, root}.
local function parse_data(data)
	assert(data:sub(1, 3) == MAGIC, "not a .char file (bad magic)")
	assert(data:byte(4) == 1, "non-compact save format not supported")
	local count, q = read7(data, 5)

	local function read_strings(start, mode)
		local strings, pos = {}, start
		for i = 1, count do
			if mode == "per" then
				assert(data:byte(pos) == 33, "map desync at " .. pos)
				pos = pos + 1
			end
			local len, r = read7(data, pos)
			strings[i] = data:sub(r, r + len - 1)
			pos = r + len
		end
		return strings, pos
	end

	for _, mode in ipairs({ "once", "none", "per" }) do
		local start = (mode == "once") and q + 1 or q
		local ok, res = pcall(function()
			local strings, pos = read_strings(start, mode)
			if data:byte(pos) ~= 32 then return nil end -- root must be CompactObject(0x20)
			local root = parse_object(data, pos + 1)
			if root.body_end ~= #data + 1 then return nil end -- must consume exactly the file
			return { strings = strings, mode = mode, root_start = pos, root = root }
		end)
		if ok and res then return res end
	end
	error("could not parse string map / payload (unknown layout)")
end

-- Byte edits over original coordinates: {start, oldlen, new}.
local function apply_edits(data, edits)
	table.sort(edits, function(a, b)
		if a[1] ~= b[1] then return a[1] > b[1] end
		return a[2] > b[2]
	end)
	for _, e in ipairs(edits) do
		data = data:sub(1, e[1] - 1) .. e[3] .. data:sub(e[1] + e[2])
	end
	return data
end

local function find_entry(obj, name, strings)
	for _, e in ipairs(obj.entries) do
		if (strings[e.id] or "?") == name then return e end
	end
	return nil
end

-- Get (or add) a string-map key. Returns id, edits, string_bytes (nil if the
-- key already exists and nothing needs changing).
-- Get (or reserve) a string-map key. Multiple new keys within one build are
-- batched on pf._pending and appended in call order with sequential ids.
-- Returns just the id; call finalize_map_edits once after all ensure calls.
local function ensure_map_key(pf, data, name)
	pf._key_ids = pf._key_ids or {}
	if pf._key_ids[name] then return pf._key_ids[name] end
	for i, s in ipairs(pf.strings) do
		if s == name then pf._key_ids[name] = i return i end
	end
	local added = pf._key_count or 0
	local new_id = #pf.strings + added + 1
	pf._key_ids[name] = new_id
	-- "per" mode writes a 0x21 typecode before EVERY element; the leading 0x21
	-- in "once" mode belongs to the list header, not to each key.
	local string_bytes = (pf.mode == "per" and "\33" or "") .. write7(#name) .. name
	pf._pending = pf._pending or { keys = {} }
	pf._pending.keys[#pf._pending.keys + 1] = string_bytes
	pf._key_count = added + 1
	return new_id
end

-- Flush all pending keys: one count bump + one appended-bytes insert.
local function finalize_map_edits(pf, data)
	local pending = pf._pending
	if not pending or #pending.keys == 0 then
		pf._pending = nil
		return {}
	end
	local count, q = read7(data, 5)
	local edits = {
		{ 5, q - 5, write7(count + #pending.keys) }, -- bump map count (right after magic)
		{ pf.root_start, 0, table.concat(pending.keys) }, -- append keys at end of map
	}
	pf._pending = nil
	return edits
end

-- Insert a pre-built field entry at the end of parent's body and grow the
-- containers. outer_entry = the parent's own entry inside the root object
-- (nil if parent IS the root).
local function insert_field(pf, parent, outer_entry, edits, entry_bytes)
	local ins = #entry_bytes
	local hdr = write7(parent.num + 1) .. write7(parent.total + ins)
	assert(#hdr == parent.total_end - parent.num_start + 1,
		"object header width change — refusing (save layout newer than this tool)")
	edits[#edits + 1] = { parent.body_end, 0, entry_bytes }
	edits[#edits + 1] = { parent.num_start, #hdr, hdr }

	if outer_entry then
		assert(#write7(outer_entry.plen + ins) == outer_entry.pstart - outer_entry.plen_pos,
			"entry length width change — refusing")
		edits[#edits + 1] = { outer_entry.plen_pos, outer_entry.pstart - outer_entry.plen_pos,
			write7(outer_entry.plen + ins) }
		local root = pf.root
		assert(#write7(root.total + ins) == root.total_end - root.total_start + 1,
			"root total width change — refusing")
		edits[#edits + 1] = { root.total_start, root.total_end - root.total_start + 1,
			write7(root.total + ins) }
	end
end

local function diff_value(data, entry)
	assert(data:byte(entry.pstart) == 3, "unexpected Difficulty inner typecode " .. data:byte(entry.pstart))
	return data:byte(entry.pstart + 1) + data:byte(entry.pstart + 2) * 256
		+ data:byte(entry.pstart + 3) * 65536 + data:byte(entry.pstart + 4) * 16777216
end

local function read_u32(data, p)
	return data:byte(p) + data:byte(p + 1) * 256 + data:byte(p + 2) * 65536
		+ data:byte(p + 3) * 16777216
end

-- Parse an object array ([7bit count][elemTc][elements...]) whose elements are
-- interface-typed (tc 48): null slots are [0][0], objects are
-- [asmId][typeId][0x20][num][total][entries].
local function parse_array(data, entry)
	local cnt, q = read7(data, entry.pstart)
	local tc = data:byte(q)
	assert(tc == 48 or tc == 32, "array element typecode " .. tc .. " not supported for editing")
	local arr = { count = cnt, count_pos = entry.pstart, count_end = q - 1,
		elem_tc = tc, elements = {} }
	local pos = q + 1
	for _ = 1, cnt do
		if tc == 48 then
			local asm, nq = read7(data, pos)
			local tid, nq2 = read7(data, nq)
			if asm == 0 and tid == 0 then
				arr.elements[#arr.elements + 1] = { is_null = true, start = pos, end_ = nq2 - 1 }
				pos = nq2
			else
				assert(data:byte(nq2) == 32, "unexpected interface element typecode " .. data:byte(nq2))
				local obj = parse_object(data, nq2 + 1)
				arr.elements[#arr.elements + 1] = { is_null = false, start = pos,
					end_ = obj.body_end - 1, asm_id = asm, type_id = tid, obj = obj }
				pos = obj.body_end
			end
		else
			local obj = parse_object(data, pos)
			arr.elements[#arr.elements + 1] = { is_null = false, start = pos,
				end_ = obj.body_end - 1, asm_id = nil, type_id = nil, obj = obj }
			pos = obj.body_end
		end
	end
	arr.end_ = pos - 1
	return arr
end

local function item_info(data, pf, elem)
	if elem.is_null then return nil end
	local obj = elem.obj
	local base = find_entry(obj, "BaseDataId", pf.strings) -- compact string -> string id
	local stack = find_entry(obj, "StackCount", pf.strings)
	local base_name, count = "?"
	if base then
		local sid = read7(data, base.pstart)
		base_name = pf.strings[sid] or "?"
	end
	if stack then
		count = read_u32(data, stack.pstart)
	end
	return { base_name = base_name, count = count, has_usable = find_entry(obj, "UsableInfo", pf.strings) ~= nil }
end

-- Build a full ItemInstanceModel element (interface-typed object).
local function build_item_element(pf, data, base_id, count)
	math.randomseed(os.time() + math.floor(os.clock() * 1e6) + #pf.strings)
	local gb = {}
	for i = 1, 16 do gb[i] = string.char(math.random(0, 255)) end
	gb[16] = string.char(math.random(128, 191)) -- RFC4122-ish variant bits
	local guid = table.concat(gb)

	local edits = {}
	local asm_id = ensure_map_key(pf, data, "Shared")
	local type_id = ensure_map_key(pf, data, "Shared.Models.Items.ItemInstanceModel")
	local base_name_id = ensure_map_key(pf, data, "BaseDataId")
	local base_val_id = ensure_map_key(pf, data, base_id)
	local id_name_id = ensure_map_key(pf, data, "Id")
	local inv_name_id = ensure_map_key(pf, data, "InventoryId")
	local inv_asm_id = ensure_map_key(pf, data, "System.Private.CoreLib")
	local inv_type_id = ensure_map_key(pf, data, "System.Guid")
	local auras_name_id = ensure_map_key(pf, data, "Auras")
	local stack_name_id = ensure_map_key(pf, data, "StackCount")
	for _, ed in ipairs(finalize_map_edits(pf, data)) do edits[#edits + 1] = ed end

	-- Field entries in declaration order: BaseDataId, Id, InventoryId, Auras, StackCount
	local body = {}
	body[#body + 1] = string.char(34) .. write7(base_name_id) .. write7(#write7(base_val_id)) .. write7(base_val_id)
	body[#body + 1] = string.char(37) .. write7(id_name_id) .. write7(16) .. guid
	body[#body + 1] = string.char(48) .. write7(inv_name_id)
		.. write7(#write7(inv_asm_id) + #write7(inv_type_id) + 17)
		.. write7(inv_asm_id) .. write7(inv_type_id) .. "\37" .. guid
	body[#body + 1] = string.char(49) .. write7(auras_name_id) .. write7(1) .. "\0"
	body[#body + 1] = string.char(3) .. write7(stack_name_id) .. write7(4) .. u32(count)
	local entries = table.concat(body)

	local elem = write7(asm_id) .. write7(type_id) .. "\32"
		.. write7(5) .. write7(#entries) .. entries
	return elem, edits
end

-- Returns edits + message + new_value, or nil + message.
local function build_difficulty_edit(pf, data, want_softcore)
	local strings = pf.strings
	local cm = find_entry(pf.root, "CharacterModel", strings)
	assert(cm, "CharacterModel field not found in root object")
	local cmo = parse_object(data, cm.pstart)
	local diff = find_entry(cmo, "Difficulty", strings)
	local payload = "\3" .. u32(want_softcore and 1 or 0)

	if diff then
		local cur = data:sub(diff.pstart, diff.pend)
		assert(diff.plen == 5, "Difficulty present but unexpected payload length " .. diff.plen)
		if cur == payload then
			return nil, want_softcore and "already Softcore" or "already Normal"
		end
		return { { diff.pstart, 5, payload } }, "changing Difficulty to " ..
			(want_softcore and "Softcore" or "Normal"), want_softcore and 1 or 0
	end

	if not want_softcore then
		return nil, "already Normal (field absent)"
	end

	local new_id = ensure_map_key(pf, data, "Difficulty")
	local edits = finalize_map_edits(pf, data)
	local entry_bytes = string.char(DIFF_ENUM_TC) .. write7(new_id) .. write7(5) .. payload
	insert_field(pf, cmo, cm, edits, entry_bytes)
	return edits, "inserting Difficulty field (Softcore)", 1
end

-- Returns edits + message + new_value, or nil + message.
local function build_money_edit(pf, data, amount)
	local money = find_entry(pf.root, "Money", pf.strings)
	local payload = u64(amount)

	if money then
		assert(money.tc == 20 and money.plen == 8, "Money field has unexpected layout")
		local cur = data:sub(money.pstart, money.pend)
		if cur == payload then
			return nil, string.format("Money already %d", amount)
		end
		return { { money.pstart, 8, payload } }, string.format("changing Money to %d", amount), amount
	end

	if amount == 0 then
		return nil, "Money already 0 (field absent)"
	end

	local new_id = ensure_map_key(pf, data, "Money")
	local edits = finalize_map_edits(pf, data)
	local entry_bytes = string.char(20) .. write7(new_id) .. write7(8) .. payload
	insert_field(pf, pf.root, nil, edits, entry_bytes)
	return edits, string.format("inserting Money field (%d)", amount), amount
end

local function dump(path)
	local data = read_file(path)
	local pf = parse_data(data)
	print(string.format("%s: %d strings (map mode %s), payload at %d, root fields=%d",
		path, #pf.strings, pf.mode, pf.root_start, #pf.root.entries))
	for _, e in ipairs(pf.root.entries) do
		print(string.format("  root.%-30s tc=%-3d len=%d", pf.strings[e.id] or "?", e.tc, e.plen))
	end
	local cm = find_entry(pf.root, "CharacterModel", pf.strings)
	assert(cm, "no CharacterModel field in root")
	local cmo = parse_object(data, cm.pstart)
	print("CharacterModel:")
	for _, e in ipairs(cmo.entries) do
		local n = pf.strings[e.id] or "?"
		local val = ""
		if n == "Difficulty" then
			local v = diff_value(data, e)
			val = string.format(" -> %d (%s)", v, v == 0 and "Normal" or (v == 1 and "Softcore" or "?"))
		end
		print(string.format("  char.%-30s tc=%-3d len=%d%s", n, e.tc, e.plen, val))
	end
	local money = find_entry(pf.root, "Money", pf.strings)
	if money then
		local v = 0
		for i = 7, 0, -1 do v = v * 256 + data:byte(money.pstart + i) end
		print(string.format("  root.Money = %d", v))
	else
		print("  root.Money = 0 (field absent)")
	end
end

-- Shared runner: build edits, apply, verify round-trip, back up, write.
-- Returns a status message (also printed unless quiet).
local function run(path, outfile, build, verify, quiet)
	local data = read_file(path)
	local pf = parse_data(data)
	local edits, msg, new_value = build(pf, data)
	if not edits then
		if not quiet then print(path .. ": " .. msg) end
		return msg
	end
	local patched = apply_edits(data, edits)

	-- Verify: patched bytes must re-parse cleanly and carry the new value.
	local pf2 = parse_data(patched)
	verify(patched, pf2, new_value)

	-- Always back up the ORIGINAL save before modifying anything.
	write_file(path .. ".bak", data)
	outfile = outfile or path
	write_file(outfile, patched)
	local done = string.format("%s: %s. Backup: %s.bak. Wrote %s (%d bytes, was %d).",
		path, msg, path, outfile, #patched, #data)
	if not quiet then print(done) end
	return done
end

local function verify_difficulty(patched, pf2, expected)
	local cm2 = find_entry(pf2.root, "CharacterModel", pf2.strings)
	assert(cm2, "CharacterModel missing after patch")
	local cmo2 = parse_object(patched, cm2.pstart)
	local d2 = find_entry(cmo2, "Difficulty", pf2.strings)
	assert(d2, "Difficulty missing after patch")
	assert(diff_value(patched, d2) == expected, "Difficulty value wrong after patch")
end

local function verify_money(patched, pf2, expected)
	local m2 = find_entry(pf2.root, "Money", pf2.strings)
	assert(m2, "Money missing after patch")
	local v = 0
	for i = 7, 0, -1 do v = v * 256 + patched:byte(m2.pstart + i) end
	assert(v == expected, string.format("Money value wrong after patch: %d", v))
end

-- TUI support ---------------------------------------------------------------
-- Windows console can't do single-key input from pure Lua, so a small embedded
-- PowerShell helper reads console key events (kernel32 ReadConsoleInput) and
-- emits one token byte per key over a pipe. It also enables ANSI/VT output and
-- reports the console modes so Lua can restore them on exit.
-- Key tokens: CR enter, BS backspace, ESC esc, UP=17 DOWN=18,
-- PgUp=25, PgDn=26 (see string.char table below).
local TOKEN = {
	up = string.char(17), down = string.char(18), enter = string.char(13),
	esc = string.char(27), back = string.char(8),
	pgup = string.char(25), pgdn = string.char(26)
}

local function start_key_reader()
	local tmp = os.getenv("TEMP") or "."
	local ps1 = tmp:gsub("[\\/]+$", "") .. "\\saveedit_keyreader.ps1"
	local f = io.open(ps1, "wb")
	if not f then return nil end
	f:write([=[
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class W {
  [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr GetStdHandle(int h);
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GetConsoleMode(IntPtr h, out uint m);
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetConsoleMode(IntPtr h, uint m);
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool ReadConsoleInput(IntPtr h, byte[] rec, uint n, out uint r);
  [DllImport("kernel32.dll", SetLastError=true)] public static extern int WaitForSingleObject(IntPtr h, int ms);
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool WriteFile(IntPtr h, byte[] b, uint n, out uint w, IntPtr o);
}
'@
function FlushTok([string]$s) { $b = [System.Text.Encoding]::ASCII.GetBytes($s)
  $w = [uint32]0
  $ok = [W]::WriteFile([W]::GetStdHandle(-11), $b, [uint32]$b.Length, [ref]$w, [IntPtr]::Zero)
  if (-not $ok) { throw 'pipe closed' } }
$inH = [W]::GetStdHandle(-10)
$outH = [W]::GetStdHandle(-11)
$inMode = [uint32]0; $outMode = [uint32]0
[void][W]::GetConsoleMode($inH, [ref]$inMode)
[void][W]::GetConsoleMode($outH, [ref]$outMode)
FlushTok ("M:" + $inMode + ":" + $outMode)
# enable VT processing on the screen buffer for ANSI rendering
$vtOk = [W]::SetConsoleMode($outH, ($outMode -bor 0x0004))
FlushTok ("V:" + [int]$vtOk)
$rec = New-Object byte[] 32
$read = [uint32]0
while ($true) {
  # wait up to 150ms for console input; then poke the stdout pipe: if the tool
  # exited (pipe closed), restore console modes and quit
  $wait = [W]::WaitForSingleObject($inH, 150)
  if ($wait -eq 0) {
    $ok = [W]::ReadConsoleInput($inH, $rec, 1, [ref]$read)
    if (-not $ok -or $read -eq 0) { continue }
    $ev = [BitConverter]::ToUInt16($rec, 0)
    if ($ev -ne 1) { continue }
    if ([BitConverter]::ToUInt32($rec, 4) -eq 0) { continue } # key up
    $vk = [BitConverter]::ToUInt16($rec, 10)
    $ch = [BitConverter]::ToChar($rec, 14)
    $tok = $null
    if ($vk -eq 38) { $tok = [char]0x11 }
    elseif ($vk -eq 40) { $tok = [char]0x12 }
    elseif ($vk -eq 33) { $tok = [char]0x19 }
    elseif ($vk -eq 34) { $tok = [char]0x1a }
    elseif ($vk -eq 13) { $tok = [char]13 }
    elseif ($vk -eq 27) { $tok = [char]27 }
    elseif ($vk -eq 8) { $tok = [char]8 }
    elseif ($vk -eq 9) { $tok = [char]9 }
    elseif ($ch -ne [char]0) { $tok = $ch }
    if ($tok -ne $null) { FlushTok ([string]$tok) }
  } else {
    $b = [byte[]]@(); $w = [uint32]0
    $ok = [W]::WriteFile($outH, $b, 0, [ref]$w, [IntPtr]::Zero)
    if (-not $ok) {
      [void][W]::SetConsoleMode($outH, $outMode)
      exit 0
    }
  }
}
]=])
	f:close()
	local reader = io.popen('powershell -NoProfile -ExecutionPolicy Bypass -File "' .. ps1 .. '"', "r")
	if not reader then return nil end
	-- handshake lines: console modes, VT status
	local mline = reader:read("l")
	local m_in, m_out = mline and mline:match("^M:(%d+):(%d+)")
	local vline = reader:read("l")
	local vt_ok = vline and vline:match("^V:(%d+)")
	if not m_in or not vt_ok then
		pcall(function() reader:close() end)
		return nil
	end
	return reader, m_in, m_out, vt_ok == "1"
end

local function stop_key_reader(reader)
	if reader then pcall(function() reader:close() end) end
end

local TARGETS = { inventory = "Inventory", equipment = "Equipment", secondary = "SecondaryEquipment" }

local cmds = {}

local function target_of(name)
	local t = TARGETS[name or "inventory"]
	assert(t, "target must be 'inventory', 'equipment' or 'secondary'")
	return t
end

local function get_array(path, target_name)
	local data = read_file(path)
	local pf = parse_data(data)
	local e = find_entry(pf.root, target_name, pf.strings)
	assert(e, target_name .. " field not found in save")
	return data, pf, parse_array(data, e)
end

local function verify_item_array(patched, pf2, v, want_null)
	local e = find_entry(pf2.root, v.target, pf2.strings)
	assert(e, v.target .. " missing after patch")
	local arr = parse_array(patched, e)
	local el = assert(arr.elements[v.slot], "slot missing after patch")
	local info = item_info(patched, pf2, el)
	if want_null then
		assert(info == nil, "slot " .. v.slot .. " not emptied after patch")
	else
		assert(info, "slot " .. v.slot .. " empty after patch")
		assert(info.base_name == v.id, "item id wrong after patch: " .. tostring(info.base_name))
		assert(info.count == v.count, "stack count wrong after patch")
	end
end

cmds.dump = function(arg)
	dump(assert(arg and arg[1], "usage: lua saveedit.lua dump <file>"))
end

cmds.items = function(arg)
	local file = assert(arg and arg[1], "usage: lua saveedit.lua items <file> [inventory|equipment|secondary]")
	local target = target_of(arg[2])
	local data, pf, arr = get_array(file, target)
	local used = 0
	for _, el in ipairs(arr.elements) do if not el.is_null then used = used + 1 end end
	print(string.format("%s: %s — %d slots, %d used", file, target, arr.count, used))
	for i, el in ipairs(arr.elements) do
		local info = item_info(data, pf, el)
		if not info then
			print(string.format("  slot %2d: (empty)", i))
		else
			print(string.format("  slot %2d: %s x%s%s", i, info.base_name, info.count,
				info.has_usable and " (usable)" or ""))
		end
	end
end

-- Give/remove/stack builders ---------------------------------------------

-- Returns edits + message + value, or nil + message.
local function build_give_edit(pf, data, target, base_id, count, slot_opt)
	local e = find_entry(pf.root, target, pf.strings)
	assert(e, target .. " field not found in save")
	local arr = parse_array(data, e)

	local slot = slot_opt
	if not slot then
		local free
		for i, el in ipairs(arr.elements) do
			if el.is_null then free = i break end
		end
		assert(free, target .. " is full — remove something first or pick a specific slot")
		slot = free
	else
		local el = arr.elements[slot]
		assert(el, string.format("slot %d out of range (1..%d)", slot, arr.count))
		assert(el.is_null, string.format("slot %d is occupied", slot))
	end

	local elem, edits = build_item_element(pf, data, base_id, count)
	for _, ed in ipairs(finalize_map_edits(pf, data)) do edits[#edits + 1] = ed end
	local target_el = arr.elements[slot]
	local oldlen = target_el.end_ - target_el.start + 1
	assert(target_el.is_null and oldlen == 2, "can only replace empty slots")
	edits[#edits + 1] = { target_el.start, 2, elem }

	local ins = #elem - 2
	local plen_new = e.plen + ins
	assert(#write7(plen_new) == e.pstart - e.plen_pos, "entry length width change")
	edits[#edits + 1] = { e.plen_pos, e.pstart - e.plen_pos, write7(plen_new) }
	local rtotal = pf.root.total + ins
	assert(#write7(rtotal) == pf.root.total_end - pf.root.total_start + 1, "root total width change")
	edits[#edits + 1] = { pf.root.total_start, pf.root.total_end - pf.root.total_start + 1, write7(rtotal) }
	return edits, string.format("%s slot %d: added %s x%d", target, slot, base_id, count),
		{ target = target, slot = slot, id = base_id, count = count }
end

-- Returns edits + message + value, or nil + message.
local function build_remove_edit(pf, data, target, slot)
	local e = find_entry(pf.root, target, pf.strings)
	assert(e, target .. " field not found in save")
	local arr = parse_array(data, e)
	local el = arr.elements[slot]
	assert(el, string.format("slot %d out of range (1..%d)", slot, arr.count))
	assert(not el.is_null, string.format("slot %d is already empty", slot))
	assert(arr.elem_tc == 48, "removing from a non-interface array is not supported")
	local oldlen = el.end_ - el.start + 1
	local edits = { { el.start, oldlen, "\0\0" } }
	local grow = 2 - oldlen
	local plen_new = e.plen + grow
	assert(#write7(plen_new) == e.pstart - e.plen_pos, "entry length width change on shrink")
	edits[#edits + 1] = { e.plen_pos, e.pstart - e.plen_pos, write7(plen_new) }
	local rtotal = pf.root.total + grow
	assert(#write7(rtotal) == pf.root.total_end - pf.root.total_start + 1, "root total width change on shrink")
	edits[#edits + 1] = { pf.root.total_start, pf.root.total_end - pf.root.total_start + 1, write7(rtotal) }
	return edits, string.format("%s slot %d: item removed", target, slot),
		{ target = target, slot = slot }
end

-- Returns edits + message + value, or nil + message. Sets StackCount of an
-- existing item; appends the field if the element lacks it.
local function build_stack_edit(pf, data, target, slot, count)
	local e = find_entry(pf.root, target, pf.strings)
	assert(e, target .. " field not found in save")
	local arr = parse_array(data, e)
	local el = arr.elements[slot]
	assert(el and not el.is_null, string.format("slot %d empty or out of range (must be an item slot)", slot))
	local stack = find_entry(el.obj, "StackCount", pf.strings)
	local edits = {}
	if stack then
		assert(stack.tc == 3 and stack.plen == 4, "StackCount has unexpected layout")
		edits[#edits + 1] = { stack.pstart, 4, u32(count) }
	else
		local name_id = ensure_map_key(pf, data, "StackCount")
		for _, ed in ipairs(finalize_map_edits(pf, data)) do edits[#edits + 1] = ed end
		local entry_bytes = string.char(3) .. write7(name_id) .. write7(4) .. u32(count)
		local ins = #entry_bytes
		local hdr = write7(el.obj.num + 1) .. write7(el.obj.total + ins)
		assert(#hdr == el.obj.total_end - el.obj.num_start + 1, "element header width change")
		edits[#edits + 1] = { el.obj.body_end, 0, entry_bytes }
		edits[#edits + 1] = { el.obj.num_start, #hdr, hdr }
		-- element sits inside the target entry -> grow its plen and root total
		local plen_new = e.plen + ins
		assert(#write7(plen_new) == e.pstart - e.plen_pos, "entry length width change")
		edits[#edits + 1] = { e.plen_pos, e.pstart - e.plen_pos, write7(plen_new) }
		local rtotal = pf.root.total + ins
		assert(#write7(rtotal) == pf.root.total_end - pf.root.total_start + 1, "root total width change")
		edits[#edits + 1] = { pf.root.total_start, pf.root.total_end - pf.root.total_start + 1, write7(rtotal) }
	end
	return edits, string.format("%s slot %d: StackCount -> %d", target, slot, count),
		{ target = target, slot = slot, set = true }
end

-- Verification for item edits: pf2/patched must carry the requested state.
local function verify_item(patched, pf2, v)
	local e = find_entry(pf2.root, v.target, pf2.strings)
	assert(e, v.target .. " missing after patch")
	local arr = parse_array(patched, e)
	local el = assert(arr.elements[v.slot], "slot missing after patch")
	local info = item_info(patched, pf2, el)
	if v.removed then
		assert(info == nil, "slot " .. v.slot .. " not emptied after patch")
	elseif v.set then
		assert(info, "slot " .. v.slot .. " empty after patch")
		local stack = find_entry(el.obj, "StackCount", pf2.strings)
		assert(stack and read_u32(patched, stack.pstart) == v.count, "StackCount wrong after patch")
	else
		assert(info, "slot " .. v.slot .. " empty after patch")
		assert(info.base_name == v.id, "item id wrong after patch: " .. tostring(info.base_name))
		assert(info.count == v.count, "stack count wrong after patch")
	end
end

cmds.give = function(arg)
	local file = assert(arg and arg[1], "usage: lua saveedit.lua give <file> <itemId> [count] [target] [outfile]")
	local id = assert(arg[2], "missing itemId")
	local count = tonumber(arg[3] or 1)
	assert(count and count > 0 and count == math.floor(count) and count < 2 ^ 31, "count must be a positive integer")
	local target = target_of(arg[4])
	local slot = tonumber(arg[5])
	assert(not slot or slot >= 1, "slot must be >= 1")
	run(file, arg[6],
		function(pf, data) return build_give_edit(pf, data, target, id, count, slot and math.floor(slot)) end,
		verify_item)
end

cmds.remove = function(arg)
	local file = assert(arg and arg[1], "usage: lua saveedit.lua remove <file> <slot> [target] [outfile]")
	local slot = tonumber((assert(arg[2], "missing slot")))
	assert(slot and slot >= 1 and slot == math.floor(slot), "slot must be a positive integer")
	local target = target_of(arg[3])
	run(file, arg[4],
		function(pf, data)
			local edits, msg, v = build_remove_edit(pf, data, target, slot)
			if v then v.removed = true end
			return edits, msg, v
		end,
		verify_item)
end

cmds.stack = function(arg)
	local file = assert(arg and arg[1], "usage: lua saveedit.lua stack <file> <slot> <count> [target] [outfile]")
	local slot = tonumber((assert(arg[2], "missing slot")))
	local count = tonumber((assert(arg[3], "missing count")))
	assert(slot and slot >= 1 and slot == math.floor(slot), "slot must be a positive integer")
	assert(count and count > 0 and count == math.floor(count) and count < 2 ^ 31, "count must be a positive integer")
	local target = target_of(arg[4])
	run(file, arg[5],
		function(pf, data)
			local edits, msg, v = build_stack_edit(pf, data, target, slot, count)
			if v then v.count = count end
			return edits, msg, v
		end,
		verify_item)
end

cmds.difficulty = function(arg)
	local file = assert(arg and arg[1], "usage: lua saveedit.lua difficulty <file> <softcore|normal> [outfile]")
	local want = arg[2]
	assert(want == "softcore" or want == "normal", "difficulty must be 'softcore' or 'normal'")
	local want_soft = want == "softcore"
	run(file, arg[3],
		function(pf, data) return build_difficulty_edit(pf, data, want_soft) end,
		function(p, pf2, v) verify_difficulty(p, pf2, v) end)
end

-- SAVEEDIT_TUI_CHILD: global set to true by cmds.tui while the TUI re-loads
-- this file, so the CLI dispatch below is skipped and the lib table returns.

cmds.tui = function(arg)
	SAVEEDIT_TUI_CHILD = true
	local ok, te = pcall(function()
		local dir = (arg and arg[0] or ""):match("^(.*[/\\]).*$")
		local tui_path = (dir and (dir .. "/saveedit_tui.lua") or "saveedit_tui.lua")
		assert(select(1, io.open(tui_path, "rb")), "saveedit_tui.lua missing next to saveedit.lua")
		io.open(tui_path, "rb"):close()
		local tui = dofile(tui_path)
		tui.main(arg and arg[2] or ".")
	end)
	if not ok then
		io.stderr:write("TUI failed: " .. tostring(te) .. "\n")
		os.exit(1)
	end
end

cmds.enable = function(arg)
	cmds.difficulty({ arg[1], "softcore", arg[2] })
end

cmds.money = function(arg)
	local file = assert(arg and arg[1], "usage: lua saveedit.lua money <file> <amount> [outfile]")
	local amount = tonumber((assert(arg[2], "missing amount")))
	assert(amount and amount >= 0 and amount == math.floor(amount), "amount must be a non-negative integer")
	run(file, arg[3],
		function(pf, data) return build_money_edit(pf, data, amount) end,
		function(p, pf2, v) verify_money(p, pf2, v) end)
end

local cmd = arg and arg[1]
local fn = cmd and cmds[cmd]
if not fn and not SAVEEDIT_TUI_CHILD then
	io.write("saveedit.lua — edit Romestead .char saves\n")
	io.write("usage:\n")
	io.write("  lua saveedit.lua dump       <file>\n")
	io.write("  lua saveedit.lua difficulty <file> <softcore|normal> [outfile]\n")
	io.write("  lua saveedit.lua money      <file> <amount> [outfile]\n")
	io.write("  lua saveedit.lua items      <file> [inventory|equipment|secondary]\n")
	io.write("  lua saveedit.lua give       <file> <itemId> [count] [target] [slot] [outfile]\n")
	io.write("  lua saveedit.lua remove     <file> <slot> [target] [outfile]\n")
	io.write("  lua saveedit.lua stack      <file> <slot> <count> [target] [outfile]\n")
	io.write("  lua saveedit.lua enable     <file> [outfile]   (alias: difficulty softcore)\n")
	io.write("  lua saveedit.lua tui        [folder]           (interactive save editor)\n")
	if cmd and not fn then print("unknown command: " .. cmd) end
elseif not SAVEEDIT_TUI_CHILD then
	cmds[cmd]({ select(2, table.unpack(arg)) })
end

return {
	read_file = read_file, write_file = write_file, parse_data = parse_data,
	apply_edits = apply_edits, find_entry = find_entry, parse_object = parse_object,
	read7 = read7, build_difficulty_edit = build_difficulty_edit,
	build_money_edit = build_money_edit, build_give_edit = build_give_edit,
	build_remove_edit = build_remove_edit, build_stack_edit = build_stack_edit,
	verify_difficulty = verify_difficulty, verify_money = verify_money,
	verify_item = verify_item, diff_value = diff_value,
	parse_array = parse_array, item_info = item_info, run = run,
	start_key_reader = start_key_reader, stop_key_reader = stop_key_reader,
}