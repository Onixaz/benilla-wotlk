-- The descent from Acherus (12657 -> 12849, 12850 -> 12670): The Might Of The Scourge from the
-- Lich King, the teleporter down to the Hall of Command, its turn-in to Mograine and the two
-- quests that follow, Thorval's, Thalanor's, the Scourge Gryphon's ride down to Death's Breach (a
-- spellclick that starts a taxi flight) and The Scarlet Harvest's turn-in to Prince Valanar.

local S = Stretch

local LICH_KING = 25462
local MOGRAINE = 28444
local THORVAL = 28472
local THALANOR = 28510
local GRYPHON = 29488
local VALANAR = 28377

-- The pad in the Heart of Acherus that teleports down to the Hall of Command, and a height between
-- the two floors.
local PAD = { 2383.7, -5645.2, 420.9 }
local HALL_Z = 400

-- Walks, as the movement keys take them; the last point stands in talking range of its NPC.
-- The pad stands below the Lich King's platform, open to the north-east: down the north stairs and
-- round into its mouth, wide of the Hall's own pad, whose trigger straight below at (2390, -5641)
-- takes a body passing over it on this floor too.
local TO_PAD = { { 2366.0, -5645.0, 426.0 }, { 2378.0, -5637.0, 421.5 }, { 2397.0, -5636.0, 420.7 },
  { 2395.0, -5646.0, 420.7 }, { 2388.4, -5645.5, 420.8 }, PAD }
local TO_MOGRAINE = { { 2455.5, -5596.5, 367.5 } }
local TO_THORVAL = { { 2522.5, -5552.5, 377.1 } }
-- Thalanor's balcony: back by Mograine the way we came, past the Hall's pad, up the steps on its
-- east side.
local TO_THALANOR = { { 2455.5, -5596.5, 367.5 }, { 2402.0, -5634.0, 377.0 }, { 2398.0, -5648.0, 377.0 }, { 2390.0, -5651.0, 380.0 },
  { 2381.0, -5653.0, 382.4 }, { 2375.0, -5662.0, 382.3 }, { 2373.0, -5672.0, 382.3 } }
local TO_GRYPHON = { { 2352.5, -5690.0, 382.3 } }
-- Valanar's tent: south of the bookshelf at its mouth.
local TO_VALANAR = { { 2392.0, -5707.0, 153.9 }, { 2385.5, -5705.0, 153.9 }, { 2376.0, -5702.5, 154.0 } }

local walk, talk = S.walk, S.talk

local function inLog(id)
  return S.questIndex(id) ~= nil
end

local steps = {
  talk("accept The Might Of The Scourge from the Lich King", LICH_KING, { accept = 12657 },
    function()
      return inLog(12657)
    end),
  -- Done on arrival below: the Hall's own pad, back up, stands right under this one.
  {
    "walk onto the teleporter down to the Hall of Command",
    done = function()
      local _, _, z = ProbePos()
      if z and z < HALL_Z then
        S.release()
        return true
      end
      S.route(TO_PAD)
    end,
    status = function()
      return "at " .. S.where() .. ", leg " .. tostring(TO_PAD.at)
    end,
    timeout = 40,
  },
  walk("walk to Highlord Darion Mograine", TO_MOGRAINE),
  talk("turn in The Might Of The Scourge and accept Report To Scourge Commander Thalanor", MOGRAINE,
    { turnin = 12657, accept = 12850 },
    function()
      return not inLog(12657) and inLog(12850)
    end),
  walk("walk to Lord Thorval", TO_THORVAL),
  talk("take The Power Of Blood, Frost And Unholy to its reward", THORVAL,
    { accept = 12849, turnin = 12849 },
    -- Nothing to do: Thorval offers it straight to its reward, and it never enters the log.
    function()
      return S.completed["The Power Of Blood, Frost And Unholy"]
    end),
  walk("walk to Scourge Commander Thalanor", TO_THALANOR, 90),
  talk("turn in Report To Scourge Commander Thalanor and accept The Scarlet Harvest", THALANOR,
    { turnin = 12850, accept = 12670 },
    function()
      return not inLog(12850) and inLog(12670)
    end),
  walk("walk to a Scourge Gryphon", TO_GRYPHON),
  {
    "mount the Scourge Gryphon",
    start = function()
      S.want = {}
    end,
    -- The click is a spellclick (`CMSG_SPELLCLICK`), which the server answers with a taxi flight.
    poke = function()
      if not UnitOnTaxi("player") then
        ProbeInteractEntry(GRYPHON)
      end
    end,
    every = 5,
    done = function()
      return UnitOnTaxi("player")
    end,
    timeout = 30,
  },
  {
    "the gryphon flies down to Death's Breach and sets us down",
    done = function()
      local _, _, z = ProbePos()
      return not UnitOnTaxi("player") and z and z < 200 and S.steerable()
    end,
    status = function()
      return "at " .. S.where() .. ", on taxi " .. tostring(UnitOnTaxi("player"))
    end,
    timeout = 120,
  },
  walk("walk to Prince Valanar", TO_VALANAR),
  talk("turn in The Scarlet Harvest", VALANAR, { turnin = 12670 }, function()
    return not inLog(12670)
  end),
}

STRETCH = {
  name = "descent",
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
    ".quest remove 12678",
    ".quest remove 12670",
    ".quest remove 12850",
    ".quest remove 12849",
    ".quest remove 12657",
    -- The Might Of The Scourge follows Death Comes From On High.
    ".quest add 12641",
    ".quest complete 12641",
    ".quest reward 12641",
    -- An earlier run's reward amulet, so the bags never fill.
    ".additem 38662 -1",
    ".go xyz 2336.0 -5668.0 426.6 609 0.45",
  },
  steps = steps,
}
