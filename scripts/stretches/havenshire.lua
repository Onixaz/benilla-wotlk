-- Havenshire (12678, 12679, 12711): If Chaos Drives, Let Suffering Hold The Reins from Prince
-- Valanar and Tonight We Dine In Havenshire from Orithos, the walk down from Death's Breach,
-- Abandoned Mail at its mailbox, the fights (a right-click and the strikes on ten Scarlet
-- crusaders and ten citizens), the Saronite Arrows looted off the ground, and both turn-ins back at
-- the Breach.

local S = Stretch
local walk, talk = S.walk, S.talk

local VALANAR = 28377
local ORITHOS = 28647
local MAILBOX = 190917
local ARROW = 190691
local ARROW_ITEM = 39160

-- The two kill objectives' units: each credits its proxy (28763, 28764). The level-44 peasants and
-- miners go before the soldiers.
local CRUSADERS = { { 28557, 28841 }, { 28608, 28609, 28611 } }
local CITIZENS = { 28576, 28577 }

-- Off the Breach by the ramp at its north-west corner, then south into Havenshire down the slope at
-- x 2145, west of the lumber mill's carts and of a ledge the walk back up cannot climb.
local DOWN = { { 2330.0, -5690.0, 154.0 }, { 2300.0, -5677.0, 153.0 }, { 2275.0, -5672.0, 148.0 },
  { 2250.0, -5670.0, 142.0 }, { 2225.0, -5672.0, 125.0 }, { 2200.0, -5676.0, 115.0 },
  { 2175.0, -5680.0, 111.0 }, { 2146.0, -5683.0, 106.0 }, { 2144.0, -5700.0, 103.0 },
  { 2150.0, -5715.0, 102.0 } }
local TO_MAILBOX = { { 2140.0, -5770.0, 101.0 }, { 2117.0, -5792.0, 101.4 } }
-- Where the fights and the arrows are thick, walked in turn while none is in sight.
local HUNT = { { 2170.0, -5850.0, 101.0 }, { 2000.0, -5840.0, 101.0 }, { 1880.0, -5880.0, 103.0 },
  { 1780.0, -5900.0, 116.0 } }
local GATHER = { { 1870.0, -5840.0, 102.0 }, { 1760.0, -5900.0, 116.0 }, { 1700.0, -5860.0, 116.0 } }
-- Back up from the mailbox the way down, west of the fence at (2157, -5735).
local UP = { { 2140.0, -5770.0, 101.0 } }
for i = #DOWN, 1, -1 do
  table.insert(UP, DOWN[i])
end
-- Out of Valanar's tent by its mouth, south of the bookshelf, and back in the same way.
local TO_ORITHOS = { { 2385.5, -5705.0, 153.9 }, { 2392.0, -5711.0, 153.9 }, { 2378.0, -5724.0, 154.0 },
  { 2330.0, -5732.0, 154.0 } }
local TO_VALANAR = { { 2378.0, -5724.0, 154.0 }, { 2392.0, -5711.0, 153.9 }, { 2385.5, -5705.0, 153.9 },
  { 2376.0, -5702.5, 154.0 } }

local function inLog(id)
  return S.questIndex(id) ~= nil
end

-- Whether objective `j` of quest `id` is done.
local function objective(id, j)
  local i = S.questIndex(id)
  if not i then
    return false
  end
  local _, _, finished = GetQuestLogLeaderBoard(j, i)
  return finished and true or false
end

-- The nearest living unit of any of `entries`: its entry, yards and position.
local function nearestOf(entries)
  local best, bd, bx, by, bz
  for _, e in ipairs(entries) do
    local d, x, y, z = ProbeNearest(e, true)
    if d and (not bd or d < bd) then
      best, bd, bx, by, bz = e, d, x, y, z
    end
  end
  return best, bd, bx, by, bz
end

-- The strikes on the main bar's keys, found by name at the fight's start: pressed in turn, as a
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

