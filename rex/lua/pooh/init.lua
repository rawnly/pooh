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
-- rest are `pooh pick` args. The UI is drawn on stderr, so the pick comes back on stdout.
local SCRIPT = [[
bin=$1; mode=$2; label_by=$3; client=$4; shift 4
pick=$("$bin" pick --json "$@") || exit 0
[ -n "$pick" ] || exit 0
# Several clients can be connected, so address the one that ran the action.
if [ -n "$client" ]; then set -- -C "$client"; else set --; fi
POOH_PICK=$pick POOH_MODE=$mode POOH_LABEL_BY=$label_by rex "$@" do -e '
  local pick = rex.json.decode(os.getenv("POOH_PICK"))
  local mode = os.getenv("POOH_MODE")
  local label = os.getenv("POOH_LABEL_BY") == "path" and pick.path or pick.label
  local layout = rex.layout.block({
    flavor = "com.superlogical.terminal.shell",
    options = { cwd = pick.path },
  })

  if mode == "window" then
    local win, err = rex.session.new_window({ focus = false, window_label = label, layout = layout })
    if err then error(err, 0) end
    -- A new window does not move the app client; it needs session.select to switch tabs.
    rex.client.queue("session.select", { session_id = rex.session_id, window_id = win.window_id })
    return
  end

  local session_id
  for _, s in ipairs(rex.call("session.list").sessions or {}) do
    if s.label == label then session_id = s.session_id break end
  end
  if not session_id then
    local created = rex.call("session.create", {
      label = label,
      initial_windows = { { window_label = label, layout = layout } },
    })
    session_id = created.session_id
  end
  rex.client.queue("session.select", { session_id = session_id })
' || exit 1
exec rex block close "$REX_BLOCK"
]]

--- The id when it belongs to an app client, else "" so the script falls back to the default client (e.g. `rex do` from a shell).
local function app_client(client_id)
	local r = client_id and rex.call("client.list")
	for _, c in ipairs(r and r.clients or {}) do
		if c.client_id == client_id and c.info and c.info.kind == "app" then
			return client_id
		end
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
---@field mode? "session"|"window"  open the pick in its own session (default) or a new tab of the current one
---@field session_label? "label"|"path"  name sessions/tabs by `project[:branch]` (default) or by directory
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

	rex.action({
		name = "pooh_pick",
		title = "pooh: open project",
		run = function(ctx)
			local cmd = {
				"sh", "-c", SCRIPT, "pooh",
				opts.bin or "pooh", mode, opts.session_label or "label", app_client(ctx.client_id),
			}
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
