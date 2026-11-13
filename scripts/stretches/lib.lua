-- The stretch library, prepended to every stretch by scripts/stretch.sh: a step machine that waits
-- on game state, the GM and quest helpers, and the verdict. A stretch file defines STRETCH:
--
--   STRETCH = {
--     setup = { ".quest add 12641", ... },   -- GM lines or functions, one per second, before step 1
--     steps = {
--       { "talk to the Lich King", start = function() ... end, done = function() return ... end,
--         timeout = 30 },
--       ...
--     },
--   }
--
-- A step's `start` runs once, then `done` is polled every 0.25 s until it answers true (the next
-- step) or the step's timeout (default 60 s) runs out (the stretch fails, naming the step).
-- `poke`, when given, runs at the start and again every `every` seconds (default 4) until done:
-- a right-click repeated as a player would, when the NPC was not in view yet.
-- `done` may answer false, "why" to fail at once. STRETCH_BEAT (set by --watch) holds each step
-- that many seconds before its start, so a watcher can follow.

local S = {}
Stretch = S

local now = GetTime

function S.log(fmt, ...)
  ProbeLog("stretch: " .. string.format(fmt, ...))
end

-- A GM or chat line, spoken as SAY: dot-commands go out this way.
function S.gm(line)
  S.log("gm %s", line)
  SendChatMessage(line, "SAY")
end

-- The quest log row of `id`, or nil.
function S.questIndex(id)
  for i = 1, GetNumQuestLogEntries() do
    local _, _, _, _, header, _, _, _, questID = GetQuestLogTitle(i)
    if not header and questID == id then
      return i
    end
  end
end

-- Whether `id` is in the log, and whether every objective is done.
function S.questState(id)
  local i = S.questIndex(id)
  if not i then
    return false, false
  end
  local _, _, _, _, _, _, complete = GetQuestLogTitle(i)
  if complete == 1 then
    return true, true
  end
  local n = GetNumQuestLeaderBoards(i)
  if n == 0 then
    return true, false
  end
  for j = 1, n do
    local _, _, finished = GetQuestLogLeaderBoard(j, i)
    if not finished then
      return true, false
    end
  end
  return true, true
end

-- Objectives done of `id`'s log row, as "a/b".
function S.objectives(id)
  local i = S.questIndex(id)
  if not i then
    return "not in log"
  end
  local n, d = GetNumQuestLeaderBoards(i), 0
  for j = 1, n do
    local _, _, finished = GetQuestLogLeaderBoard(j, i)
    if finished then
      d = d + 1
    end
  end
  return d .. "/" .. n
end

-- Values of any type, comma-joined for a log line.
function S.list(...)
  local t = {}
  for i = 1, select("#", ...) do
    t[i] = tostring((select(i, ...)))
  end
  return table.concat(t, ", ")
end

-- The quest dialogs, answered as a player would, while `S.want` names what to do:
-- { accept = <id>, turnin = <id>, reward = <choice index or 1> }.
S.want = {}
local dialogs = CreateFrame("Frame")
for _, e in ipairs({ "GOSSIP_SHOW", "QUEST_GREETING", "QUEST_DETAIL", "QUEST_PROGRESS",
  "QUEST_COMPLETE", "QUEST_ACCEPTED", "QUEST_FINISHED" }) do
  pcall(dialogs.RegisterEvent, dialogs, e)
end
dialogs:SetScript("OnEvent", function(_, e)
  S.log("event %s", e)
  -- A turn-in goes first: the follow-up quest it unlocks is offered after the reward.
  if e == "GOSSIP_SHOW" then
    S.log("gossip: available {%s} active {%s}", S.list(GetGossipAvailableQuests()),
      S.list(GetGossipActiveQuests()))
    if S.want.turnin and S.questIndex(S.want.turnin) and GetNumGossipActiveQuests() > 0 then
      SelectGossipActiveQuest(1)
    elseif S.want.accept and GetNumGossipAvailableQuests() > 0 then
      SelectGossipAvailableQuest(1)
    end
  elseif e == "QUEST_GREETING" then
    if S.want.turnin and S.questIndex(S.want.turnin) and GetNumActiveQuests() > 0 then
      SelectActiveQuest(1)
    elseif S.want.accept and GetNumAvailableQuests() > 0 then
      SelectAvailableQuest(1)
    end
  elseif e == "QUEST_DETAIL" and S.want.accept then
    AcceptQuest()
  elseif e == "QUEST_PROGRESS" and S.want.turnin then
    CompleteQuest()
  elseif e == "QUEST_COMPLETE" and S.want.turnin then
    GetQuestReward(S.want.reward or 1)
  end
end)

