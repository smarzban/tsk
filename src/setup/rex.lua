---------------------------------------------------------------------------
-- tsk board and quick-capture actions for Rex.
-- The board opens beside the focused pane, in that pane's directory, so tsk
-- picks the project scope from there. Quick capture opens in a centred
-- floating layer; saving or Esc closes it.
---------------------------------------------------------------------------

local TERMINAL = "com.superlogical.terminal"
local SHELL = "com.superlogical.terminal.shell"
local BOARD_LABEL = "tsk"
local CAPTURE_LABEL = "tsk-capture"

local function must(result, err)
  if err ~= nil then error(err, 0) end
  return result
end

-- First placed block in this session carrying `label`, or nil.
local function find_block(label)
  local list = must(rex.session.list_blocks{})
  for _, b in ipairs(list.blocks or {}) do
    if b.label == label and b.placement == "placed" then return b end
  end
  return nil
end

local function url_path(s)
  s = s:gsub("^file://[^/]*", "")
  return (s:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end))
end

-- Working directory of the focused pane: its foreground process, then the
-- directory the shell reported, then $HOME.
local function block_cwd(block_id)
  if block_id ~= nil then
    local p = rex.block.call(TERMINAL, "process", { block_id = block_id })
    if p and p.foreground and p.foreground.cwd then return p.foreground.cwd end
    local d = rex.block.call(TERMINAL, "pwd", { block_id = block_id })
    if d and d.pwd then return url_path(d.pwd) end
  end
  return os.getenv("HOME")
end

local function tsk_block(label, cwd, argv)
  return {
    block = {
      flavor = SHELL,
      label = label,
      options = { command = argv, cwd = cwd },
    },
  }
end

-- Approximate window size in cells from the focused block's size and its
-- share of the window, so the capture popup can be sized at about 80x15.
local function window_cells(block_id)
  local size = block_id and rex.block.call(TERMINAL, "size", { block_id = block_id })
  if not size or not size.columns then return nil end
  local view = rex.session.view{}
  local function find(node)
    if type(node) ~= "table" then return nil end
    if node.block_id == block_id and node.rect then return node.rect end
    for _, v in pairs(node) do
      local r = find(v)
      if r then return r end
    end
    return nil
  end
  local rect = view and find(view.windows) or { 0, 0, 1, 1 }
  return size.columns / rect[3], size.rows / rect[4]
end

rex.action{
  name = "tsk_board",
  title = "tsk: Open Board",
  description = "Focus the tsk board in this session, or open it beside the focused pane.",
  category = "tsk",
  keywords = "task board tsk",
  run = function(ctx, args)
    local existing = find_block(BOARD_LABEL)
    if existing then
      must(rex.session.focus_block{ block_id = existing.block_id })
      return { block_id = existing.block_id, opened = false }
    end
    local r = must(rex.session.new_split{
      anchor_id = ctx.block_id,
      direction = "horizontal",
      side = "after",
      focus = true,
      layout = tsk_block(BOARD_LABEL, block_cwd(ctx.block_id), { TSK_BIN }),
    })
    return { block_id = r.block_ids and r.block_ids[1], opened = true }
  end,
}

rex.action{
  name = "tsk_capture",
  title = "tsk: Quick Capture",
  description = "Capture a task in a floating popup. Saving or Esc closes it.",
  category = "tsk",
  keywords = "task capture add tsk",
  run = function(ctx, args)
    local existing = find_block(CAPTURE_LABEL)
    if existing then
      must(rex.session.focus_block{ block_id = existing.block_id })
      return { block_id = existing.block_id, opened = false }
    end
    local cols, rows = window_cells(ctx.block_id)
    local w = cols and math.min(0.9, 82 / cols) or 0.6
    local h = rows and math.min(0.9, 19 / rows) or 0.4
    local r = must(rex.session.new_layer{
      bounds = { (1 - w) / 2, (1 - h) / 2, w, h },
      focus = true,
      layout = tsk_block(CAPTURE_LABEL, block_cwd(ctx.block_id), { TSK_BIN, "capture" }),
    })
    return { block_id = r.block_ids and r.block_ids[1], opened = true }
  end,
}
