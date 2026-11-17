-- Palomino (12680, 12687): Grand Theft Palomino and Into the Realm of Shadows from Salanar the
-- Horseman. The walk down to the Havenshire Stables, a horse taken by its spellclick (a vehicle
-- seat), the ride back up to Death's Breach and its Deliver Stolen Horse action on Salanar; then
-- the Dark Rider of Acherus fought off his Deathcharger, the Deathcharger taken by its spellclick
-- and ridden back to the Breach, its Horseman's Call action, both turn-ins, and the mount the
-- second one teaches, called from the companion list and ridden.

local S = Stretch
local walk, talk = S.walk, S.talk

local SALANAR = 28653
local HORSES = { 28605, 28606, 28607 }
local DARK_RIDER = 28768
local DEATHCHARGER = 28782
-- The mount Into the Realm of Shadows teaches.
local MOUNT_SPELL = 48778

-- Off the Breach by the ramp at its north-west corner and down the slope at x 2145 into
-- Havenshire, as the havenshire stretch walks it.
local DOWN = { { 2330.0, -5690.0, 154.0 }, { 2300.0, -5677.0, 153.0 }, { 2275.0, -5672.0, 148.0 },
  { 2250.0, -5670.0, 142.0 }, { 2225.0, -5672.0, 125.0 }, { 2200.0, -5676.0, 115.0 },
  { 2175.0, -5680.0, 111.0 }, { 2146.0, -5683.0, 106.0 }, { 2144.0, -5700.0, 103.0 },
  { 2150.0, -5715.0, 102.0 } }
-- South past the fence at (2157, -5735) and west of the tree at (2149.5, -5758) to the stables,
-- where the horses stand.
local TO_STABLES = { { 2142.0, -5750.0, 101.0 }, { 2140.0, -5770.0, 101.0 }, { 2165.0, -5825.0, 101.4 } }
-- The fields south of the Breach, where the Deathchargers walk.
local FIELDS = { { 2118.0, -5762.0, 98.4 }, { 2001.0, -5751.0, 100.4 }, { 2074.0, -5827.0, 102.5 },
  { 2183.0, -5809.0, 101.4 } }
-- Back up the slope the way down.
local UP = {}
for i = #DOWN, 1, -1 do
  table.insert(UP, DOWN[i])
end

local function inLog(id)
  return S.questIndex(id) ~= nil
end

-- The action key `n` reaches while seated, for the log: the vehicle bar's button when its bar is
-- up, the bonus bar's otherwise.
local function action(n)
  local vehicleBar = VehicleMenuBar and VehicleMenuBar:IsShown()
  local b = _G[(vehicleBar and "VehicleMenuBarActionButton" or "BonusActionButton") .. n]
  local slot = b and b.action
  if not slot then
    return S.list("vehicle bar", vehicleBar, "no slot")
  end
  -- A spell action's fourth value is the spell; the second is its spellbook slot, which a seat's
  -- action does not have.
  local kind, _, _, spell = GetActionInfo(slot)
  return S.list("vehicle bar", vehicleBar, "slot", slot, kind, spell, spell and GetSpellInfo(spell))
end

-- A step that takes the seat of the nearest of `entries` by its spellclick, from talking range.
local function board(text, entries, timeout)
  local clickedAt = 0
  return {
    text,
    start = function()
      S.want = {}
    end,
    done = function()
      if UnitInVehicle("player") then
        S.release()
        S.log("seated: vehicle %s, key 1 %s", tostring(UnitName("vehicle")), action(1))
        return true
      end
      local e, _, x, y, z = S.nearestOf(entries)
      if e and S.steer(x, y, z, 4, false, 6) and GetTime() - clickedAt > 2 then
        clickedAt = GetTime()
        ProbeInteractEntry(e, true)
      end
      return false
    end,
    status = function()
      local _, d = S.nearestOf(entries)
      return string.format("at %s, %s yd from the nearest", S.where(), tostring(d and math.floor(d)))
    end,
    timeout = timeout or 60,
  }
end

-- A ride back to Salanar the way the walk came: the trail to its first crumb, up the slope by
-- `up` when the trail starts below it, then into range of Salanar, still seated.
local function rideBack(text, up)
  local leg = "trail"
  return {
    text,
    done = function()
      if not UnitInVehicle("player") then
        return false, "thrown from the seat " .. S.where()
      end
      if leg == "trail" then
        if S.retrace() then
          leg = up and "up" or "salanar"
        end
      elseif leg == "up" then
        if S.route(up) then
          leg = "salanar"
        end
      else
        local _, x, y, z = ProbeNearest(SALANAR)
        return not x or S.steer(x, y, z, 5, false, 6)
      end
      return false
    end,
    status = function()
      return "at " .. S.where() .. ", leg " .. leg
    end,
    timeout = 240,
  }
end