-- Each unit's completed casts, counted (`UNIT_SPELLCAST_SUCCEEDED`), with the last spell's name.
S.casts = {}
local casts = CreateFrame("Frame")
casts:RegisterEvent("UNIT_SPELLCAST_SUCCEEDED")
casts:SetScript("OnEvent", function(_, _, unit, spell)
  local c = S.casts[unit] or { n = 0 }
  c.n, c.last = c.n + 1, spell
  S.casts[unit] = c
  S.log("cast %s by %s", tostring(spell), tostring(unit))
end)

-- Each quest turned in, by title, as its system line says (`ERR_QUEST_COMPLETE_S`): a quest with
-- nothing to do is offered straight to its reward and never enters the log.
S.completed = {}
local completeLine = "^" .. ERR_QUEST_COMPLETE_S:gsub("%.", "%%."):gsub("%%s", "(.+)") .. "$"
local system = CreateFrame("Frame")
system:RegisterEvent("CHAT_MSG_SYSTEM")
system:SetScript("OnEvent", function(_, _, text)
  local title = text and text:match(completeLine)
  if title then
    S.completed[title] = true
    S.log("completed %s", title)
  end
end)

function S.castCount(unit)
  return (S.casts[unit] or { n = 0 }).n
end

-- Whether input moves the driven body now (ours, unrooted, off any server path).
function S.steerable()
  local _, _, _, _, _, s = ProbePos()
  return s
end

-- Every `UseAction` a button press reaches, logged with the slot's kind and id.
hooksecurefunc("UseAction", function(slot)
  S.log("UseAction(%s) -> %s", tostring(slot), table.concat({ tostring(GetActionInfo(slot)) }, " "))
end)

-- The action bar key `n` (1-12), as its binding runs it (`Bindings.xml` ACTIONBUTTONn): the down and
-- up halves, which route to the bonus bar's button while a stance, possess or vehicle page is up.
function S.pressKey(n)
  ActionButtonDown(n)
  ActionButtonUp(n)
end

-- Distance in yards between two WoW points.
function S.dist(x1, y1, z1, x2, y2, z2)
  return math.sqrt((x1 - x2) ^ 2 + (y1 - y2) ^ 2 + (z1 - z2) ^ 2)
end

-- Whether the driven body is within `r` yards of the point.
function S.near(x, y, z, r)
  local px, py, pz = ProbePos()
  return px and S.dist(px, py, pz, x, y, z) <= r
end

-- The driven body's position, for a step's status line.
function S.where()
  local x, y, z = ProbePos()
  return x and string.format("(%.1f, %.1f, %.1f)", x, y, z) or "unknown"
end

-- Whether a unit or object of `entry` is within `r` yards.
function S.nearEntry(entry, r)
  local d = ProbeNearest(entry)
  return d and d <= r
end

-- Steering as a player does, with the movement keys: turn toward the point, hold forward once
-- facing it, and ascend or descend toward its height when flying. Call every tick; answers true on
-- arrival, `r` yards across and `vr` (default 25) up or down, having let go of every key: a roof
-- or the ground may hold a flyer off the exact height. Real keys (W, A, D, Space, X through their
-- default bindings): the movement functions refuse any caller but a binding body.
local held = {}
local KEYS = { fwd = "W", left = "A", right = "D", up = "Space", down = "X" }
local function hold(key, on)
  if (held[key] or false) == on then
    return
  end
  held[key] = on
  if on then
    ProbeKeyDown(KEYS[key])
  else
    ProbeKeyUp(KEYS[key])
  end
end

function S.release()
  for _, k in ipairs({ "fwd", "left", "right", "up", "down" }) do
    hold(k, false)
  end
end

