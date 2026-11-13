-- The Eye of Acherus (12636 -> 12641, Death Comes From On High): a quest turn-in and accept by
-- gossip, the control mechanism's use, the possession and its pet bar on the main bar, the eye's
-- scripted launch, its own buttons, flying it by the movement keys, its siphon on four buildings,
-- the recall, and the turn-in on our own body again.

local S = Stretch

-- The Lich King, the mechanism, and the four buildings' credit units with where they stand.
local LICH_KING = 25462
local MECHANISM = 191609
local BUILDINGS = {
  { "New Avalon Forge", 28525, 1814.6, -5988.7, 125.5 },
  { "Scarlet Hold", 28542, 1651.2, -5994.7, 133.6 },
  { "New Avalon Town Hall", 28543, 1590.8, -5731.7, 143.9 },
  { "Chapel of the Crimson Flame", 28544, 1385.9, -5702.1, 146.3 },
}
-- How close the eye flies over each building's credit unit before it siphons, and how high.
local SIPHON_RANGE = 40
local SIPHON_HEIGHT = 25

local function foreign()
  local _, _, _, f = ProbePos()
  return f
end

local function possessed()
  return UnitExists("pet") and GetBonusBarOffset() == 5
end

local steps = {
  {
    "turn in The Eye Of Acherus and accept Death Comes From On High",
    start = function()
      S.want = { turnin = 12636, accept = 12641 }
    end,
    poke = function()
      ProbeInteractEntry(LICH_KING)
    end,
    done = function()
      return S.questIndex(12641) ~= nil and S.questIndex(12636) == nil
    end,
    timeout = 30,
  },
  {
    "use the Eye of Acherus control mechanism",
    start = function()
      S.want = {}
    end,
    poke = function()
      if not possessed() then
        ProbeInteractEntry(MECHANISM)
      end
    end,
    every = 8,
    done = possessed,
    status = function()
      return "pet=" .. tostring(UnitExists("pet")) .. " bonus=" .. tostring(GetBonusBarOffset())
    end,
    timeout = 30,
  },
  {
    "the eye launches to Havenshire",
    done = function()
      local x = ProbePos()
      if not possessed() then
        return false, "the possession ended in flight"
      end
      return foreign() and x and x < 1900
    end,
    status = function()
      return string.format("at %s, foreign %s", S.where(), tostring(foreign()))
    end,
    timeout = 60,
  },
  {
    "the eye's bar is lit",
    done = function()
      return IsUsableAction(121) and GameTooltip and true
    end,
    timeout = 20,
  },
}

-- The server's script roots the eye at its spline's end, boosts it and unroots it: then it is ours.
table.insert(steps, {
  "the eye is handed over (unrooted)",
  done = function()
    return possessed() and foreign() and S.steerable()
  end,
  timeout = 30,
})

-- The eye's own buttons, pressed as a player would: Shroud must complete a cast (and is pressed
-- again on each flight); Summon Ghouls needs corpses near, so its press is only logged.
local shroudBefore
table.insert(steps, {
  "the eye casts Shroud (bar slot 123)",
  start = function()
    shroudBefore = S.castCount("pet")
    S.pressKey(3)
  end,
  done = function()
    return S.castCount("pet") > shroudBefore
  end,
  status = function()
    return "pet casts " .. S.castCount("pet") .. ", usable " .. tostring(IsUsableAction(123))
  end,
  timeout = 15,
})
table.insert(steps, {
  "the eye tries Summon Ghouls (bar slot 122)",
  start = function()
    S.pressKey(2)
  end,
  done = function()
    return true
  end,
})

for _, b in ipairs(BUILDINGS) do
  local name, entry, x, y, z = b[1], b[2], b[3], b[4], b[5]
  local before
  table.insert(steps, {
    "fly the eye to " .. name,
    -- Key 3, Shroud, on the way: the defenders shoot an eye that lingers unshrouded, and a press
    -- on the siphon's tick would be replaced by the siphon's cast.
    start = function()
      S.pressKey(3)
    end,
    done = function()
      if not possessed() then
        S.release()
        return false, "the possession ended"
      end
      -- The eye flies with the movement keys: click to move refuses another mover.
      return S.steer(x, y, z + SIPHON_HEIGHT, SIPHON_RANGE, true)
    end,
    status = function()
      return string.format("at %s, target (%.1f, %.1f, %.1f)", S.where(), x, y, z + SIPHON_HEIGHT)
    end,
    timeout = 90,
  })
  table.insert(steps, {
    "siphon " .. name,
    start = function()
      before = S.objectives(12641)
    end,
    -- Key 1: bar slot 121 on the possess page, the eye's first pet action; pressed again after
    -- each channel until the building credits.
    poke = function()
      S.pressKey(1)
    end,
    every = 7,
    done = function()
      return S.objectives(12641) ~= before
    end,
    status = function()
      return string.format("objectives %s, usable %s, at %s", S.objectives(12641),
        tostring(IsUsableAction(121)), S.where())
    end,
    timeout = 30,
  })
end

table.insert(steps, {
  "all four buildings analyzed",
  done = function()
    local _, complete = S.questState(12641)
    return complete
  end,
  timeout = 10,
})
table.insert(steps, {
  "recall the eye",
  start = function()
    -- Key 5: slot 125, the eye's fifth pet action, Recall Eye of Acherus.
    S.pressKey(5)
  end,
  done = function()
    return not foreign() and not UnitExists("pet") and GetBonusBarOffset() == 0
  end,
  timeout = 30,
})
table.insert(steps, {
  "turn in Death Comes From On High",
  start = function()
    S.want = { turnin = 12641 }
    ProbeInteractEntry(LICH_KING)
  end,
  done = function()
    return S.questIndex(12641) == nil
  end,
  timeout = 30,
})

STRETCH = {
  name = "eye",
  setup = {
    ".cheat god on",
    -- An eye left possessed by an earlier run keeps our view out at it; end it on ourselves.
    function()
      TargetUnit("player")
    end,
    ".unaura 51852",
    function()
      ClearTarget()
    end,
    -- A quest is offered only while the next in its chain is neither taken nor done.
    ".quest remove 12657",
    ".quest remove 12641",
    ".quest remove 12636",
    ".quest add 12636",
    ".quest complete 12636",
    ".go xyz 2336.0 -5668.0 426.6 609 2.8",
  },
  steps = steps,
}