-- The seat's first action, pressed as its key until `done`.
local function useAction(text, done, timeout)
  local pressedAt = 0
  return {
    text,
    done = function()
      if done() then
        return true
      end
      if UnitInVehicle("player") and GetTime() - pressedAt > 3 then
        pressedAt = GetTime()
        S.log("key 1: %s", action(1))
        S.pressKey(1)
      end
      return false
    end,
    status = function()
      return "at " .. S.where() .. ", in vehicle " .. tostring(UnitInVehicle("player"))
        .. ", 12687 " .. S.objectives(12687)
    end,
    timeout = timeout or 30,
  }
end

-- The Dark Rider, while one rides in sight.
local function rider()
  local d, x, y, z = ProbeNearest(DARK_RIDER, true)
  if d then
    return DARK_RIDER, d, x, y, z
  end
end

local riderDown = false
local deaths = CreateFrame("Frame")
deaths:RegisterEvent("COMBAT_LOG_EVENT_UNFILTERED")
deaths:SetScript("OnEvent", function(_, _, _, kind, _, _, _, guid, name)
  if kind == "UNIT_DIED" and name == "Dark Rider of Acherus" then
    riderDown = true
    S.log("the Dark Rider died: %s", tostring(guid))
  end
end)

-- The mount's index in the companion list.
local function mountIndex()
  for i = 1, GetNumCompanions("MOUNT") do
    local _, name, spell = GetCompanionInfo("MOUNT", i)
    if spell == MOUNT_SPELL then
      return i, name
    end
  end
end

local steps = {
  talk("accept Grand Theft Palomino from Salanar the Horseman", SALANAR, { accept = 12680 }, function()
    return inLog(12680)
  end),
  -- The trail starts at the top: the horse rides back up on the line the walk came down, which
  -- keeps the middle of the lane between the bank and the barricade at (2158, -5677), where a
  -- horse on the straight legs catches.
  {
    "walk down from Death's Breach to Havenshire",
    start = function()
      S.trailStart()
    end,
    done = function()
      return S.route(DOWN)
    end,
    status = function()
      return "at " .. S.where() .. ", leg " .. tostring(DOWN.at)
    end,
    timeout = 120,
  },
  walk("walk to the Havenshire Stables", TO_STABLES),
  board("steal a Havenshire horse", HORSES),
  rideBack("ride the stolen horse back up to Salanar"),
  useAction("deliver the stolen horse to Salanar", function()
    local _, complete = S.questState(12680)
    return complete
  end),
  talk("turn in Grand Theft Palomino and accept Into the Realm of Shadows", SALANAR,
    { turnin = 12680, accept = 12687 }, function()
      return not inLog(12680) and inLog(12687)
    end),
  walk("walk down from Death's Breach to the fields", DOWN, 120),
  S.fight("slay the Dark Rider of Acherus", rider, function()
    if riderDown or UnitIsDead("target") and UnitName("target") == "Dark Rider of Acherus" then
      S.trailStart()
      return true
    end
  end, FIELDS, 300),
  board("take the Dark Rider's Acherus Deathcharger", { DEATHCHARGER }),
  rideBack("ride the Deathcharger back up to Death's Breach", UP),
  useAction("use the Horseman's Call", function()
    local _, complete = S.questState(12687)
    return complete
  end, 60),
  -- The reward teaches the mount, which joins the companion list, not the spellbook.
  talk("turn in Into the Realm of Shadows", SALANAR, { turnin = 12687 }, function()
    return not inLog(12687) and mountIndex() ~= nil
  end),
  {
    "call the Acherus Deathcharger from the mount list",
    start = function()
      S.want = {}
    end,
    poke = function()
      local i, name = mountIndex()
      if i and not IsMounted() then
        S.log("CallCompanion(MOUNT, %d): %s", i, tostring(name))
        CallCompanion("MOUNT", i)
      end
    end,
    every = 5,
    done = function()
      return IsMounted() == 1
    end,
    status = function()
      return S.list("mounts", GetNumCompanions("MOUNT"), "index", mountIndex(), "mounted", IsMounted())
    end,
    timeout = 30,
  },
  walk("ride the Acherus Deathcharger about the Breach", { { 2330.0, -5690.0, 154.0 }, { 2345.0, -5700.0, 154.0 } }),
  {
    "dismount",
    start = function()
      Dismount()
    end,
    done = function()
      return not IsMounted()
    end,
    timeout = 10,
  },
}

STRETCH = {
  name = "palomino",
  setup = {
    ".cheat god on",
    function()
      TargetUnit("player")
    end,
    ".unaura 51852",
    -- An earlier run's seat, and its mount.
    ".unaura 52263",
    ".unaura 52349",
    function()
      ClearTarget()
    end,
    ".quest remove 12680",
    ".quest remove 12687",
    ".quest remove 12697",
    ".unlearn 48778",
    -- Into the Realm of Shadows' reward item.
    ".additem 39208 -1",
    -- Grand Theft Palomino follows The Scarlet Harvest.
    ".quest add 12670",
    ".quest complete 12670",
    ".quest reward 12670",
    ".go xyz 2340.0 -5692.0 154.0 609 5.5",
  },
  steps = steps,
}