function S.steer(x, y, z, r, fly, vr)
  local px, py, pz, _, facing = ProbePos()
  if not px then
    return false
  end
  if S.dist(px, py, 0, x, y, 0) <= r and math.abs(pz - z) <= (vr or 25) then
    S.release()
    return true
  end
  -- WoW facing: radians from +x toward +y.
  local diff = math.atan2(y - py, x - px) - facing
  diff = (diff + math.pi) % (2 * math.pi) - math.pi
  hold("left", diff > 0.08)
  hold("right", diff < -0.08)
  hold("fwd", math.abs(diff) < 0.5)
  if fly then
    -- One vertical key at a time, the other let go a tick before: a same-frame swap is one resync
    -- that starts descent over a standing ascent, and with both bits the reference climbs
    -- (`0x987709` tests ASCENDING alone).
    local up, down = pz < z - 3, pz > z + 3
    if up and held.down or down and held.up then
      hold("up", false)
      hold("down", false)
    else
      hold("up", up)
      hold("down", down)
    end
  end
  return false
end

-- A walk through `points` ({x, y, z} each) on the movement keys, each reached within `r` yards
-- (default 2): call every tick; answers true at the last. The legs keep the walk off walls.
function S.route(points, r)
  local i = points.at or 1
  while points[i] do
    local p = points[i]
    if not S.steer(p[1], p[2], p[3], r or 2, false, 6) then
      points.at = i
      return false
    end
    i = i + 1
  end
  points.at = nil
  return true
end

-- The step machine.
local f = CreateFrame("Frame")
local acc, setupAt, stepAt, idx, started, finished, pokedAt = 0, 0, 0, 0, false, false, 0
local t0 = now()

local function fail(why)
  finished = true
  local step = STRETCH.steps[idx]
  S.log("FAIL at step %d (%s): %s", idx, step and step[1] or "setup", why)
  ProbeShot("fail")
  ProbeVerdict(false, why)
end

local function tick(el)
  if finished then
    return
  end
  acc = acc + el
  if acc < 0.25 then
    return
  end
  acc = 0
  local t = now() - t0
  -- Setup: one GM line a second.
  if setupAt < #(STRETCH.setup or {}) then
    if t >= setupAt + 1 then
      setupAt = setupAt + 1
      local line = STRETCH.setup[setupAt]
      if type(line) == "function" then
        local ok, err = pcall(line)
        if not ok then
          return fail("setup: " .. tostring(err))
        end
      else
        S.gm(line)
      end
    end
    return
  end
  if idx == 0 then
    -- The last setup line's settle.
    if t < setupAt + 4 then
      return
    end
    idx, started = 1, false
  end
  local step = STRETCH.steps[idx]
  if not step then
    finished = true
    S.log("PASS in %.0f s", t)
    ProbeStep("pass")
    ProbeVerdict(true, (STRETCH.name or "stretch") .. " passed")
    return
  end
  if not started then
    if stepAt == 0 then
      stepAt = now()
      ProbeStep(step[1])
    end
    if now() - stepAt < (STRETCH_BEAT or 0) then
      return
    end
    started = true
    stepAt = now()
    if step.start then
      local ok, err = pcall(step.start)
      if not ok then
        return fail("start: " .. tostring(err))
      end
    end
    pokedAt = 0
  end
  if step.poke and now() - pokedAt >= (step.every or 4) then
    pokedAt = now()
    local ok, err = pcall(step.poke)
    if not ok then
      return fail("poke: " .. tostring(err))
    end
  end
  local ok, done, why = pcall(step.done or function() return true end)
  if not ok then
    return fail("done: " .. tostring(done))
  end
  if done then
    S.log("step %d done in %.1f s: %s", idx, now() - stepAt, step[1])
    idx, started, stepAt = idx + 1, false, 0
  elseif done == false and why then
    return fail(why)
  elseif now() - stepAt > (step.timeout or 60) then
    return fail("timed out after " .. (step.timeout or 60) .. " s" .. (step.status and (": " .. tostring(step.status())) or ""))
  end
end

f:SetScript("OnUpdate", function(_, el)
  local ok, err = pcall(tick, el)
  if not ok then
    fail("lua: " .. tostring(err))
  end
end)