-- Kills and arrows so far, as the quest log and the bags tell them.
local function progress()
  local i = S.questIndex(12678)
  local t = { tostring(GetItemCount(ARROW_ITEM)) }
  for j = 1, i and GetNumQuestLeaderBoards(i) or 0 do
    t[#t + 1] = (GetQuestLogLeaderBoard(j, i))
  end
  return table.concat(t, "; ")
end

-- A step that walks to the nearest wanted object and right-clicks it every 1.5 s until `done`; with
-- none in sight it walks the `roam` spots in turn. A walk with no progress for 25 s (an object out
-- of reach) is dropped for the next spot.
local CHASE = 25
local function gather(text, want, done, roam, reach, timeout)
  local clickedAt, spot, last, lastAt, ignoreUntil = 0, 1, nil, 0, 0
  return {
    text,
    done = function()
      if done() then
        S.release()
        return true
      end
      local now = GetTime()
      local p = progress()
      if p ~= last then
        last, lastAt = p, now
        S.log("progress: %s", p)
      end
      local e, _, x, y, z
      if now >= ignoreUntil then
        e, _, x, y, z = want()
        if e and now - lastAt > CHASE then
          S.log("no progress for %d s: on to the next spot", CHASE)
          ignoreUntil, lastAt, spot = now + 20, now, spot % #roam + 1
          e = nil
        end
      end
      if not e then
        local p = roam[spot]
        if S.steer(p[1], p[2], p[3], 4, false, 10) then
          spot = spot % #roam + 1
        end
        return false
      end
      if S.steer(x, y, z, reach, false, 8) and GetTime() - clickedAt > 1.5 then
        clickedAt = GetTime()
        ProbeInteractEntry(e)
      end
      return false
    end,
    status = function()
      return string.format("at %s, chaos %s, arrows %d", S.where(), S.objectives(12678),
        GetItemCount(ARROW_ITEM))
    end,
    timeout = timeout,
  }
end

-- A step that fights until `done`, a player's way: right-click the nearest wanted unit (which
-- targets it and starts the swings), close in on the target where it runs, press the strikes in
-- turn, and on its death take the next. A target that loses no health for 20 s (out of reach, or
-- evading) is skipped for good; with no wanted unit in sight the step walks the `roam` spots.
local STALL = 20
local function fightStep(text, want, done, roam, timeout)
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
      return string.format("at %s, %s", S.where(), progress())
    end,
    timeout = timeout,
  }
end

local function foe()
  if not objective(12678, 1) then
    for _, tier in ipairs(CRUSADERS) do
      local e, d, x, y, z = nearestOf(tier)
      if e and d < 60 then
        return e, d, x, y, z
      end
    end
    local e, d, x, y, z = nearestOf(CRUSADERS[2])
    if e then
      return e, d, x, y, z
    end
  end
  if not objective(12678, 2) then
    return nearestOf(CITIZENS)
  end
end

local function arrow()
  local d, x, y, z = ProbeNearest(ARROW)
  if d then
    return ARROW, d, x, y, z
  end
end

local steps = {
  talk("accept If Chaos Drives, Let Suffering Hold The Reins from Prince Valanar", VALANAR,
    { accept = 12678 }, function()
      return inLog(12678)
    end),
  walk("walk to Orithos the Sky Darkener", TO_ORITHOS),
  talk("accept Tonight We Dine In Havenshire from Orithos", ORITHOS, { accept = 12679 }, function()
    return inLog(12679)
  end),
  walk("walk down from Death's Breach to Havenshire", DOWN, 120),
  walk("walk to the abandoned mailbox", TO_MAILBOX),
  -- The mailbox gives the quest and takes it back at once: nothing to do between.
  talk("take Abandoned Mail at the mailbox to its reward", MAILBOX, { accept = 12711, turnin = 12711 },
    function()
      if S.completed["Abandoned Mail"] then
        S.trailStart()
        return true
      end
    end),
  fightStep("slay ten Scarlet crusaders and ten citizens of Havenshire", foe, function()
    local _, complete = S.questState(12678)
    return complete
  end, HUNT, 900),
  gather("gather fifteen Saronite Arrows", arrow, function()
    return GetItemCount(ARROW_ITEM) >= 15
  end, GATHER, 3, 420),
  -- Back from wherever the last arrow lay by the ground the fights and the gathering covered.
  {
    "walk back to the mailbox the way we came",
    done = S.retrace,
    status = function()
      return "at " .. S.where() .. ", trail " .. #(S.trail or {})
    end,
    timeout = 240,
  },
  walk("walk back up to Death's Breach", UP, 120),
  talk("turn in Tonight We Dine In Havenshire to Orithos", ORITHOS, { turnin = 12679 }, function()
    return not inLog(12679)
  end),
  walk("walk to Prince Valanar", TO_VALANAR),
  talk("turn in If Chaos Drives, Let Suffering Hold The Reins", VALANAR, { turnin = 12678 }, function()
    return not inLog(12678)
  end),
}

STRETCH = {
  name = "havenshire",
  setup = {
    ".cheat god on",
    function()
      TargetUnit("player")
    end,
    ".unaura 51852",
    function()
      ClearTarget()
    end,
    ".quest remove 12678",
    ".quest remove 12679",
    ".quest remove 12711",
    -- Tonight We Dine shares an exclusive group with Death's Challenge.
    ".quest remove 12733",
    -- All three follow The Scarlet Harvest.
    ".quest add 12670",
    ".quest complete 12670",
    ".quest reward 12670",
    -- An earlier run's arrows.
    ".additem 39160 -15",
    ".go xyz 2378.0 -5702.0 154.0 609 3.4",
  },
  steps = steps,
}
