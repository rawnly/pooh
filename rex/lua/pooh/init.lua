-- Rex plugin for pooh: pick a project or worktree in a floating pane, then open it
-- in its own Rex session (default) or in a new tab of the current session.
-- Registers the `pooh_pick` action; bind it yourself or pass `key`.
--
-- Works with Amber (the module lives in rex/lua, setup(opts) is the entry point)
-- and without it (see README: put rex/lua on package.path and call setup).
local M = {}

local TERM = "com.superlogical.terminal"
local SHELL = TERM .. ".shell"

-- Runs inside the float. $1 pooh, $2 mode, $3 session label source, $4 app client id (may be empty),
-- rest are `pooh pick` args. The UI is drawn on the tty, so the pick comes back on stdout.
local SCRIPT = [[
bin=$1; mode=$2; label_by=$3; client=$4; shift 4
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
POOH_PICK=$pick POOH_MODE=$mode POOH_LABEL_BY=$label_by rex "$@" do -e '
  local pick = rex.json.decode(os.getenv("POOH_PICK"))
  local mode = os.getenv("POOH_MODE")
  local by_path = os.getenv("POOH_LABEL_BY") == "path"
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
    local listed, list_err = rex.session.list_blocks()
    if not listed then error(list_err, 0) end
    for _, b in ipairs(listed.blocks or {}) do
      if b.label == block_label and b.placement == "placed" then
        rex.client.queue("session.select", { session_id = rex.session_id, window_id = b.window_id })
        return
      end
    end
    local win, err = rex.session.new_window({ focus = false, window_label = label, layout = layout })
    if err then error(err, 0) end
    -- A new window does not move the app client; it needs session.select to switch tabs.
    rex.client.queue("session.select", { session_id = rex.session_id, window_id = win.window_id })
    return
  end

  local session_id
  local listed, list_err = rex.call("session.list")
  if not listed then error(list_err, 0) end
  for _, s in ipairs(listed.sessions or {}) do
    if s.label == label then session_id = s.session_id break end
  end
  if not session_id then
    local created, err = rex.call("session.create", {
      label = label,
      initial_windows = { { window_label = label, layout = layout } },
    })
    if not created then error(err, 0) end
    session_id = created.session_id
  end
  rex.client.queue("session.select", { session_id = session_id })
' || exit 1
exec rex block close "$REX_BLOCK"
]]

--- The app client to drive: the caller's if it is one, else the single app client attached to
--- this session (e.g. `rex do pooh_pick` from a shell). "" lets `rex do` pick the default client.
local function app_client(client_id)
	local r = client_id and rex.call("client.list")
	for _, c in ipairs(r and r.clients or {}) do
		if c.client_id == client_id and c.info and c.info.kind == "app" then
			return client_id
		end
	end
	local attached = rex.call("client.list", { session_id = rex.session_id, kinds = { "app" } })
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
---@field key? string          optional key binding for the action, e.g. "ctrl+a>p"

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
	for _, key in ipairs({ "args", "bounds" }) do
		if opts[key] ~= nil and type(opts[key]) ~= "table" then
			error("pooh: " .. key .. " must be a list, got " .. type(opts[key]), 0)
		end
	end

	rex.action({
		name = "pooh_pick",
		title = "pooh: open project",
		run = function(ctx)
			local cmd = {
				"sh", "-c", SCRIPT, "pooh",
				opts.bin or "pooh", mode, label_by, app_client(ctx.client_id),
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
