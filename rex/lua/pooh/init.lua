-- Rex plugin for pooh: pick a project or worktree in a floating pane running `pooh pick`
-- (default) or in Rex's command palette, then open it in its own Rex session (default)
-- or in a new tab of the current session.
-- Registers the `pooh_pick` action; bind it yourself or pass `key`.
--
-- Works with Amber (the module lives in rex/lua, setup(opts) is the entry point)
-- and without it (see README: put rex/lua on package.path and call setup).
local M = {}

local TERM = "com.superlogical.terminal"
local SHELL = TERM .. ".shell"

-- Opens a pick (`pooh pick --json` shape) in its session or tab. Kept as source so the float's
-- `rex do -e` script and the palette action run the same code. Args: pick, mode, by_path, session_id.
local OPEN = [[
  local pick, mode, by_path, session_id = ...
  -- Rex stores labels trimmed, without control characters, at most 128 characters.
  -- Normalize the same way before comparing or a label never matches its own session.
  local function clean(text, keep_tail)
    text = text:gsub("[%z\1-\31\127]", ""):gsub("^%s+", ""):gsub("%s+$", "")
    local chars = {}
    for c in text:gmatch("[%z\1-\127\194-\244][\128-\191]*") do chars[#chars + 1] = c end
    if #chars <= 128 then return text end
    local from = keep_tail and (#chars - 127) or 1
    return (table.concat(chars, "", from, from + 127):gsub("^%s+", ""):gsub("%s+$", ""))
  end
  -- Paths are distinguished by their tail, names by their head.
  local label = clean(by_path and pick.path or pick.label, by_path)
  -- Blocks carry the label, windows do not, so tab reuse goes through it.
  local block_label = clean("pooh:" .. label, by_path)
  local layout = rex.layout.block({
    flavor = "com.superlogical.terminal.shell",
    label = block_label,
    options = { cwd = pick.path },
  })

  if mode == "window" then
    local listed, list_err = rex.session.list_blocks({ session_id = session_id })
    if not listed then error(list_err, 0) end
    for _, b in ipairs(listed.blocks or {}) do
      if b.label == block_label and b.placement == "placed" then
        rex.client.queue("session.select", { session_id = session_id, window_id = b.window_id })
        return
      end
    end
    local win, err = rex.session.new_window({ session_id = session_id, focus = false, window_label = label, layout = layout })
    if err then error(err, 0) end
    -- A new window does not move the app client; it needs session.select to switch tabs.
    rex.client.queue("session.select", { session_id = session_id, window_id = win.window_id })
    return
  end

  local target
  local listed, list_err = rex.call("session.list")
  if not listed then error(list_err, 0) end
  for _, s in ipairs(listed.sessions or {}) do
    if s.label == label then target = s.session_id break end
  end
  if not target then
    local created, err = rex.call("session.create", {
      label = label,
      initial_windows = { { window_label = label, layout = layout } },
    })
    if not created then error(err, 0) end
    target = created.session_id
  end
  rex.client.queue("session.select", { session_id = target })
]]

-- Runs inside the float. $1 pooh, $2 mode, $3 session label source, $4 app client id (may be empty),
-- $5 the OPEN chunk, rest are `pooh pick` args. The UI is drawn on the tty, so the pick comes back on stdout.
local SCRIPT = [[
bin=$1; mode=$2; label_by=$3; client=$4; open=$5; shift 5
pick=$("$bin" pick --json "$@"); rc=$?
# 1 = cancelled. Anything else is a real error: keep the float open so it can be read.
[ "$rc" -eq 1 ] && exit 0
if [ "$rc" -ne 0 ]; then
  printf "%s\n\npress enter to close " "$pick" >&2
  read -r _
  exit 1
fi
[ -n "$pick" ] || exit 0
# Several clients can be connected, so address the one that ran the action.
if [ -n "$client" ]; then set -- -C "$client"; else set --; fi
POOH_PICK=$pick POOH_MODE=$mode POOH_LABEL_BY=$label_by POOH_OPEN=$open rex "$@" do -e '
  local open = assert(loadstring(os.getenv("POOH_OPEN"), "pooh.open"))
  open(rex.json.decode(os.getenv("POOH_PICK")), os.getenv("POOH_MODE"),
    os.getenv("POOH_LABEL_BY") == "path", rex.session_id)
' || exit 1
exec rex block close "$REX_BLOCK"
]]

--- The app client to drive: the caller's if it is one, else the single app client attached to
--- this session (e.g. `rex do pooh_pick` from a shell). "" lets `rex do` pick the default client.
local function app_client(client_id, session_id)
	local r = client_id and rex.call("client.list")
	for _, c in ipairs(r and r.clients or {}) do
		if c.client_id == client_id and c.info and c.info.kind == "app" then
			return client_id
		end
	end
	local attached = rex.call("client.list", { session_id = session_id, kinds = { "app" } })
	local clients = attached and attached.clients or {}
	if #clients == 1 then
		return clients[1].client_id
	end
	return ""
end

--- Working directory of a block: the reported `file://` URI, else the foreground process cwd, else $HOME.
local function cwd_of(block_id)
	local r = rex.block.call(TERM, "pwd", { block_id = block_id })
	local path = r and r.pwd and r.pwd:match("^file://[^/]*(/.*)$")
	if path then
		return (path:gsub("%%(%x%x)", function(hex)
			return string.char(tonumber(hex, 16))
		end))
	end
	local proc = rex.block.call(TERM, "process", { block_id = block_id })
	return (proc and proc.foreground and proc.foreground.cwd) or os.getenv("HOME")
end

---@class PoohRexOptions
---@field bin? string          pooh executable (default "pooh")
---@field mode? "session"|"window"  open the pick in its own session (default) or a tab of the current one; both reuse the one a previous pick created
---@field session_label? "label"|"path"  name sessions/tabs by `project[:branch]` (default) or by directory
---@field full_screen? boolean  pass `--full-screen` so the picker fills the float (default true)
---@field args? string[]       extra `pooh pick` flags, e.g. { "--tag", "work", "--no-worktrees" }
---@field bounds? number[]     float geometry `{ x, y, w, h }` as window fractions (default centered 60% x 60%)
---@field picker? "float"|"palette"  `pooh pick` in a floating pane (default) or Rex's command palette
---@field key? string          optional key binding for the action, e.g. "ctrl+a>p"

local open_pick = assert(loadstring(OPEN, "pooh.open"))

--- `~/x` for paths under $HOME, for display.
local function tilde(path)
	local home = os.getenv("HOME")
	if home and path:sub(1, #home + 1) == home .. "/" then
		return "~" .. path:sub(#home + 1)
	end
	return path
end

local resolved_bins = {}

--- Absolute path of `bin`. The server's PATH is launchd's (/usr/bin:/bin:...), not the user's,
--- so a bare name is looked up through the login shell once. The float never needs this: its
--- shell block already has the user's PATH.
local function find_bin(bin)
	if bin:find("/", 1, true) or not bin:match("^[%w._+-]+$") then
		return bin
	end
	if not resolved_bins[bin] then
		local r = rex.exec({ os.getenv("SHELL") or "/bin/sh", "-l", "-c", "command -v " .. bin }, { timeout = 5 })
		-- Login shells may print banners first; the path is the last line.
		local path = r and r.ok and r.stdout:match("(/[^\n]+)\n?$")
		resolved_bins[bin] = path or bin
	end
	return resolved_bins[bin]
end

--- The palette picker: list the entries `pooh pick` would show, ask with rex.ui.pick, open.
local function pick_palette(ctx, opts, mode, label_by)
	local bin = find_bin(opts.bin or "pooh")
	local cmd = { bin, "pick", "--list", "--json" }
	for _, arg in ipairs(opts.args or {}) do
		cmd[#cmd + 1] = arg
	end
	local cwd = cwd_of(ctx.block_id)
	local r, err = rex.exec(cmd, { cwd = cwd, timeout = 15 })
	if not r then
		error("pooh: " .. tostring(err) .. " (" .. bin .. "; set `bin` to its absolute path)", 0)
	end
	local entries = rex.json.decode(r.stdout)
	if not r.ok or type(entries) ~= "table" then
		local msg = type(entries) == "table" and entries.error or (r.stderr ~= "" and r.stderr or r.stdout)
		error("pooh: " .. tostring(msg), 0)
	end
	if #entries == 0 then
		error("pooh: no projects to pick from (add one with `pooh add`, or loosen `args`)", 0)
	end

	local items = {}
	-- rex.ui.pick takes at most 2000 items; entries are ranked, so the tail goes.
	for i = 1, math.min(#entries, 2000) do
		local e = entries[i]
		items[i] = {
			-- Indices, not labels: ids must be unique and a label is only unique per project.
			id = tostring(i),
			title = e.label,
			subtitle = tilde(e.path),
			icon = e.worktree and "branch" or "folder",
			detail = (e.tags and #e.tags > 0) and table.concat(e.tags, ", ") or nil,
		}
	end
	local choice, why, detail = rex.ui.pick({
		title = "Open project",
		placeholder = "Filter projects and worktrees",
		confirm = mode == "window" and "Open in tab" or "Open session",
		items = items,
	})
	if choice == nil then
		if why == "cancelled" then
			return
		end
		error("pooh: picker " .. tostring(why) .. (detail and (": " .. detail) or ""), 0)
	end
	local pick = entries[tonumber(choice)]

	-- Record the jump like `pooh pick` does (frecency + zoxide). Best effort: never block the open.
	local visited = rex.exec(
		{ bin, "resolve", "--json", "-p", pick.project, "--", pick.label },
		{ cwd = cwd, timeout = 5, stdout = false }
	)
	if not (visited and visited.ok) then
		rex.log("warn", "pooh: could not record the visit to " .. pick.label)
	end
	open_pick(pick, mode, label_by == "path", ctx.session_id)
end

---@param opts? PoohRexOptions
function M.setup(opts)
	opts = opts or {}
	local mode = opts.mode or "session"
	if mode ~= "session" and mode ~= "window" then
		error("pooh: mode must be \"session\" or \"window\", got " .. tostring(mode), 0)
	end
	local label_by = opts.session_label or "label"
	if label_by ~= "label" and label_by ~= "path" then
		error("pooh: session_label must be \"label\" or \"path\", got " .. tostring(label_by), 0)
	end
	local picker = opts.picker or "float"
	if picker ~= "float" and picker ~= "palette" then
		error("pooh: picker must be \"float\" or \"palette\", got " .. tostring(picker), 0)
	end
	for _, key in ipairs({ "args", "bounds" }) do
		if opts[key] ~= nil and type(opts[key]) ~= "table" then
			error("pooh: " .. key .. " must be a list, got " .. type(opts[key]), 0)
		end
	end

	rex.action({
		name = "pooh_pick",
		title = "pooh: open project",
		run = function(ctx)
			if picker == "palette" then
				return pick_palette(ctx, opts, mode, label_by)
			end
			local cmd = {
				"sh", "-c", SCRIPT, "pooh",
				opts.bin or "pooh", mode, label_by, app_client(ctx.client_id, ctx.session_id), OPEN,
			}
			if opts.full_screen ~= false then
				cmd[#cmd + 1] = "--full-screen"
			end
			for _, arg in ipairs(opts.args or {}) do
				cmd[#cmd + 1] = arg
			end

			local result, err = rex.session.new_layer({
				bounds = opts.bounds or { 0.2, 0.2, 0.6, 0.6 },
				focus = true,
				layout = rex.layout.block({
					flavor = SHELL,
					options = { cwd = cwd_of(ctx.block_id), command = cmd },
				}),
			})
			if err then
				error(err, 0)
			end
			return result
		end,
	})

	if opts.key then
		rex.bind(opts.key, "pooh_pick")
	end
end

return M
