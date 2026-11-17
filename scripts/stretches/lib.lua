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
-- { accept = <id>, turnin = <id>, reward = <choice index or 1> }. Each answer waits STRETCH_DIALOG
-- seconds (default 5) with its window up, so a watcher can read it; the step machine holds still
-- meanwhile (no click, no timeout clock).
S.want = {}
local DIALOG_HOLD = STRETCH_DIALOG or 5
local held
local function answer(what, fn)
  if DIALOG_HOLD <= 0 then
    return fn()
  end
  S.log("holding the dialog %d s, then %s", DIALOG_HOLD, what)
  held = { at = now() + DIALOG_HOLD, fn = fn }
end

-- Whether a dialog is up and waiting for its answer.
function S.dialogHeld()
  return held ~= nil
end

local dialogs = CreateFrame("Frame")
for _, e in ipairs({ "GOSSIP_SHOW", "QUEST_GREETING", "QUEST_DETAIL", "QUEST_PROGRESS",
  "QUEST_COMPLETE", "QUEST_ACCEPTED", "QUEST_FINISHED" }) do
  pcall(dialogs.RegisterEvent, dialogs, e)
end
dialogs:SetScript("OnUpdate", function()
  if held and now() >= held.at then
    local fn = held.fn
    held = nil
    fn()
  end
end)
dialogs:SetScript("OnEvent", function(_, e)
  S.log("event %s", e)
  -- A turn-in goes first: the follow-up quest it unlocks is offered after the reward.
  if e == "GOSSIP_SHOW" then
    S.log("gossip: available {%s} active {%s}", S.list(GetGossipAvailableQuests()),
      S.list(GetGossipActiveQuests()))
    if S.want.turnin and S.questIndex(S.want.turnin) and GetNumGossipActiveQuests() > 0 then
      answer("pick the quest to turn in", function()
        SelectGossipActiveQuest(1)
      end)
    elseif S.want.accept and GetNumGossipAvailableQuests() > 0 then
      answer("pick the quest to accept", function()
        SelectGossipAvailableQuest(1)
      end)
    end
  elseif e == "QUEST_GREETING" then
    if S.want.turnin and S.questIndex(S.want.turnin) and GetNumActiveQuests() > 0 then
      answer("pick the quest to turn in", function()
        SelectActiveQuest(1)
      end)
    elseif S.want.accept and GetNumAvailableQuests() > 0 then
      answer("pick the quest to accept", function()
        SelectAvailableQuest(1)
      end)
    end
  elseif e == "QUEST_DETAIL" and S.want.accept then
    answer("accept", AcceptQuest)
  elseif e == "QUEST_PROGRESS" and S.want.turnin then
    answer("continue", CompleteQuest)
  elseif e == "QUEST_COMPLETE" and S.want.turnin then
    local choice = S.want.reward or 1
    answer("take the reward", function()
      GetQuestReward(choice)
    end)
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
-- roughly facing it, and ascend or descend toward its height when flying. Call every tick; answers
-- true on arrival, `r` yards across and `vr` (default 25) up or down, having let go of every key: a
-- roof or the ground may hold a flyer off the exact height. The keys themselves move every frame
-- toward the goal the last call set, so a turn stops on its heading instead of a tick past it.
-- Real keys (W, A, D, Space, X through their default bindings): the movement functions refuse any
-- caller but a binding body.
local held = {}
local KEYS = { fwd = "W", left = "A", right = "D", up = "Space", down = "X", sleft = "Q", sright = "E" }
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

local goal
function S.release()
  goal = nil
  for _, k in ipairs({ "fwd", "left", "right", "up", "down", "sleft", "sright" }) do
    hold(k, false)
  end
end

function S.steer(x, y, z, r, fly, vr)
  local px, py, pz = ProbePos()
  if not px then
    return false
  end
  if S.dist(px, py, 0, x, y, 0) <= r and math.abs(pz - z) <= (vr or 25) then
    S.release()
    return true
  end
  goal = { x = x, y = y, z = z, fly = fly, at = now() }
  return false
end

-- A turn starts past TURN_START radians off the heading and stops inside TURN_STOP.
local TURN_START, TURN_STOP = 0.12, 0.03
-- The loading screen lets go of every held key, as the reference's held key comes back released
-- on world enter, and a press under it is swallowed: a key still held here is pressed again every
-- REPRESS seconds, as a player presses again once the screen drops. A press of a key already down
-- changes nothing.
local REPRESS, pressedAt = 0.5, 0
local steerer = CreateFrame("Frame")
steerer:SetScript("OnUpdate", function()
  S.crumb()
  if now() - pressedAt >= REPRESS then
    pressedAt = now()
    for key, on in pairs(held) do
      if on then
        ProbeKeyDown(KEYS[key])
      end
    end
  end
  -- A goal no step renews is dropped: the step that set it has moved on.
  if goal and now() - goal.at > 0.6 then
    S.release()
  end
  if not goal then
    return
  end
  local px, py, pz, _, facing = ProbePos()
  if not px then
    return
  end
  -- WoW facing: radians from +x toward +y.
  local diff = math.atan2(goal.y - py, goal.x - px) - facing
  diff = (diff + math.pi) % (2 * math.pi) - math.pi
  local turning = held.left or held.right
  local off = math.abs(diff) > (turning and TURN_STOP or TURN_START)
  hold("left", off and diff > 0)
  hold("right", off and diff < 0)
  hold("fwd", math.abs(diff) < 0.5)
  if not goal.fly then
    S.unstick(px, py)
  else
    -- One vertical key at a time, the other let go a frame before: a same-frame swap is one
    -- resync that starts descent over a standing ascent, and with both bits the reference climbs
    -- (`0x987709` tests ASCENDING alone).
    local up, down = pz < goal.z - 3, pz > goal.z + 3
    if up and held.down or down and held.up then
      hold("up", false)
      hold("down", false)
    else
      hold("up", up)
      hold("down", down)
    end
  end
end)

-- A breadcrumb trail of where the body went, one crumb every TRAIL_STEP yards from S.trailStart on:
-- ground it got over, for S.retrace to walk back by.
local TRAIL_STEP = 4
function S.trailStart()
  S.trail = {}
end

function S.crumb()
  local t = S.trail
  if not t or t.back then
    return
  end
  local px, py, pz = ProbePos()
  local last = t[#t]
  if px and (not last or S.dist(px, py, 0, last[1], last[2], 0) > TRAIL_STEP) then
    t[#t + 1] = { px, py, pz }
  end
end

-- Back along the trail to its first crumb: each leg goes to the crumb before the earliest one within
-- a few yards, so the walk skips any loop the trail made back past itself. Answers true there.
local SHORTCUT = 8
function S.retrace()
  local t = S.trail
  if not t or #t == 0 then
    return true
  end
  t.back = true
  local px, py = ProbePos()
  if not px then
    return false
  end
  local j, best, bd = nil, 1, math.huge
  for i, c in ipairs(t) do
    local d = S.dist(px, py, 0, c[1], c[2], 0)
    if d < SHORTCUT and not j then
      j = i
    end
    if d < bd then
      best, bd = i, d
    end
  end
  j = j or best
  if j == 1 and bd < 3 then
    S.release()
    return true
  end
  local c = t[math.max(j - 1, 1)]
  S.steer(c[1], c[2], c[3], 1.5, false, 10)
  return false
end

-- On foot, a way round what the walk caught on, as a player gets past a fence or a step: after a
-- second and a half of forward without a yard of headway a jump, and if that fails a second of
-- strafing, to either side in turn, before going on.
local stuck = { x = 0, y = 0, t = 0, tries = 0, side = "sright" }
function S.unstick(px, py)
  if held.up then
    hold("up", false)
  end
  if held.sleft or held.sright then
    if now() - stuck.t > 1 then
      hold("sleft", false)
      hold("sright", false)
      stuck.t = now()
    end
    return
  end
  if not held.fwd or S.dist(px, py, 0, stuck.x, stuck.y, 0) > 1 then
    stuck.x, stuck.y, stuck.t, stuck.tries = px, py, now(), 0
  elseif now() - stuck.t > 1.5 then
    stuck.tries, stuck.t = stuck.tries + 1, now()
    if stuck.tries % 2 == 1 then
      hold("up", true)
    else
      stuck.side = stuck.side == "sright" and "sleft" or "sright"
      hold(stuck.side, true)
    end
  end
end

-- The loot window, emptied as it opens, as autoloot does.
local loot = CreateFrame("Frame")
loot:RegisterEvent("LOOT_OPENED")
loot:SetScript("OnEvent", function()
  for i = GetNumLootItems(), 1, -1 do
    LootSlot(i)
  end
end)

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

-- A step walking `points` (S.route).
function S.walk(text, points, timeout)
  return {
    text,
    done = function()
      return S.route(points)
    end,
    status = function()
      return "at " .. S.where() .. ", leg " .. tostring(points.at)
    end,
    timeout = timeout or 60,
  }
end

-- A step talking to `npc` with S.want set to `want`, done when `done` answers true. The NPC may
-- stand off its spawn: the last steps close in on where it is now, into talking range.
local TALK_RANGE = 4
function S.talk(text, npc, want, done)
  local near = false
  return {
    text,
    start = function()
      S.want = want
    end,
    poke = function()
      if near then
        ProbeInteractEntry(npc)
      end
    end,
    every = 5,
    done = function()
      if not near then
        local _, x, y, z = ProbeNearest(npc)
        near = not x or S.steer(x, y, z, TALK_RANGE, false, 6)
        if near then
          ProbeInteractEntry(npc)
        end
        return false
      end
      return done()
    end,
    status = function()
      local d = ProbeNearest(npc)
      return string.format("at %s, %s yd from the NPC", S.where(), tostring(d and math.floor(d)))
    end,
    timeout = 30,
  }
end

-- The strikes on the main bar's keys, found by name at the first fight: pressed in turn, as a
-- player's rotation (nothing with a long cast or a summon). Icy Touch and Death Coil reach a
-- fleeing unit.
local STRIKES = { ["Icy Touch"] = true, ["Plague Strike"] = true, ["Blood Strike"] = true,
  ["Death Strike"] = true, ["Death Coil"] = true, ["Heart Strike"] = true, ["Obliterate"] = true,
  ["Scourge Strike"] = true }
local strikeKeys, nextStrike = nil, 1
local function strikes()
  if not strikeKeys then
    strikeKeys = {}
    for slot = 1, 12 do
      -- A spell action's second value is its spellbook slot, its fourth the spell.
      local kind, id, _, spell = GetActionInfo(slot)
      local name = kind == "spell" and (spell and GetSpellInfo(spell) or GetSpellName(id, "spell"))
      if name and STRIKES[name] then
        table.insert(strikeKeys, slot)
        S.log("strike key %d: %s", slot, name)
      end
    end
  end
  return strikeKeys
end

-- The nearest living unit of any of `entries`: its entry, yards and position.
function S.nearestOf(entries)
  local best, bd, bx, by, bz
  for _, e in ipairs(entries) do
    local d, x, y, z = ProbeNearest(e, true)
    if d and (not bd or d < bd) then
      best, bd, bx, by, bz = e, d, x, y, z
    end
  end
  return best, bd, bx, by, bz
end

-- A step that fights until `done`, a player's way: right-click the nearest unit `want` names
-- (entry, yards, x, y, z; which targets it and starts the swings), close in on the target where it
-- runs, press the strikes in turn, and on its death take the next. A target that loses no health
-- for 20 s (out of reach, or evading) is skipped for good; with no wanted unit in sight the step
-- walks the `roam` spots. `progress`, when given, adds to the step's status line.
local STALL = 20
function S.fight(text, want, done, roam, timeout, progress)
  local spot, guid, hp, hpAt, swungAt = 1, nil, 0, 0, 0
  return {
    text,
    done = function()
      if done() then
        S.release()
        return true
      end
      local now = GetTime()
      local live = UnitExists("target") and not UnitIsDead("target") and UnitCanAttack("player", "target")
      if live then
        local g = UnitGUID("target")
        if g ~= guid or UnitHealth("target") < hp then
          guid, hpAt = g, now
        end
        hp = UnitHealth("target")
        if now - hpAt > STALL then
          S.log("skipping %s (%s): no damage in %d s", tostring(UnitName("target")), g, STALL)
          ProbeSkip(g)
          ClearTarget()
          S.release()
          return false
        end
        local x, y, z = ProbeTargetPos()
        if x then
          S.steer(x, y, z, 3, false, 8)
        end
        if now - swungAt > 1.5 then
          swungAt = now
          local keys = strikes()
          if #keys > 0 then
            nextStrike = nextStrike % #keys + 1
            S.pressKey(keys[nextStrike])
          end
        end
        return false
      end
      guid, hp = nil, 0
      local e, d, x, y, z = want()
      if e then
        if d > 25 then
          S.steer(x, y, z, 20, false, 10)
        elseif now - swungAt > 1 then
          swungAt = now
          ProbeInteractEntry(e, true)
        end
        return false
      end
      local p = roam[spot]
      if S.steer(p[1], p[2], p[3], 4, false, 10) then
        spot = spot % #roam + 1
      end
      return false
    end,
    status = function()
      return "at " .. S.where() .. (progress and (", " .. progress()) or "")
    end,
    timeout = timeout,
  }
end

-- The step machine.
local f = CreateFrame("Frame")
local acc, setupAt, stepAt, idx, started, finished, pokedAt = 0, 0, 0, 0, false, false, 0
local t0, lastTick = now(), now()

-- Every stretch's setup opens on the character's level, reset to STRETCH_LEVEL (default 60): the
-- quests along the chain raise it, and a stretch reads the same at any point of a long series. By
-- name, as the selection a level command would take follows the server's, not ours.
local setup
local function setupLines()
  local lines = {
    function()
      S.gm(string.format(".character level %s %d", UnitName("player"), STRETCH_LEVEL or 60))
    end,
  }
  for _, line in ipairs(STRETCH.setup or {}) do
    table.insert(lines, line)
  end
  return lines
end

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
  local dt = now() - lastTick
  lastTick = now()
  local t = now() - t0
  setup = setup or setupLines()
  -- Setup: one GM line a second.
  if setupAt < #setup then
    if t >= setupAt + 1 then
      setupAt = setupAt + 1
      local line = setup[setupAt]
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
  -- A held dialog stops the step, as a player reads: its time counts toward neither the timeout
  -- nor the next poke.
  if S.dialogHeld() then
    stepAt, pokedAt = stepAt + dt, pokedAt + dt
    return
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
