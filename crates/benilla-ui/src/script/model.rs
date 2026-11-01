use std::collections::{HashMap, HashSet};

use crate::layout::{LayoutInput, LayoutSolver, Rect};
use crate::widget::{FrameHandle, RegionHandle, WidgetArena};

use super::{
    auction, backdrop, bank, bind_confirm, calendar, camera_view, char_stats, chat_window,
    colorselect, combat_log, container, cursor, death, duel, follow, gossip, guild, inspect,
    item_text, lfg, loot, loot_roll, macros, mail, merchant, mirror_timer, parse_unit_token, party,
    petition, pvp, quest, quest_log, reputation, session, simplehtml, skills, slider, social,
    spellbook, stable, taxi, trade, tradeskill, trainer, unit_menu, weapon_enchant, ActionSlot,
    AuraState, FontObject, ItemTemplateView, MusicRequest, PlayerReqState, RegionData, ScriptValue,
    SoundRequest, UnitState, UnitTokenParse,
};

// ── The Rust-side model ──

/// The host's answer to whether a texture path resolves to a file.
pub type TextureProbe = Box<dyn Fn(&str) -> bool>;

/// The host's answer to a texture path's size in texels.
pub type TextureSizeProbe = Box<dyn Fn(&str) -> Option<(u32, u32)>>;

/// The host's answer to whether a font path loads.
pub type FontProbe = Box<dyn Fn(&str) -> bool>;

/// The host's builder for the macro icon chooser's list, run by the first `GetNumMacroIcons`.
pub type MacroIconSource = Box<dyn Fn() -> Vec<String>>;

/// Minted id to handle, indexed directly: ids are dense and every scripted call looks one up.
#[derive(Debug, Clone)]
pub(crate) struct IdMap<T>(Vec<Option<T>>);

impl<T> Default for IdMap<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T: Copy> IdMap<T> {
    pub(crate) fn get(&self, id: &u32) -> Option<&T> {
        self.0.get(*id as usize).and_then(Option::as_ref)
    }
    pub(crate) fn contains_key(&self, id: &u32) -> bool {
        self.get(id).is_some()
    }
    pub(crate) fn insert(&mut self, id: u32, v: T) -> Option<T> {
        let i = id as usize;
        if self.0.len() <= i {
            self.0.resize(i + 1, None);
        }
        self.0[i].replace(v)
    }
    pub(crate) fn remove(&mut self, id: &u32) -> Option<T> {
        self.0.get_mut(*id as usize).and_then(Option::take)
    }
}

/// The Rust-side model behind the Lua VM, in `lua.app_data`. It holds no mlua handles, whose
/// persistent count the vendored `LUAI_MAXCSTACK` caps.
pub(crate) struct Model {
    /// Which script state this VM is: it picks the event table and the frame types.
    pub(crate) vm: super::glue::VmKind,
    /// Every discovered addon in load order: the AddOn API's registry, filled at world entry.
    pub(crate) addons: Vec<super::addon::AddOnInfo>,
    /// The Lua index space into `addons`, a title-sorted, hidden-filtered list.
    pub(crate) addon_index: Vec<usize>,
    /// Lowercased names `SMSG_ADDON_INFO` marked `status = 2`; `None` until the reply arrives, and
    /// until then `GetNumAddOns()` is 0, as in the reference.
    pub(crate) addon_info_hidden: Option<Vec<String>>,
    /// The current character's enable hash, which every enable verb writes through and the shutdown
    /// writer saves only while dirty.
    pub(crate) addon_enable: super::EnableHash,
    /// The hash as the host read it from the file, what `ResetDisabledAddOns` reloads.
    pub(crate) addon_enable_saved: super::EnableHash,
    /// The AddOns folder, so `LoadAddOn` can read an addon's files from inside a Lua binding.
    pub(crate) addons_root: Option<std::path::PathBuf>,
    /// The host's reader for chain-sourced addon files; without one such an addon is `MISSING`.
    pub(crate) addons_chain_reader: Option<super::addon::SharedChainReader>,
    /// The host's font engine, so a metric answers inside the asking call, not a frame later.
    pub(crate) measurer: Option<Box<dyn super::TextMeasure>>,
    /// Whether a texture path resolves (patch chain or loose addon file), so the path form of
    /// `SetTexture` returns the reference's 1 or nil inline. `None` answers nil.
    pub(crate) texture_probe: Option<TextureProbe>,
    /// `SetUIVisibility` (`0x517c20`, `[0xac80a8]`): whether the world draws its UI, the names
    /// and plates over units, which `TOGGLEUI` turns off with `UIParent`.
    pub(crate) world_ui_visible: bool,
    /// A path's texel size, which an axis authored as 0 takes, one texel per unit, as the client's
    /// `GetWidth` and `GetHeight` do; `None` leaves it as authored.
    pub(crate) texture_size_probe: Option<TextureSizeProbe>,
    /// Whether a font path loads: `SetFont`'s 1 or nil, nil meaning a load failure. Unlike
    /// `texture_probe`, `None` answers 1 for a non-empty path: without a font backend nothing
    /// failed, and addons revert their font on a nil.
    pub(crate) font_probe: Option<FontProbe>,
    /// Saved-variables folders, one `<Addon>.lua` each: the account's, then this character's.
    pub(crate) addons_saved_account: Option<std::path::PathBuf>,
    pub(crate) addons_saved_character: Option<std::path::PathBuf>,
    /// FrameXML templates, global across files as the client's; held here so the loader runs from a
    /// bare `&Lua`, which lets `LoadAddOn` load inside a binding.
    pub(crate) framexml_templates:
        std::cell::RefCell<std::collections::HashMap<String, crate::framexml::Element>>,
    /// `FrameXML_Debug`'s flag (boots 0); the loader's trace lines print while it is greater than
    /// 0, so it is signed.
    pub(crate) framexml_debug: std::cell::Cell<i32>,
    /// The loads' records for `Logs\FrameXML.log` and the drains the host has yet to write.
    pub(crate) load_log: crate::status::LoadLog,
    /// The FrameXML font registry, a namespace apart from templates (a font inherits only a font).
    pub(crate) framexml_fonts:
        std::cell::RefCell<std::collections::HashMap<String, crate::framexml::Element>>,
    pub(crate) arena: WidgetArena,
    /// Per-frame anchors, size and scale; every live frame has one from creation on.
    pub(crate) layout_inputs: crate::widget::HandleMap<FrameHandle, LayoutInput>,
    /// Each resolvable frame's rect from the last resolve.
    pub(crate) resolved: crate::widget::HandleMap<FrameHandle, Rect>,
    /// The anchor solver, kept across resolves so a steady round allocates nothing; scratch only.
    pub(crate) solver: LayoutSolver,
    /// Tier 2 of the resolve gate, the last converged inputs' fingerprint; `None` forces a resolve.
    pub(crate) layout_fingerprint: Option<super::layout::InputFingerprint>,
    /// Tier 1 of the resolve gate: every layout write bumps it, and a resolve returns at once while
    /// it equals `layout_epoch_resolved`. Never bump it per frame: tier 2 hashes the whole UI.
    pub(crate) layout_epoch: u64,
    /// Layout ids named by writes since the last converged resolve, each write claiming it left the
    /// cached graph's shape alone, so a resolve seeds its dirty closure from them. `None` derives
    /// the graph in full; `WOW_LAYOUT_VERIFY` checks every incremental pass against a full one.
    pub(crate) layout_touched: Option<Vec<u32>>,
    /// Regions a write may have re-keyed for text measure (text, font, outline, wrap, scale) since
    /// the last sweep, which asks only these; `None` walks all. A site that forgets to name its
    /// region is caught by `WOW_LAYOUT_VERIFY`, on for every test here, which panics on the miss.
    pub(crate) measure_dirty: Option<Vec<crate::widget::RegionHandle>>,
    /// Per message frame, `(lines_gen, env hash)` at its last request-free sweep; a match skips it.
    /// A destroyed frame's entry is inert: generational handles never match again.
    pub(crate) msg_swept: std::collections::HashMap<crate::widget::FrameHandle, (u64, u64)>,
    /// Lines the message sweep hashed; a settled second sweep hashes none.
    pub(crate) msg_lines_hashed: u64,
    /// Under `WOW_LAYOUT_VERIFY`, the resolve just taken was incremental and owes a full re-run.
    pub(crate) layout_verify_recheck: bool,
    /// Per-node input hashes and dirty-closure scratch: which nodes a let-through resolve solves.
    pub(crate) layout_scope: super::layout::LayoutScope,
    /// The epoch the last converged resolve closed on; `None` sends the next one past tier 1.
    pub(crate) layout_epoch_resolved: Option<u64>,
    /// Resolves the gate let through, counted at its decision, so verify mode reads the same.
    pub(crate) layout_solves: u64,
    /// Resolves that got past tier 1, including those that then skipped: the whole-roster walk's
    /// cost, which `layout_solves` misses. Verify mode walks anyway, so assert on this only from a
    /// crate that depends on `benilla-ui`; inside this crate, assert on `layout_solves`.
    pub(crate) layout_gate_walks: u64,
    /// Full layout-graph derivations: 0 on a steady UI, so a rise means a write fell back to
    /// `touch_layout`. The `WOW_LAYOUT_VERIFY` re-run's derivation is not counted.
    pub(crate) layout_derives: u64,
    /// The last solve's width, `(frames solved, regions swept)`.
    pub(crate) layout_last_scope: (usize, usize),
    /// Fixpoint rounds run across all solves.
    pub(crate) layout_rounds: u64,
    /// Per frame, rasterized links `(y-up rect, link, full |H…|h markup)` for `OnHyperlinkClick`.
    pub(crate) link_spans: crate::widget::HandleMap<FrameHandle, Vec<(Rect, String, String)>>,
    /// Region visuals (texture, colour, text) and layout (anchors, size, justify).
    pub(crate) region_data: crate::widget::HandleMap<RegionHandle, RegionData>,
    /// Each frame's backdrop plate (`<Backdrop>` or `SetBackdrop`, the client's `frame+0x1ac`).
    pub(crate) backdrops: crate::widget::HandleMap<FrameHandle, backdrop::Backdrop>,
    /// The SimpleHTML widgets' state: element fonts `+0x350`, link format `+0x360`, content nodes
    /// `+0x340`.
    pub(crate) simple_html: simplehtml::SimpleHtmlStates,
    /// Named `<Font>` objects by lowercased name, as 1.12 matches them case-insensitively; read
    /// them only through `font_object`, where the fold lives.
    pub(crate) font_objects_by_lower: HashMap<String, FontObject>,
    /// Anchored regions' owner-relative rects from the last resolve; anchor-less ones are absent.
    pub(crate) region_resolved: crate::widget::HandleMap<RegionHandle, Rect>,

    /// The next object id to mint; ids start at 1, since 0 is `SCREEN`.
    pub(crate) next_id: u32,
    pub(crate) id_to_frame: IdMap<FrameHandle>,
    pub(crate) frame_to_id: crate::widget::HandleMap<FrameHandle, u32>,
    pub(crate) id_to_region: IdMap<RegionHandle>,
    pub(crate) region_to_id: crate::widget::HandleMap<RegionHandle, u32>,
    /// Region name to id, first wins: `SetPoint` anchors to a sibling region by name, after frames.
    pub(crate) region_names: HashMap<String, u32>,
    /// The reverse of [`Self::region_names`]: each named region's first name, so naming a region
    /// in an error string or `GetName` is one lookup, not a scan of every named region.
    pub(crate) region_name_by_id: HashMap<u32, String>,

    /// Which handlers each frame has; the closures themselves live in Lua's `REG_SCRIPTS` table.
    pub(crate) scripts: HashMap<FrameHandle, HashSet<&'static str>>,
    /// Frames with an `OnUpdate` script and their ids, kept by `SetScript`.
    pub(crate) on_update_frames: Vec<(FrameHandle, u32)>,
    /// Bumped when a frame gains or loses an `OnUpdate` or an animation group starts: the update
    /// walk compares it to notice that a later frame's slot changed under a handler.
    pub(crate) walk_roster: u64,
    /// Frames with an `OnSizeChanged` script, kept by `SetScript`, for the resolve's size snapshot.
    pub(crate) on_size_changed_frames: Vec<FrameHandle>,
    /// Frames with an `OnUpdateModel` script, fired as a visible model pane paints.
    pub(crate) on_update_model_frames: Vec<FrameHandle>,
    /// Frames inside their XML load, from its start to their own `OnLoad` (`+0x130`, set at
    /// `0x4903b2`, cleared at `0x490421` before `OnLoad`): their `OnEnable`/`OnDisable` stay quiet.
    pub(crate) xml_loading: HashSet<FrameHandle>,
    /// Per model file, a pane clock's sequences and bounds, read by the host off the `MD20`.
    pub(crate) model_facts: HashMap<String, std::sync::Arc<crate::widget::ModelFileFacts>>,
    /// Model files still waiting for their facts, for the host to load; deduplicated.
    pub(crate) model_facts_wanted: Vec<String>,
    /// The model instances `SetModel` has made in this state, the last one's number.
    pub(crate) model_instances: u64,
    /// Event name to its frames in registration order, never a set: the event signal dispatches in
    /// that order across frames. Re-registering keeps a frame's position.
    pub(crate) event_to_frames: HashMap<String, Vec<FrameHandle>>,
    pub(crate) frame_events: HashMap<FrameHandle, HashSet<String>>,
    /// `RegisterAllEvents` frames in registration order, dispatched after an event's own list and
    /// cleared only by `UnregisterAllEvents`. Deviation: a flag, not the reference's append to
    /// every event's list (which orders by registration time and lets `UnregisterEvent` drop one
    /// event), because events register here by name and no list of every name exists.
    pub(crate) all_event_frames: Vec<FrameHandle>,

    /// The focused edit box, taking all keys while visible; `None` admits `autoFocus`.
    pub(crate) focused_editbox: Option<FrameHandle>,

    /// The mouse-enabled frame under the cursor, for the next move's `OnLeave`/`OnEnter`.
    pub(crate) mouseover: Option<FrameHandle>,
    /// A frame under a still cursor hid or showed, so the tick re-runs the hover walk at the saved
    /// position and a newly exposed frame gets `OnEnter` without a mouse move: the reference's
    /// `[root+0x1100]` flag.
    pub(crate) hover_repick: bool,
    /// Per button, the frame its press landed on, for `OnClick`'s same-frame release test.
    pub(crate) mouse_down_on: HashMap<String, FrameHandle>,
    /// The held mouse buttons as `MOUSEBUTTON` bits, wherever pressed, as `IsMouseButtonDown` reads.
    pub(crate) mouse_buttons_down: u32,
    /// The client's one mouse-capture slot (`root+0x80`), read by the mouse-down raise. Set to the
    /// capture, else the hovered frame, so a chorded press keeps the capture; cleared once no
    /// button is down. A title-region press captures nothing.
    pub(crate) mouse_capture: Option<FrameHandle>,
    /// Per frame, when its last single `OnClick` fired: the client's button `[+0x334]`, the whole
    /// double-click state. It has no button identity (left then right makes a double), is zeroed
    /// when a double fires, and nothing else clears it, not hide, disable or the cursor leaving.
    pub(crate) last_click: HashMap<FrameHandle, f64>,

    /// Frames the last resolve resized, `(id, width, height)`, owed `OnSizeChanged` outside it.
    pub(crate) pending_size_changed: Vec<(u32, f32, f32)>,

    /// Script errors caught from `pcall`'d handlers, drained by the host each frame.
    pub(crate) errors: Vec<String>,
    /// Errors owed to `geterrorhandler()`, which the reference calls on each caught error; one the
    /// handler itself raises skips this queue, which stops the recursion.
    pub(crate) pending_error_dispatch: Vec<String>,
    /// The session's retained diagnostic log, deduplicated and bounded; `errors` drains each frame.
    pub(crate) diagnostics: super::diagnostics::DiagnosticLog,
    /// Non-fatal warnings for the host, such as a dropped `inherits=` template.
    pub(crate) warnings: Vec<String>,
    /// The screen-root rect (`[bottom, left, top, right]`), the anchor base for top-level frames.
    pub(crate) screen: Rect,

    /// Each unit token's state as pushed this frame, keyed lowercased because 1.12's resolver
    /// matches case-insensitively: read it only through `unit`, where the fold lives.
    pub(crate) units_by_lower: HashMap<String, UnitState>,
    /// The snapshot of each unit a `target` chain can end on, by guid, which the app pushes for
    /// [`super::UnitGuids::chain_ends`]: a token with a hop that no push names reads its
    /// resolved guid's entry here.
    pub(crate) units_by_guid: HashMap<u64, UnitState>,
    /// The player's auras in the reference cache's insertion order, durations joined: the
    /// `GetPlayerBuff` family's list, and any token naming the player's.
    pub(crate) player_auras: Vec<AuraState>,
    /// Every other unit's auras by guid, ascending aura slot: one list per unit, whatever token
    /// names it.
    pub(crate) unit_auras: HashMap<u64, Vec<AuraState>>,
    /// The unit-token resolver's inputs, which the aura bindings resolve through.
    pub(crate) unit_guids: super::UnitGuids,
    /// Each unit's running cast and channel, for `UnitCastingInfo` and `UnitChannelInfo`.
    pub(crate) unit_casts: super::unit_cast::UnitCasts,
    /// Spell ids the cancel verbs queued (`CancelUnitBuff`, `SetTracking`, …), one
    /// `CMSG_CANCEL_AURA` each.
    pub(crate) cancel_aura_requests: Vec<u32>,
    /// The minimap's tracking spell and town type, behind the tracking verbs.
    pub(crate) minimap_tracking: super::tracking::TrackingModel,
    /// The calls that touch the selection, the cast or the targeting cursor, in call order.
    pub(crate) script_calls: Vec<super::calls::ScriptCall>,
    /// The binding functions' queue, gate depth and mouselook state.
    pub(crate) input: super::input_verbs::InputState,
    /// `DropItemOnUnit` tokens, gated by the app; a refusal silently keeps the item.
    pub(crate) drop_item_on_unit: Vec<String>,

    /// Channels the server confirmed, in join order: the numbers `GetChannelName` answers.
    pub(crate) joined_channels: Vec<Option<String>>,
    /// The channel pane's display list, selection and roster.
    pub(crate) channel_pane: super::channel_pane::ChannelPane,
    /// The party or raid roster the app pushes; the default is not in a group.
    pub(crate) party: party::PartyState,
    /// The instance difficulties the app pushes, `GetDungeonDifficulty`'s and `GetRaidDifficulty`'s.
    pub(crate) difficulty: unit_menu::DifficultyState,
    /// The group list's LFG-restricted bit `[0xbd19a8]`, `HasLFGRestrictions`.
    pub(crate) lfg_restricted: bool,
    /// What `CanGrantLevel` and `CanSummonFriend` read beside the unit: the app's push.
    pub(crate) refer_a_friend: unit_menu::ReferAFriendState,
    /// Party and loot-method calls (`AcceptGroup`, `InviteUnit`, `SetLootMethod`, …) queued.
    pub(crate) party_requests: Vec<party::PartyRequest>,
    /// System chat lines the party bindings composed, drained by the app.
    pub(crate) party_lines: Vec<String>,
    /// The ready-check deadline and the unanswered flags the timeout tick reads.
    pub(crate) ready_check: party::ReadyCheckState,
    /// Saved raid lockouts from `SMSG_RAID_INSTANCE_INFO`, which outlive any roster change.
    pub(crate) saved_instances: Vec<party::SavedInstanceInfo>,
    /// `SetRaidRosterSelection`'s raid row index, client-side only.
    pub(crate) raid_selection: i64,
    /// `GetTime()`'s session seconds, which only the host moves ([`super::UiScript::tick`],
    /// [`super::UiScript::set_now`]); off `_G`, as the reference's clock is the OS tick count.
    pub(crate) now: f64,
    /// `GetGameTime()`'s `(hour, minute)`, pushed by the host as the game clock ticks.
    pub(crate) game_time: (u32, u32),
    /// The zone caches the four zone-text getters and `GetZonePVPInfo` read.
    pub(crate) zone: super::ZoneTexts,
    /// The current map's `Map.dbc` `InstanceType`, all `IsInInstance()` reads; `None` for no row.
    pub(crate) instance_type: Option<u32>,
    /// `CanShowResetInstances()`, the reference's four-term predicate, app-computed.
    pub(crate) can_reset_instances: bool,
    /// `ResetInstances()` calls queued.
    pub(crate) reset_instance_asks: u32,
    /// The instance lock clock (`0x513b90`), app-pushed each frame.
    pub(crate) instance_lock: super::instance::InstanceLock,
    /// `RespondInstanceLock(accept)` answers queued.
    pub(crate) instance_lock_responses: Vec<bool>,
    /// The `Map.dbc` ids whose `Flags` hold `0x100`, the dynamic-difficulty maps.
    pub(crate) dynamic_difficulty_maps: Vec<i32>,
    /// Friends, ignores and the last `/who`, display-resolved as the reference resolves them.
    pub(crate) social: social::SocialState,
    /// Social calls (`AddFriend`, `RemoveFriend`, `SendWho`, …) queued.
    pub(crate) social_requests: Vec<social::SocialRequest>,
    /// Guild roster, ranks, MOTD and info, sorted and filtered by the app, which holds the toggles.
    pub(crate) guild: guild::GuildState,
    /// The rank editor's unsaved edits, apart from `guild` so a push cannot discard them.
    pub(crate) guild_control: guild::GuildRankEdit,
    /// Guild calls (`GuildInvite`, `GuildSetMOTD`, `GuildControlSaveRank`, …) queued.
    pub(crate) guild_requests: Vec<guild::GuildRequest>,
    /// The guild bank as the app pushes it.
    pub(crate) guild_bank: super::guild_bank::GuildBankView,
    /// The current bank tab, 0-based (`0xc1dc0d`).
    pub(crate) guild_bank_tab: u8,
    /// Locked bank slots as `(tab, slot, item id)`, the reference's per-slot lock (`0x5a4c90`).
    pub(crate) guild_bank_locks: Vec<(u8, u8, u32)>,
    /// Guild bank sends, queued for the app.
    pub(crate) guild_bank_requests: Vec<super::guild_bank::GuildBankRequest>,
    /// The charter price and open petition, names resolved from caches as the reference does.
    pub(crate) petition: petition::PetitionState,
    /// Charter calls (`BuyGuildCharter`, `SignPetition`, `TurnInGuildCharter`, …) queued.
    pub(crate) petition_requests: Vec<petition::PetitionRequest>,
    /// The arena registrar's petition vendor, as pushed.
    pub(crate) petition_vendor: petition::PetitionVendorState,
    /// Per chat window from `ChatFrame1`, the tint, alpha and font size its tab menu can change.
    /// One per frame the target's FrameXML builds (`NUM_CHAT_WINDOWS`).
    pub(crate) chat_window_looks: Vec<chat_window::ChatWindowLook>,
    /// 0-based windows whose look Lua changed, the persist cue.
    pub(crate) chat_window_changes: HashSet<usize>,
    /// Chat-type colours: 94 fixed and 10 `CHANNELn`, overlaid by `chat-cache.txt`.
    pub(crate) chat_colors: Vec<super::chat_types::ChatTypeColor>,
    pub(crate) chat_colors_changed: bool,
    /// The languages this character knows, in `Languages.dbc` row order.
    pub(crate) known_languages: Vec<String>,
    /// `ChrClasses.dbc`'s rows in record order, which `FillLocalizedClassList` walks.
    pub(crate) class_names: Vec<super::class_list::ClassNames>,
    /// `Languages.dbc`'s `(ID, Name_lang)` rows in file order, whatever the character knows.
    pub(crate) language_table: Vec<(u32, String)>,
    pub(crate) zone_channel_catalog: Vec<super::channel::ZoneChannelRow>,
    pub(crate) channel_commands: Vec<super::channel::ChannelCommand>,
    /// The recruitment auto-join latch, the chat's guild-recruitment mode: 0 `STANDARD`, 1 `AUTO`.
    /// Boots at 1, as the reference writes `AUTO` for an untouched option.
    pub(crate) guild_recruitment_mode: u8,
    /// The recruitment mode moved: the chat cache's dirty signal.
    pub(crate) guild_recruitment_changed: bool,
    /// `DoEmote` calls queued.
    pub(crate) emote_requests: Vec<super::chat_misc::EmoteRequest>,
    /// `RandomRoll` calls queued.
    pub(crate) roll_requests: Vec<(u32, u32)>,
    /// `ConsoleExec` lines that were not CVar writes.
    pub(crate) console_lines: Vec<String>,
    /// `LoggingChat` and `LoggingCombat`'s flags, and their change cue.
    pub(crate) logging_chat: bool,
    pub(crate) logging_combat: bool,
    pub(crate) logging_changed: bool,
    /// The default chat language; `None` is no player, so `GetDefaultLanguage()` returns nothing.
    pub(crate) default_language: Option<String>,
    /// `AcceptDuel`, `CancelDuel` and `StartDuel` calls; Lua sees duels only through events.
    pub(crate) duel_requests: Vec<duel::DuelRequest>,
    /// The three mirror-timer records (`0xbd0b80`); `None` is an empty one.
    pub(crate) mirror_timers: [Option<mirror_timer::MirrorTimer>; 3],
    /// `FollowUnit` and follow-by-name calls; Lua sees following only as `AUTOFOLLOW_*` events.
    pub(crate) follow_requests: Vec<follow::FollowRequest>,
    /// `SetView`, `SaveView`, `ResetView`, `NextView`, `PrevView` and `FlipCameraYaw` calls; the
    /// views reach Lua only as CVars, `cameraView` and `camera{Distance,Pitch,Yaw}{,A..D}`.
    pub(crate) camera_view_requests: Vec<camera_view::CameraViewRequest>,
    /// `Logout`, `Quit`, `CancelLogout`, `ForceQuit` calls; Lua sees the countdown only as events.
    pub(crate) session_requests: Vec<session::SessionRequest>,
    /// The glue core's slots and request queue; empty in an in-game VM.
    pub(crate) glue: super::glue_core::GlueCore,
    /// `TogglePVP` calls, a count because `CMSG_TOGGLE_PVP` has no body.
    pub(crate) pvp_toggles: u32,
    /// `SetPVP` calls, each the byte its `CMSG_TOGGLE_PVP` carries.
    pub(crate) pvp_sets: Vec<u8>,
    /// The player's PvP toggle timer (`0x6de7a0`) as the app computed it this frame, ms.
    pub(crate) pvp_toggle_timer: i32,
    /// The inspected player's arena teams by slot, resolved by the app.
    pub(crate) inspect_arena_teams: [Option<pvp::InspectArenaTeamView>; 3],
    /// The arena-team mirror: the three teams, the roster's sort and selection.
    pub(crate) arena_teams: super::arena_team::ArenaTeams,
    /// The player's private honor fields; before the first push the six self getters read zeros.
    pub(crate) honor: Option<pvp::HonorState>,
    /// The last `MSG_INSPECT_HONOR_STATS` reply; its presence is `HasInspectHonorData`'s answer.
    pub(crate) inspect_honor: Option<pvp::InspectHonorData>,
    /// `RequestInspectHonorData` calls; the pending gate keeps this at 0 or 1.
    pub(crate) inspect_honor_requests: u32,
    /// An inspect-honor query is out; `RequestInspectHonorData` refuses while set. Cleared by a
    /// reply or by the app clearing the slot.
    pub(crate) inspect_honor_pending: bool,
    /// `ShowingHelm()`: `PLAYER_FLAGS`' `HIDE_HELM` bit as pushed, set ahead on `ShowHelm` because
    /// the wire verb is a blind toggle. Starts shown, as a fresh character's flags are 0.
    pub(crate) helm_shown: bool,
    /// `ShowingCloak()`, the same for `HIDE_CLOAK`.
    pub(crate) cloak_shown: bool,
    /// Flips from `ShowHelm`/`ShowCloak`, one `CMSG_TOGGLE_HELM`/`CMSG_TOGGLE_CLOAK` each.
    pub(crate) worn_display_toggles: Vec<super::worn_display::WornDisplay>,

    /// `PLAYER_FIELD_BYTES` byte 2 (descriptor `+0x102a`), which extra action bars are on. Only
    /// the server moves it, as in the reference: `SetActionBarToggles` leaves it a round trip late.
    pub(crate) action_bar_toggles: Option<u8>,
    /// One `CMSG_SET_ACTIONBAR_TOGGLES` payload per `SetActionBarToggles` call, ungated.
    pub(crate) action_bar_toggle_sends: Vec<u8>,

    /// `PlaySound` and `PlaySoundFile` calls queued.
    pub(crate) sound_queue: Vec<SoundRequest>,

    /// `PlayMusic` and `StopMusic` calls in call order, for the Lua music stream.
    pub(crate) music_queue: Vec<MusicRequest>,

    /// The UI-load sound-suppression depth, the reference's suppression counter, held across the
    /// whole UI load, so login and `/reloadui` are silent. Only name-keyed `PlaySound` reads it:
    /// the call is dropped before any lookup yet answers `(willPlay, handle)` as usual.
    /// `PlaySound(kitId)`, `PlaySoundFile` and music play normally.
    pub(crate) sound_suppression: u32,

    /// The output devices the engine cached at its last init, after the "System Default" entry
    /// the verbs add; `None` with no engine.
    pub(crate) sound_output_drivers: Option<Vec<String>>,

    /// `Sound_GameSystem_RestartSoundSystem` was called since the host's last drain.
    pub(crate) sound_restart: bool,

    /// The CVar table, by lowercased name.
    pub(crate) cvars: HashMap<String, super::cvars::CvarSlot>,
    /// The current map's `MapName_lang` and dynamic-difficulty bit, `GetInstanceInfo`'s.
    pub(crate) map_info: Option<(String, bool)>,
    /// Saved CVar values, set before any registration so a CVar starts at its saved value: the
    /// reference's table outlives `ReloadUI`, while ours is per VM.
    pub(crate) cvars_saved_base: HashMap<String, String>,
    /// `(registered name, new value)` per Lua `SetCVar`, the app's sync and save cue.
    pub(crate) cvar_changes: Vec<(String, String)>,
    /// `(name, default)` per addon `RegisterCVar` that created a slot.
    pub(crate) cvar_registrations: Vec<(String, String)>,
    pub(crate) cvars_warned: HashSet<String>,
    /// The lowercased names a Lua `SetCVar` refuses, the reference's read-only flag bit2
    /// (`rec+0x1c & 4`). Kept beside the rows, so a host re-seed never clears it.
    pub(crate) cvars_read_only: HashSet<String>,

    /// The adapter's multisample formats in dropdown order, the reference's list of
    /// `{colorBits, depthBits, multisample}`.
    pub(crate) multisample_formats: Vec<super::cvars::MultisampleFormat>,

    /// This display's resolutions, ascending, behind `GetScreenResolutions`; empty until pushed.
    pub(crate) screen_modes: Vec<super::cvars::ScreenMode>,
    /// The live size's index in that list, which the host always includes; `None` while the list
    /// is empty. `GetCurrentResolution` answers it 1-based.
    pub(crate) current_resolution: Option<usize>,
    /// What this device and presentation path offer, behind `GetVideoCaps`.
    pub(crate) video_caps: super::cvars::VideoCaps,
    /// `RestartGx()` calls, the video window's apply button.
    pub(crate) restart_gx_asks: u32,

    /// Globals `RegisterForSave` declared, in order: written at logout and re-run at load.
    pub(crate) saved_names: Vec<String>,
    /// Globals `RegisterForSavePerCharacter` declared, in order: the character's file.
    pub(crate) saved_names_per_character: Vec<String>,
    /// Saved-variables files that failed to load; the shutdown write leaves them untouched.
    pub(crate) held_saved_files: Vec<std::path::PathBuf>,

    /// The key-binding table the Key Bindings window edits, with its account and character sets.
    pub(crate) keybinds: super::keybind::KeybindState,

    /// Action slots by Lua action id, 1..120.
    pub(crate) actions: HashMap<u32, ActionSlot>,
    /// The saved equipment sets by id, their names, which a set slot answers.
    pub(crate) equipment_sets: HashMap<u32, String>,
    /// `CurrencyTypes.dbc` and `CurrencyCategory.dbc`.
    pub(crate) currency_catalog: super::currency::CurrencyCatalogView,
    /// The player's currencies, `None` with no player.
    pub(crate) currency_holdings: Option<super::currency::CurrencyHoldings>,
    /// Categories closed by `ExpandCurrencyList`; every one starts open (`0x5afd90`).
    pub(crate) currency_collapsed: HashMap<u32, bool>,
    /// The equipment manager's saved sets and swap state.
    pub(crate) equipment_manager: super::equipment_manager::EquipmentManager,
    /// Equipment set sends, for the app.
    pub(crate) equipment_set_requests: Vec<super::equipment_manager::EquipmentSetRequest>,
    /// Per action, its usable, range, current and cooldown state; an absent one reads cold.
    pub(crate) action_states: HashMap<u32, super::action::StoredActionState>,
    pub(crate) bonus_bar_offset: u8,
    /// The temporary bar's owners (`CGActionBar::m_tempPageActiveFlags`).
    pub(crate) temp_action_bar: u32,
    /// The main bar's page, 0-based (`[0xc1e598]`): `GetActionBarPage` answers it plus one.
    pub(crate) action_bar_page: u8,
    /// `(action id, packed)` per slot `PickupAction`/`PlaceAction` changed, 0 clearing it: one
    /// `CMSG_SET_ACTION_BUTTON` each, so a drag swap is two sends.
    pub(crate) action_sets: Vec<(u32, u32)>,
    /// GlobalStrings keys of refusals this crate raises, fired by the app as `UI_ERROR_MESSAGE`
    /// (the reference's error display), each a literal from its table (stride `0x14`).
    pub(crate) ui_errors: Vec<&'static str>,

    pub(crate) spellbook: spellbook::SpellBookState,
    /// The book's highest-rank view, rebuilt with every [`Self::spellbook`] push.
    pub(crate) spellbook_highest_rank: spellbook::HighestRankView,
    /// The pet's spellbook, a second array as in the reference.
    pub(crate) pet_book: spellbook::PetBookState,
    /// The critter and mount lists (`0xbe8df4`).
    pub(crate) companions: super::companion::CompanionState,
    /// `DismissCompanion` and `SummonRandomCritter` asks, in call order, for the app.
    pub(crate) companion_intents: Vec<super::companion::CompanionIntent>,
    /// The player's macros, owned here as 1.12 macros have no server side; the app persists them.
    pub(crate) macros: macros::MacroState,
    /// A script changed the macros: the save and `UPDATE_MACROS` cue.
    pub(crate) macros_dirty: bool,
    /// Bumped by every seed and change, for readers that must not drain `macros_dirty`.
    pub(crate) macros_generation: u64,
    /// Each macro's cached cast by 1-based index, from the app; an absent macro reads unbound.
    pub(crate) macro_bindings: HashMap<u32, macros::MacroBinding>,
    /// The macro icon chooser's texture paths, behind `GetMacroIconInfo`.
    pub(crate) macro_icons: Vec<String>,
    /// Builds [`Self::macro_icons`] on the first `GetNumMacroIcons`, then is spent.
    pub(crate) macro_icon_source: Option<MacroIconSource>,
    /// The `INV_` icons, behind `GetMacroItemIconInfo` and the guild bank tab chooser.
    pub(crate) macro_item_icons: Vec<String>,
    /// Builds [`Self::macro_item_icons`] on first use, then is spent.
    pub(crate) macro_item_icon_source: Option<MacroIconSource>,
    /// `ToggleSpellAutocast` ids for `CMSG_PET_SPELL_AUTOCAST` (`0x2F3`), which names a spell.
    pub(crate) pet_spell_autocasts: Vec<u32>,
    /// An auto-repeat or cast that `SpellStopCasting()` can stop, not a channel. Its 1 or nil
    /// matters: the ESC chain (`UIParent.lua:1489`) reaches `CloseAllWindows()` on nil.
    pub(crate) casting: bool,
    /// Spell targeting is on (`SpellIsTargeting`); it gates `SpellStopTargeting()`, whose nil the
    /// ESC chain falls through on (`UIParent.lua:1490`).
    pub(crate) spell_targeting: bool,
    /// The units, by guid, for which `SpellCanTargetUnit`'s armed unit word clears fully, which
    /// stock unit frames ask before they call `SpellTargetUnit`; a token resolves to its guid
    /// through [`Self::unit_guids`].
    pub(crate) spell_targetable_units: HashSet<u64>,

    pub(crate) talents: super::talent::TalentUiState,
    /// The achievement tables, lists and tracked set.
    pub(crate) achievement: super::achievement::AchievementModel,
    /// The preview ranks per group, tab and talent, 0-based with -1 for none; reset to the learned
    /// ranks by every push.
    pub(crate) talent_preview: Vec<Vec<Vec<i32>>>,
    /// `LearnTalent` sends queued, `(talentId, rank)`.
    pub(crate) talent_learns: Vec<(u32, u32)>,
    /// `LearnPreviewTalents` sends queued, one `(talentId, rank)` list each.
    pub(crate) talent_preview_learns: Vec<Vec<(u32, u32)>>,
    /// The pet's talent snapshot: its family's tree and at most one group.
    pub(crate) pet_talents: super::talent::TalentUiState,
    /// The pet group's preview ranks, as [`Self::talent_preview`].
    pub(crate) pet_talent_preview: Vec<Vec<Vec<i32>>>,
    /// `LearnTalent(.., true)` sends queued, `(talentId, rank)`.
    pub(crate) pet_talent_learns: Vec<(u32, u32)>,
    /// `LearnPreviewTalents(true)` sends queued.
    pub(crate) pet_talent_preview_learns: Vec<Vec<(u32, u32)>>,
    /// `ConfirmTalentWipe()` calls, one `MSG_TALENT_WIPE_CONFIRM` each; the app holds the trainer.
    pub(crate) talent_wipe_confirms: u32,
    /// `CheckTalentMasterDist()`: the respec question is live and in range; false hides its dialog.
    pub(crate) talent_master_pending: bool,
    // ── The dialog engine's verbs ──
    /// `ConfirmPetUnlearn()` calls; the app holds the trainer and the money gate.
    pub(crate) pet_unlearn_confirms: u32,
    /// `CheckPetUntrainerDist()`: the pet trainer's question is pending and in reach.
    pub(crate) pet_untrainer_pending: bool,
    /// `GetInstanceBootTimeRemaining()` in whole seconds, from the `SMSG_RAID_GROUP_ONLY` deadline.
    pub(crate) instance_boot_secs: u32,
    /// A cached area spirit healer and seconds to its wave.
    pub(crate) area_spirit_healer_cached: bool,
    pub(crate) area_spirit_secs: u32,
    /// `AcceptAreaSpiritHeal()` calls, one `CMSG_AREA_SPIRIT_HEALER_QUEUE` each.
    pub(crate) area_spirit_accepts: u32,
    /// The battleground scoreboard: pushed rows, faction filter, sort order.
    pub(crate) battlefield_board: super::battlefield_score::ScoreBoard,
    /// `RequestBattlefieldScoreData()` calls.
    pub(crate) battlefield_score_requests: u32,
    /// The battlefield queue mirror: types, list, selection, slots and the sends.
    pub(crate) battlefield: super::battlefield_queue::BattlefieldInfo,
    /// Teammates' map positions, the flag carriers, the icon scale and position requests.
    pub(crate) battlefield_positions: Vec<super::battlefield_positions::BattlefieldPositionView>,
    pub(crate) battlefield_flags: Vec<super::battlefield_positions::BattlefieldFlagView>,
    pub(crate) battlefield_icon_scale: f32,
    /// A `Cell`: the stock `UIParent` OnUpdate calls the verb every frame and the app drains it
    /// every frame, and neither is anything drawn (`ModelCell`).
    pub(crate) battlefield_position_requests: std::cell::Cell<u32>,
    /// `CancelMeetingStoneRequest()` calls; the app gates on leadership.
    pub(crate) meeting_stone_cancels: u32,
    /// The meeting stone's queued area and status text, as pushed.
    pub(crate) meeting_stone_area: u32,
    pub(crate) meeting_stone_status_text: Option<String>,
    /// Acknowledged tutorials, `None` before `SMSG_TUTORIAL_FLAGS`; all `TutorialsEnabled()` reads.
    pub(crate) tutorial_bank: Option<Vec<u8>>,
    /// `FlagTutorial(n)` calls, 0-based and in range.
    pub(crate) tutorial_flag_requests: Vec<u32>,
    /// `TriggerTutorial(n)` calls, 0-based and in range.
    pub(crate) tutorial_triggers: Vec<u32>,
    pub(crate) tutorial_clears: u32,
    pub(crate) tutorial_resets: u32,

    /// The stance bar's forms, in bar order.
    pub(crate) shapeshift_forms: Vec<super::shapeshift::StoredShapeshiftForm>,
    /// The player's raw shapeshift form byte, 0 unshifted or no player.
    pub(crate) shapeshift_form_byte: u8,
    /// The rune table `GetRuneType` reads.
    pub(crate) runes: super::rune::RuneState,
    /// `GlyphProperties.dbc` and `GlyphSlot.dbc`.
    pub(crate) glyph_catalog: super::glyph::GlyphCatalogView,
    /// The player's glyph fields.
    pub(crate) glyph_sockets: super::glyph::GlyphSockets,
    /// The targeting cast's glyph reading, `None` while nothing targets.
    pub(crate) glyph_targeting: Option<super::glyph::GlyphTargeting>,
    /// `RemoveGlyphFromSocket` sends, 0-based sockets, for the app to send.
    pub(crate) glyph_removes: Vec<u32>,
    /// The socketing window's engine.
    pub(crate) socket_info: super::socket::SocketInfo,
    /// `AcceptSockets` sends, the item and a gem guid per socket, for the app.
    pub(crate) socket_sends: Vec<(u64, [u64; 3])>,
    /// The purchase refund records by item guid, the reference's `item+0x3d4`.
    pub(crate) refund_records: HashMap<u64, super::refund::RefundView>,
    /// A bound-but-tradeable item's `ITEM_FIELD_CREATE_PLAYED_TIME`, by guid, for an item the player
    /// owns with `ITEM_FIELD_FLAGS & 0x100` and no binding enchant.
    pub(crate) bound_trade_stamps: HashMap<u64, u32>,
    /// The played clock (`player+0x194c`, `+0x1950`): the total seconds the server last said and
    /// the `GetTime` second it arrived; `None` until then, when the clock reads 0 (`0x6cf440`).
    pub(crate) played_time: Option<(u32, f64)>,
    /// `ContainerRefundItemPurchase` sends, item guids, for the app.
    pub(crate) refund_sends: Vec<u64>,
    /// The open loot window's object (`[0xbfa8d8]`), 0 with none.
    pub(crate) loot_source: u64,
    /// `CharTitles.dbc`'s rows in file order.
    pub(crate) title_rows: Vec<super::title::TitleRow>,
    /// The active player's title fields, `None` with no player.
    pub(crate) player_titles: Option<super::title::PlayerTitles>,
    /// `SetCurrentTitle` sends, `CMSG_SET_TITLE` bodies, for the app to send.
    pub(crate) title_sets: Vec<i32>,
    pub(crate) title_throttle: super::title::TitleThrottle,
    /// What `GetSpellInfo` asks; absent before the app installs it.
    pub(crate) spell_info: Option<std::rc::Rc<dyn super::spell_info::SpellInfoSource>>,
    /// Each held unit's threat list.
    pub(crate) threat: super::threat::ThreatView,
    /// Each unit's passenger state and seat.
    pub(crate) vehicles: super::vehicle::VehicleView,
    /// The seat indicators by id.
    pub(crate) vehicle_indicators:
        std::sync::Arc<HashMap<u32, super::vehicle::VehicleIndicatorView>>,
    /// The vehicle verbs' requests, for the app to send.
    pub(crate) vehicle_requests: Vec<super::vehicle::VehicleRequest>,
    /// `s_vehicleAimNormPower`, 0..1.
    pub(crate) vehicle_aim_power: f32,
    /// The four totem slots and their tools.
    pub(crate) totems: super::totem::TotemSlots,
    /// `DestroyTotem` calls on live slots, 0-based, for the app to send.
    pub(crate) totem_destroys: Vec<u8>,
    /// The totem bar's element lists and the tables its slot test reads.
    pub(crate) multicast: super::multicast::MultiCastView,
    /// Keyed refusals with one `%s`, shown by the app like [`Self::ui_errors`].
    pub(crate) ui_errors_s: Vec<(&'static str, String)>,

    /// The pet bar's ten slots and two bar-wide bits, replaced whole by every `SMSG_PET_SPELLS`.
    pub(crate) pet_bar: super::pet::PetBarState,
    /// 1-based slot indices `TogglePetAutocast` queued.
    pub(crate) pet_autocast_toggles: Vec<u32>,
    /// `PetStopAttack()` calls.
    pub(crate) pet_stop_attacks: u32,
    /// `HasFullControl`, the reference's control flag: set by `SMSG_CLIENT_CONTROL_UPDATE` for the
    /// player, boots 1, and every cast, item and cursor gate refuses at 0.
    pub(crate) player_control: bool,
    /// One `CMSG_PET_SET_ACTION` per entry, its one or two `(position, packed word)` pairs: the
    /// server tells the forms apart by body size, so a move's two pairs must not be flattened.
    pub(crate) pet_set_actions: Vec<Vec<(u32, u32)>>,
    /// `PetAbandon()` and `PetDismiss()` calls, counted apart though both end at one opcode.
    pub(crate) pet_abandons: u32,
    pub(crate) pet_dismisses: u32,
    /// Names `PetRename` queued, from the `PETRENAMECONFIRM` popup.
    pub(crate) pet_renames: Vec<String>,

    /// Bag contents by API bag id (0 is the backpack).
    pub(crate) containers: HashMap<i64, container::ContainerState>,
    /// Per `(bag, slot)`, `(start, duration, enabled)` in `GetTime` seconds, stamped at push.
    pub(crate) container_cooldowns: HashMap<(i64, u32), (f64, f64, bool)>,
    /// `HasKey()`: a keys-family item in equipment, bags, bank or keyring; gates the keyring UI.
    pub(crate) has_key: bool,
    /// What the cursor carries; nothing moves locally, the server's updates settle the bags.
    pub(crate) cursor: Option<cursor::CursorPayload>,
    /// The action bar's drop grid: shown on the cursor's empty-to-held edge for any payload but a
    /// pet action or a vendor row (mode 5; the grab setter shows it for mode 7 only).
    pub(crate) cursor_grid_shown: bool,
    /// The pet bar's grid, lit only by a pet-action pickup; never both grids at once.
    pub(crate) pet_grid_shown: bool,
    /// The world pick under the cursor (`[this+0x350]`): a world click drops no payload on an
    /// object, only an item on terrain, and anything over nothing.
    pub(crate) world_pick: cursor::WorldPick,
    /// Held-item bag moves, sent as `CMSG_SWAP_INV_ITEM` or `CMSG_SPLIT_ITEM`.
    pub(crate) container_moves: Vec<container::ContainerMove>,
    /// `(bag, slot)` clicks made in repair mode.
    pub(crate) container_repairs: Vec<(i64, u32)>,
    /// Targeting wants an item: a bag or paper-doll click becomes a pick.
    pub(crate) item_pick_armed: bool,
    /// Picked `(bag, slot)`s; a paper-doll click is `EQUIPMENT_BAG` and its 1-based inventory slot.
    pub(crate) item_picks: Vec<(i64, u32)>,
    /// `BindEnchant()` and `ReplaceEnchant()`, the enchant popups' answers.
    pub(crate) enchant_confirms: Vec<cursor::EnchantConfirm>,
    /// `DeleteCursorItem`'s `(bag, slot, count)`, 0 for the whole stack, as `CMSG_DESTROYITEM`.
    pub(crate) container_destroys: Vec<(i64, u32, u32)>,
    /// The wrapping paper a right-click armed (cursor mode 2, no packet). Not a cursor payload: the
    /// reference sets only the cursor mode, so `CursorHasItem()` stays nil.
    pub(crate) pending_wrap: Option<container::PendingWrap>,
    /// `(giftBag, giftSlot, itemBag, itemSlot)` per completed wrap, in `CMSG_WRAP_ITEM`'s order.
    pub(crate) container_wraps: Vec<(i64, u32, i64, u32)>,
    /// The cursor mode Lua set; `ResetCursor` returns it to the world's mode.
    pub(crate) ui_cursor: Option<container::UiCursorMode>,
    /// A FrameXML cursor call ran. The mode is a write, not a level: the reference keeps its value
    /// until the world or a hover handler writes it, so a handler-less frame leaves it be.
    pub(crate) ui_cursor_dirty: bool,
    /// `AutoEquipCursorItem`'s `(bag, slot)` sources, sent as `CMSG_AUTOEQUIP_ITEM`.
    pub(crate) container_autoequips: Vec<(i64, u32)>,
    /// `PutItemInBag` and `PutItemInBackpack` stores, sent as `CMSG_AUTOSTORE_BAG_ITEM`, or
    /// `CMSG_SPLIT_ITEM` for a split stack.
    pub(crate) bag_autostores: Vec<container::BagAutoStore>,

    /// `RegisterForDrag` buttons per frame, compared case-insensitively. Nothing destroys a frame,
    /// so this, `scripts` and `frame_events` are never pruned; a destroy path must prune all three.
    pub(crate) drag_registered: HashMap<FrameHandle, HashSet<String>>,
    /// The drag gesture, armed by a press on a drag-registered frame, started past the threshold.
    pub(crate) drag: Option<cursor::DragGesture>,
    /// The one `StartMoving()` in flight, the client's `root+0xcfc` slot, apart from `drag` as in
    /// the reference: a move outlives the button, since mouse-up's auto-stop skips a Lua drag.
    pub(crate) moving: Option<super::object::FrameMove>,
    /// The `StartSizing` in flight, cleared by `StopMovingOrSizing`.
    pub(crate) sizing: Option<super::object::FrameSizing>,
    /// A Slider thumb drag, left press to release or pointer leave, handled here with no Lua.
    pub(crate) slider_drag: Option<slider::SliderDrag>,
    /// A ColorSelect wheel or value-bar drag, press to release.
    pub(crate) color_drag: Option<colorselect::ColorDrag>,

    /// The open gossip menu, the `SelectGossipOption` calls (position, code) and whether
    /// `CloseGossip` ran.
    pub(crate) gossip: Option<gossip::GossipMenu>,
    pub(crate) gossip_selects: Vec<(u32, Option<String>)>,
    pub(crate) gossip_close: bool,
    /// The open menu's greeting query is out: selects do nothing.
    pub(crate) gossip_pending: bool,
    /// `SelectGossipQuest` 1-based rows, each sent as `CMSG_QUESTGIVER_QUERY_QUEST` for its quest.
    pub(crate) gossip_quest_selects: Vec<u32>,

    /// The open vendor's stock, the `BuyMerchantItem` calls and whether `CloseMerchant` ran.
    pub(crate) merchant: Option<merchant::MerchantState>,
    /// `GetRepairAllCost`'s total; 0 unless a repairer is open.
    pub(crate) repair_all_cost: u32,
    /// `GetRepairAllCost`'s second return: a swept item is damaged.
    pub(crate) repair_all_damaged: bool,
    /// The repair costs `SetInventoryItem` and `SetBagItem` return.
    pub(crate) repair_costs: merchant::RepairCosts,
    pub(crate) merchant_buys: Vec<(u32, u32)>,
    /// The held `(bag, slot)` when `PickupMerchantItem` sells, sent as `CMSG_SELL_ITEM`.
    pub(crate) merchant_cursor_sells: Vec<(i64, u32)>,
    /// A held vendor row dropped in a bag, `(bag, slot, 1-based row)`, for `CMSG_BUY_ITEM_IN_SLOT`.
    pub(crate) merchant_slot_buys: Vec<(i64, u32, u32)>,
    pub(crate) merchant_close: bool,
    /// `BuybackItem` 1-based slots, the `RepairAllItems` flag and repair mode (`InRepairMode`).
    pub(crate) merchant_buybacks: Vec<u32>,
    pub(crate) repair_all: Option<bool>,
    pub(crate) repair_mode: bool,

    /// The stable window: snapshot, selection, drag and queued calls, reset together on close.
    pub(crate) stable: stable::StableModel,

    /// The open bank's slot-purchase row and calls; its contents are bags -1 and 5..=11.
    pub(crate) bank: Option<bank::BankState>,
    pub(crate) bank_purchase: bool,
    pub(crate) bank_close: bool,

    /// The open trainer, its buys, selection and close. The selection is a spell id, since
    /// filters, collapses and re-lists all move row numbers.
    pub(crate) trainer: Option<trainer::TrainerState>,
    pub(crate) trainer_buys: Vec<u32>,
    pub(crate) trainer_selection: Option<u32>,
    pub(crate) trainer_close: bool,
    /// Collapsed skill-line ids (the complement of the expanded set), reset per list.
    pub(crate) trainer_collapsed: HashSet<u32>,
    /// Skill-line ids `SetTrainerSkillLineFilter` turned off, reset per list.
    pub(crate) trainer_lines_off: HashSet<u32>,

    /// The open taxi map, the `TakeTaxiNode` calls, whether `CloseTaxiMap` ran and whether we ride.
    pub(crate) taxi: Option<taxi::TaxiUiState>,
    pub(crate) taxi_takes: Vec<usize>,
    pub(crate) taxi_close: bool,
    pub(crate) taxi_riding: bool,

    /// The open tradeskill window, its filters and repeat.
    pub(crate) trade_skill: tradeskill::TradeSkillModel,

    /// The dungeon finder.
    pub(crate) lfg: lfg::LfgModel,

    /// The calendar.
    pub(crate) calendar: calendar::CalendarModel,

    /// `EquipPendingItem`/`CancelPendingEquip` answers in order, and `ConfirmBindOnUse()` calls.
    pub(crate) pending_equip_answers: Vec<bind_confirm::PendingEquipAnswer>,
    pub(crate) bind_on_use_confirms: u32,

    /// The open loot, the `LootSlot` takes and whether `CloseLoot` ran, with its `noUI` flag.
    pub(crate) loot: Option<loot::LootState>,
    pub(crate) loot_picks: Vec<u32>,
    /// `ConfirmLootSlot(slot)` rows, 1-based, for its pending-slot gate (`0x589300`).
    pub(crate) loot_confirms: Vec<u32>,
    pub(crate) loot_close: Option<bool>,
    /// `GiveMasterLoot(slot, candidateIndex)`, both 1-based display positions the app translates.
    pub(crate) loot_master_gives: Vec<(u32, u32)>,

    /// The open group-loot rolls, and the `RollOnLoot` `(roll_id, roll_type)` votes.
    pub(crate) loot_rolls: loot_roll::LootRollsState,
    /// The combat log's entries, cursor and filters.
    pub(crate) combat_log: combat_log::CombatLogStore,
    pub(crate) loot_roll_votes: Vec<(u32, u8)>,
    /// Need or Greed on a bind-on-pickup roll: a `CONFIRM_LOOT_ROLL` popup ask, not a vote.
    pub(crate) loot_roll_confirms: Vec<(u32, u8)>,
    /// `AcceptLevelGrant` (true) and `DeclineLevelGrant` (false) calls.
    pub(crate) level_grant_answers: Vec<bool>,
    /// The server's UI clock less the local clock, seconds; 0 until it is read.
    pub(crate) ui_time_offset: i32,

    /// The item-text reader: the pushed page, then the close and page-turn calls.
    pub(crate) item_text: Option<item_text::ItemTextState>,
    pub(crate) item_text_close: bool,
    pub(crate) item_text_page_turns: Vec<i32>,
    /// The inbox, rows 1-based, kept past the window's close as the reference's inbox is.
    pub(crate) mail: mail::MailState,
    /// A mailbox is open.
    pub(crate) mail_mailbox: bool,
    /// The mail verbs' packets, in call order.
    pub(crate) mail_commands: Vec<mail::MailCommand>,
    /// A mail command awaits its result; most verbs wait it out.
    pub(crate) mail_command_pending: bool,
    /// A send awaits its result.
    pub(crate) mail_send_pending: bool,
    /// `CheckInbox`'s next allowed list query, on the `GetTime` clock.
    pub(crate) mail_next_update: Option<f64>,
    pub(crate) mail_send_money: u32,
    pub(crate) mail_send_cod: u32,
    /// The Send tab's attachments by send slot, each resolved to its guid when the send fires.
    pub(crate) mail_send_items: [Option<cursor::CursorItem>; 16],
    /// `SetSendMailShowing`: the Send tab is up, so a bag click attaches.
    pub(crate) mail_send_showing: bool,
    /// The refund-lock prompt is up.
    pub(crate) mail_lock_send_items: bool,
    /// `GetLatestThreeSenders`' answer, one entry per delivered sender.
    pub(crate) mail_latest_senders: Vec<Option<String>>,
    /// The usable stationery, in the picker's order.
    pub(crate) mail_stationeries: Vec<mail::StationeryView>,
    /// `SelectStationery`'s `Stationery.dbc` id; 0 is none, which silences `SendMail`.
    pub(crate) mail_stationery: u32,
    /// `Package.dbc`'s rows in file order, `GetPackageInfo`'s list.
    pub(crate) mail_packages: Vec<mail::PackageView>,
    /// `SelectPackage`'s `Package.dbc` id; 0 is none.
    pub(crate) mail_package: u32,
    /// `HasNewMail()` (`0x56f350`), from `MSG_QUERY_NEXT_MAIL_TIME` and `SMSG_RECEIVED_MAIL`.
    pub(crate) has_new_mail: bool,

    /// The open auction house and the auction calls. `auction_selected` is each list's selected
    /// auction id, 0 for none (`0x59b2e0`); `auction_sort` each list's criteria, front first.
    pub(crate) auction: Option<auction::AuctionState>,
    /// The Browse tab's class tree, pushed once per login.
    pub(crate) auction_item_classes: Vec<auction::AuctionCategory>,
    pub(crate) auction_selected: [u32; 3],
    pub(crate) auction_sort: [Vec<(u8, bool)>; 3],
    /// The query gate per list and the get-all gate, the app's throttle.
    pub(crate) auction_can_query: [bool; 3],
    pub(crate) auction_can_get_all: bool,
    /// `SetAuctionsTabShowing`, which routes a bag right-click to the sell slot.
    pub(crate) auction_tab_showing: bool,
    /// The slots a multi-sell holds locked.
    pub(crate) auction_locked: Vec<(i64, u32)>,
    pub(crate) auction_query: Option<auction::AuctionQuery>,
    pub(crate) auction_owner_query: bool,
    pub(crate) auction_bidder_query: Option<u32>,
    pub(crate) auction_bids: Vec<auction::AuctionBid>,
    pub(crate) auction_cancels: Vec<u32>,
    pub(crate) auction_start: Option<auction::AuctionStartRequest>,
    pub(crate) auction_cancel_sell: bool,
    pub(crate) auction_close: bool,
    /// The sell slot's item, resolved to its guid when `StartAuction` fires.
    pub(crate) auction_sell_item: Option<cursor::CursorItem>,

    /// The open trade and its calls: `InitiateTrade` tokens, the accept, unaccept and cancel flags,
    /// and `SetTradeMoney` copper (`CMSG_SET_TRADE_GOLD`).
    pub(crate) trade: Option<trade::TradeState>,
    pub(crate) trade_initiates: Vec<String>,
    pub(crate) trade_accept: bool,
    pub(crate) trade_unaccept: bool,
    pub(crate) trade_close: bool,
    /// `BeginTrade()` and `CancelTrade()` calls.
    pub(crate) trade_begin: bool,
    pub(crate) trade_cancel: bool,
    pub(crate) trade_set_money: Option<u32>,
    /// Items dropped on our slots, `(1-based trade slot, bag, slot)`, and the slots clicks cleared.
    pub(crate) trade_set_items: Vec<(u32, i64, u32)>,
    pub(crate) trade_clear_items: Vec<u32>,

    /// The open questgiver panel, its greeting-row selects and its button calls.
    pub(crate) quest: Option<quest::QuestState>,
    pub(crate) quest_selects: Vec<quest::QuestSelect>,
    pub(crate) quest_actions: Vec<quest::QuestAction>,

    /// Death countdowns and offers, and the release, reclaim and resurrect calls.
    pub(crate) death: death::DeathUiState,
    pub(crate) death_actions: Vec<death::DeathAction>,

    /// The quest log, selection (1-based), abandon mark (a quest id) and abandons.
    pub(crate) quest_log: quest_log::QuestLogState,
    pub(crate) quest_log_selection: u32,
    /// The reward-faction list, refilled by a quest panel's publish or
    /// `ProcessQuestLogRewardFactions` (`0x58d050`).
    pub(crate) quest_reward_factions: Vec<(u32, i32)>,
    pub(crate) quest_log_abandon_mark: u32,
    pub(crate) quest_log_abandons: Vec<u32>,
    /// `QuestLogPushQuest()` quest ids, resolved at the click so a log change cannot retarget one.
    pub(crate) quest_log_pushes: Vec<u32>,
    /// `ConfirmAcceptQuest()` calls, the escort confirm; the app holds the pending quest.
    pub(crate) quest_confirms: u32,
    /// Item id to its tooltip view.
    pub(crate) item_templates: HashMap<u32, ItemTemplateView>,
    /// Item ids asked for and missing, for the app to answer.
    pub(crate) item_stat_asks: HashSet<u32>,
    /// Set id to its tooltip set block, and the misses asked for.
    pub(crate) item_sets: HashMap<u32, super::ItemSetView>,
    pub(crate) item_set_asks: HashSet<u32>,
    /// `ItemRandomProperties` id to its view, pushed whole at startup: a clicked tooltip cannot
    /// repaint on a late answer. The reference reads its loaded store.
    pub(crate) random_properties: HashMap<u32, super::RandomPropertyView>,
    /// `ItemRandomSuffix.dbc`'s names, the suffix a negative random-property id takes.
    pub(crate) random_suffixes: HashMap<u32, String>,
    /// The player's level, class, race and skills, for a tooltip's red requirement lines.
    pub(crate) player_req: PlayerReqState,
    /// Spell id to its tooltip view, and the misses asked for.
    pub(crate) spell_tooltips: HashMap<u32, super::SpellTooltipView>,
    pub(crate) spell_tooltip_asks: HashSet<u32>,
    /// The same for the views built against the player's pet, which the spell builder's unit
    /// selector asks for (the spell builder's fourth argument).
    pub(crate) pet_spell_tooltips: HashMap<u32, super::SpellTooltipView>,
    pub(crate) pet_spell_tooltip_asks: HashSet<u32>,
    /// The tooltips whose spell render missed its view, re-rendered when the app answers.
    pub(crate) spell_tooltip_waits: HashMap<FrameHandle, super::tooltip_spell::SpellWait>,
    /// The tooltips whose item setter missed a record, re-run when it lands.
    pub(crate) item_tooltip_waits: HashMap<FrameHandle, super::tooltip_item::ItemWait>,
    /// The item setter call running, collecting what it misses.
    pub(crate) item_render: Option<super::tooltip_item::ItemRender>,
    /// `CollapseQuestHeader`/`ExpandQuestHeader` as `(1-based entry, collapse)`, entry 0 for all.
    pub(crate) quest_log_collapses: Vec<(u32, bool)>,
    /// The quest watches, the POI store and the special items the objective tracker reads.
    pub(crate) quest_watch: super::quest_watch::QuestWatchState,
    /// The animation groups and animations, and the translation each region draws at.
    pub(crate) anims: super::anim::AnimStore,
    /// The server's clock in Unix seconds, `None` before `SMSG_QUERY_TIME_RESPONSE`. A quest
    /// deadline is a stamp in it, and `GetQuestTimers` subtracts per call, as the reference does.
    /// A `Cell`: pushed every frame, read by the quest timer verbs alone (`ModelCell`).
    pub(crate) server_unix_time: std::cell::Cell<Option<f64>>,
    /// The daily quest reset in server unix seconds (`0x5dff40`).
    pub(crate) quest_reset_unix: std::cell::Cell<Option<f64>>,
    /// `GetMoney()` in copper, from `PLAYER_FIELD_COINAGE`.
    pub(crate) money: u64,

    /// `GetNetStats`'s third return, the average round trip in ms; 0 before any sample.
    pub(crate) net_latency_ms: u32,

    /// `UnitXP("player")` and `UnitXPMax("player")`, from the private `PLAYER_XP` fields.
    pub(crate) player_xp: u32,
    pub(crate) player_next_level_xp: u32,

    /// Our combo points and the unit they sit on (`SMSG_UPDATE_COMBO_POINTS`), ungated: a warrior
    /// banks them too (Overpower); `GetComboPoints` (`0x611670`) applies the class gate.
    pub(crate) combo_points: u8,
    pub(crate) combo_target: u64,
    /// The pet's, from `SMSG_PET_UPDATE_COMBO_POINTS`.
    pub(crate) pet_combo_points: u8,
    pub(crate) pet_combo_target: u64,

    /// `PLAYER_BYTES_2` byte 3 (1 rested, 2 normal), defaulting to 2 as every 1.12 character has
    /// it: a 0 has no Exhaustion row, and `GetRestState()`'s nil would raise at the unguarded
    /// `>= 3` of `MainMenuBar.lua:25`. `rest_pool` is in base kill-XP units, and `resting` is
    /// `PLAYER_FLAGS_RESTING` (0x20).
    pub(crate) rest_state: u8,
    pub(crate) rest_pool: u32,
    pub(crate) resting: bool,

    /// `PLAYER_FLAGS` bit 12, `PartialPlayTime()` (`0x51cb30`), an anti-addiction regime's flag.
    pub(crate) partial_play_time: bool,

    /// `PLAYER_FLAGS` bit 13, `NoPlayTime()` (`0x51cba0`), the same regime's harsher half.
    pub(crate) no_play_time: bool,

    /// `GetBillingTimeRested()` (`0x511c40`) in minutes from `SMSG_AUTH_RESPONSE`, unconverted.
    pub(crate) billing_time_rested: u32,
    /// `PLAYER_FLAGS` bit 25, the server's no-XP flag, `IsXPUserDisabled()` (`0x614140`).
    pub(crate) xp_user_disabled: bool,
    /// The account's expansion byte from `SMSG_AUTH_RESPONSE`, `GetAccountExpansionLevel()`.
    pub(crate) account_expansion: u8,
    /// `InCinematic()`. While set, `StaticPopup_Show` refuses any dialog without
    /// `interruptCinematic` (`StaticPopup.lua:1454`).
    pub(crate) in_cinematic: bool,
    /// `Exhaustion.dbc` by rest byte, `(name, factor)`: `GetRestState` indexes it and row 1 scales
    /// `GetXPExhaustion` (`0x51b960`). enUS rows seed it until the app loads the install's own.
    pub(crate) exhaustion: HashMap<u8, (String, f64)>,

    /// The paper doll's combat stats, `None` until the first push.
    pub(crate) player_combat_stats: Option<char_stats::UnitCombatStats>,
    /// The pet's combat stats, which the reference's pet sheet reads as `UnitStat("pet", …)`.
    pub(crate) pet_combat_stats: Option<char_stats::UnitCombatStats>,
    pub(crate) inventory_slots: char_stats::InventorySlots,
    /// The seven bank-bag slots, ids 68..=74, fed from the player descriptor's guids.
    pub(crate) bank_bag_slots: char_stats::BankBagSlots,
    /// `GetInventoryAlertStatus` in `0xa1ce4c` order; a change fires `UPDATE_INVENTORY_ALERTS`.
    pub(crate) inventory_alerts: [u8; 12],
    /// Equipped slot ids clicked while the merchant repair cursor is armed.
    pub(crate) inventory_repairs: Vec<u32>,
    /// Main- and off-hand temporary enchants in `GetWeaponEnchantInfo`'s order, pushed each frame.
    pub(crate) weapon_enchants: [Option<weapon_enchant::WeaponEnchant>; 2],

    /// The inspected unit's gear, from its public `PLAYER_VISIBLE_ITEM_*` fields.
    pub(crate) inspect: Option<inspect::InspectView>,
    /// `NotifyInspect` unit tokens, sent as `CMSG_INSPECT`.
    pub(crate) inspect_notifies: Vec<String>,
    /// `ClearInspectPlayer` ran; the app drops its inspect target.
    pub(crate) inspect_clear: bool,
    /// `DressUpModel` `SetUnit`, `Dress`, `Undress` and `TryOn` calls, applied in call order.
    pub(crate) dressup_intents: Vec<super::dressup::DressUpIntent>,
    /// Tabard designs per `TabardModel`, the app's preview five, the host facts and queued calls.
    pub(crate) tabard_designs: HashMap<crate::widget::FrameHandle, [i32; 5]>,
    pub(crate) tabard_preview: Option<[i32; 5]>,
    pub(crate) tabard_host: super::tabard::TabardHost,
    pub(crate) tabard_intents: Vec<super::tabard::TabardIntent>,
    /// The barber shop's host facts, preview dials and queued calls.
    pub(crate) barber: super::barber::BarberState,
    /// The `WorldFrame` type's one-shot record: true once the first is made.
    pub(crate) world_frame_made: bool,
    /// Per live-unit guid, reach for `CanInspect`/`CheckInteractDistance`; absent: nil.
    pub(crate) unit_reach: HashMap<u64, super::UnitReach>,

    /// The skills pane's snapshot and the display tree built from it.
    pub(crate) skills: skills::SkillsState,
    pub(crate) skills_groups: Vec<skills::SkillGroup>,
    /// Collapsed skill categories by id, kept across a re-push.
    pub(crate) skills_collapsed: HashSet<u32>,
    /// The selection as a skill id, not a row.
    pub(crate) skills_selected: Option<u32>,
    /// `AbandonSkill` ids for `CMSG_UNLEARN_SKILL`; nothing changes until the server's update.
    pub(crate) skill_abandons: Vec<u32>,

    /// The reputation pane's snapshot, and the client's faction list and header tree over it.
    pub(crate) reputation: reputation::ReputationState,
    pub(crate) reputation_tree: reputation::FactionTree,
    /// Reputation calls queued, applied locally first because none of the three sends is acked.
    pub(crate) reputation_sends: Vec<reputation::ReputationSend>,

    /// Lines for the app's slash-command parser: a probe's, or a host `SlashCmdList` row's.
    pub(crate) chat_input: Vec<String>,

    /// The world map's pushed catalog and feed, and its selection.
    pub(crate) worldmap: super::worldmap::WorldMapState,
    /// Nameplate `Button`s under `WorldFrame`, never shrunk, visible to `GetChildren()`.
    pub(crate) nameplates: super::nameplate::NamePlates,
    /// The world-state readout's rows, gated and resolved by the app.
    pub(crate) worldstate: super::worldstate::WorldStateUiState,
    /// Events bindings raise. Deviation: fired at the next tick, not synchronously as in the
    /// reference, because a binding runs inside Lua and cannot re-enter handler dispatch.
    pub(crate) pending_events: Vec<(String, Vec<ScriptValue>)>,
    /// The last cursor position in UI space (logical px, y-up), behind `GetCursorPosition()`.
    pub(crate) cursor_pos: (f32, f32),

    /// A `Minimap:PingLocation(x, y)`, drained the same frame: centre-relative offsets in UI units,
    /// `GetCursorPosition`'s space, not the window pixels of the app's minimap geometry.
    pub(crate) minimap_ping_request: Option<(f32, f32)>,
    /// The ping's offsets as fractions of the minimap's side, updated by the app every frame.
    /// `GetPingPosition()` always answers both, from statics never cleared.
    pub(crate) minimap_ping: (f32, f32),
    /// `SendChatMessage` lines, apart from chat-box input, whose drain runs the slash grammar.
    pub(crate) chat_sends: Vec<super::chat_send::ChatSend>,
    /// `SendAddonMessage` broadcasts: `LANG_ADDON`, four distributions, `prefix` TAB `message`.
    pub(crate) addon_sends: Vec<super::addon_message::AddonSend>,
    /// `RequestTimePlayed()` calls, one empty `CMSG_PLAYED_TIME` each.
    pub(crate) played_time_asks: u32,
    /// `OpeningCinematic()` calls, one empty `CMSG_OPENING_CINEMATIC` each.
    pub(crate) opening_cinematic_asks: u32,
    /// `Screenshot()` calls, one capture each.
    pub(crate) screenshot_asks: u32,
    /// `GetRealmName()`: `""` until pushed, never nil, since addons index tables with it at load.
    pub(crate) realm_name: String,
    /// The local player record, seeded at world entry before addons load, then kept by `"player"`
    /// pushes that carry it: corrected, never blanked. A per-login rebuild is unseen, as its verbs
    /// exist only in game, not at the glue screen.
    pub(crate) player_record: super::PlayerRecord,
    /// `GetBindLocation()`, the bind point's area name: `""` before it lands, never nil.
    pub(crate) bind_location: String,
    /// `GMTicketCategory.dbc` `(id, name)` rows in file order; the ids are wire values.
    pub(crate) gm_ticket_categories: Vec<(u32, String)>,
    /// GM ticket calls in call order, so a delete then a get reach the wire in that order.
    pub(crate) gm_ticket_intents: Vec<super::gm_ticket::GmTicketIntent>,
    /// `Stuck()` calls, each a cast of spell 7355, the Help window's Auto-Unstuck.
    pub(crate) stuck_casts: u32,
    /// `ConfirmBinder()` calls, one `CMSG_BINDER_ACTIVATE` each; the app holds the innkeeper.
    pub(crate) binder_confirms: u32,
    /// `CheckBinderDist()`: the innkeeper's question is live and in range; false hides its dialog.
    pub(crate) binder_pending: bool,
    /// The summon getters' answers, defaulting to the reference's `""`, `""`, 0 with none pending.
    pub(crate) summon_confirm: super::summon::SummonConfirmUiState,
    /// `ConfirmSummon()` calls, one `CMSG_SUMMON_RESPONSE` each; the app holds the summoner.
    pub(crate) summon_confirms: u32,
    /// `GetFramerate()`, pushed each tick.
    /// A `Cell`: pushed every frame, read by `GetFramerate()` alone (`ModelCell`).
    pub(crate) framerate: std::cell::Cell<f64>,
    /// `(shift, ctrl, alt)` for `Is*KeyDown`, pushed before the frame's mouse events.
    pub(crate) modifiers: (bool, bool, bool),
    /// The button of the click whose `PreClick`/`OnClick`/`PostClick` is running, which a
    /// button-qualified modified click compares (`[0xb499a8]+0x1234`).
    pub(crate) click_button: Option<String>,
    /// The player state `SecureCmdOptionParse`'s conditions read beside the unit snapshots.
    pub(crate) option_state: super::OptionState,
    /// Condition names `SecureCmdOptionParse` did not know, for `ERR_UNKNOWN_MACRO_OPTION_S`.
    pub(crate) unknown_macro_options: Vec<String>,
}

impl Model {
    /// The selected quest-log row's detail, resolved per call as the reference's bindings do.
    pub(crate) fn selected_quest_detail(&self) -> Option<&quest_log::QuestLogDetail> {
        let index = usize::try_from(self.quest_log_selection.checked_sub(1)?).ok()?;
        self.quest_log.entries.get(index)?.detail.as_ref()
    }
}

impl Model {
    /// A named font object, matched case-insensitively (ASCII) as the client's compare does.
    pub(crate) fn font_object(&self, name: &str) -> Option<&FontObject> {
        if name.bytes().any(|b| b.is_ascii_uppercase()) {
            self.font_objects_by_lower.get(&name.to_ascii_lowercase())
        } else {
            self.font_objects_by_lower.get(name)
        }
    }

    /// Records a caught script error for the host, the retained log and the Lua error handler; an
    /// error raised by the handler itself goes back to the handler alone, never through here.
    pub(crate) fn record_script_error(&mut self, msg: String) {
        self.diagnostics
            .record(super::diagnostics::DiagnosticKind::Error, &msg);
        self.pending_error_dispatch.push(msg.clone());
        self.errors.push(msg);
    }

    /// Records a non-fatal warning for the host's terminal and the retained diagnostic log.
    pub(crate) fn record_warning(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.diagnostics
            .record(super::diagnostics::DiagnosticKind::Warning, &msg);
        self.warnings.push(msg);
    }

    /// A warning for the host's terminal only, for a message already retained under another kind.
    pub(crate) fn warn_host_only(&mut self, msg: String) {
        self.warnings.push(msg);
    }

    /// The state of the unit `token` names, as the `Unit*` getters read it: the snapshot the app
    /// pushed under the token, else, for a token with a `target` hop, the chain's end.
    pub(crate) fn unit(&self, token: &str) -> Option<&UnitState> {
        if token.eq_ignore_ascii_case("focus") {
            return self.focus_unit();
        }
        if token.eq_ignore_ascii_case("vehicle") {
            return self.vehicle_unit();
        }
        self.pushed_unit(token).or_else(|| self.chained_unit(token))
    }

    /// The focus's unit by guid, as the getters look its object up: the app's `focus` push while it
    /// names that guid, else whichever snapshot holds it, so `FocusUnit` resolves the frame it
    /// runs.
    fn focus_unit(&self) -> Option<&UnitState> {
        let guid = self.unit_guids.focus;
        if guid == 0 || guid == super::focus::NOBODY {
            return None;
        }
        self.units_by_lower
            .get("focus")
            .filter(|u| u.guid == guid)
            .or_else(|| self.units_by_guid.get(&guid))
            .or_else(|| {
                self.units_by_lower
                    .values()
                    .find(|u| u.exists && u.guid == guid)
            })
    }

    /// The seated player's vehicle by guid, as the getters look its object up (`0x468460`):
    /// whichever snapshot holds it, the per-guid feed's ([`super::UnitGuids::chain_ends`]) first.
    fn vehicle_unit(&self) -> Option<&UnitState> {
        let guid = self.unit_guids.vehicle;
        if guid == 0 {
            return None;
        }
        self.units_by_guid.get(&guid).or_else(|| {
            self.units_by_lower
                .values()
                .find(|u| u.exists && u.guid == guid)
        })
    }

    /// The snapshot pushed under `token` itself, folded as 1.12's resolver folds (`_strnicmp`:
    /// `A`..`Z` only, never a byte of 0x80 or more), so `to_ascii_lowercase`, never `to_lowercase`.
    pub(crate) fn pushed_unit(&self, token: &str) -> Option<&UnitState> {
        if token.bytes().any(|b| b.is_ascii_uppercase()) {
            self.units_by_lower.get(&token.to_ascii_lowercase())
        } else {
            self.units_by_lower.get(token)
        }
    }

    /// The unit a token with a `target` hop and no push of its own names: the resolver's guid
    /// (which the getters call before their own object lookup), then that guid's snapshot. A token
    /// without a hop is the app's to push, base by base, and reads nothing here.
    fn chained_unit(&self, token: &str) -> Option<&UnitState> {
        match parse_unit_token(token) {
            UnitTokenParse::Unit { base, hops } if hops > 0 => {
                let guid = self.unit_guids.resolve_unit(base, hops)?;
                self.units_by_guid.get(&guid)
            }
            _ => None,
        }
    }

    pub(crate) fn new() -> Model {
        Model {
            vm: super::glue::VmKind::InGame,
            addons: Vec::new(),
            addon_index: Vec::new(),
            addon_info_hidden: None,
            addon_enable: Default::default(),
            addon_enable_saved: Default::default(),
            addons_root: None,
            addons_chain_reader: None,
            measurer: None,
            texture_probe: None,
            world_ui_visible: true,
            texture_size_probe: None,
            font_probe: None,
            addons_saved_account: None,
            addons_saved_character: None,
            framexml_templates: Default::default(),
            framexml_debug: Default::default(),
            load_log: Default::default(),
            framexml_fonts: Default::default(),
            arena: WidgetArena::new(),
            layout_inputs: Default::default(),
            solver: LayoutSolver::new(),
            layout_fingerprint: None,
            layout_epoch: 0,
            layout_touched: None,
            measure_dirty: None,
            msg_swept: std::collections::HashMap::new(),
            msg_lines_hashed: 0,
            layout_verify_recheck: false,
            layout_derives: 0,
            layout_scope: super::layout::LayoutScope::default(),
            layout_last_scope: (0, 0),
            layout_epoch_resolved: None,
            layout_solves: 0,
            layout_gate_walks: 0,
            layout_rounds: 0,
            resolved: Default::default(),
            link_spans: Default::default(),
            region_data: Default::default(),
            backdrops: Default::default(),
            simple_html: simplehtml::SimpleHtmlStates::new(),
            font_objects_by_lower: HashMap::new(),
            region_resolved: Default::default(),
            next_id: 1,
            id_to_frame: IdMap::default(),
            frame_to_id: Default::default(),
            id_to_region: IdMap::default(),
            region_to_id: Default::default(),
            region_names: HashMap::new(),
            region_name_by_id: HashMap::new(),
            scripts: HashMap::new(),
            on_update_frames: Vec::new(),
            walk_roster: 0,
            on_size_changed_frames: Vec::new(),
            on_update_model_frames: Vec::new(),
            xml_loading: HashSet::new(),
            model_facts: HashMap::new(),
            model_facts_wanted: Vec::new(),
            model_instances: 0,
            event_to_frames: HashMap::new(),
            frame_events: HashMap::new(),
            all_event_frames: Vec::new(),
            focused_editbox: None,
            mouseover: None,
            hover_repick: false,
            mouse_down_on: HashMap::new(),
            mouse_buttons_down: 0,
            mouse_capture: None,
            last_click: HashMap::new(),
            pending_size_changed: Vec::new(),
            errors: Vec::new(),
            pending_error_dispatch: Vec::new(),
            diagnostics: Default::default(),
            warnings: Vec::new(),
            // 1024x768 until the host calls `set_screen_size`; y-up `[bottom, left, top, right]`.
            screen: Rect::new(0.0, 0.0, 768.0, 1024.0),
            units_by_lower: HashMap::new(),
            units_by_guid: HashMap::new(),
            player_auras: Vec::new(),
            unit_auras: HashMap::new(),
            unit_guids: Default::default(),
            unit_casts: Default::default(),
            cancel_aura_requests: Vec::new(),
            minimap_tracking: Default::default(),
            script_calls: Vec::new(),
            input: Default::default(),
            drop_item_on_unit: Vec::new(),
            joined_channels: Vec::new(),
            channel_pane: Default::default(),
            party: party::PartyState::default(),
            difficulty: Default::default(),
            lfg_restricted: false,
            refer_a_friend: Default::default(),
            party_requests: Vec::new(),
            party_lines: Vec::new(),
            ready_check: party::ReadyCheckState::default(),
            saved_instances: Vec::new(),
            raid_selection: 0,
            now: 0.0,
            game_time: (0, 0),
            zone: Default::default(),
            instance_type: None,
            can_reset_instances: false,
            reset_instance_asks: 0,
            instance_lock: Default::default(),
            instance_lock_responses: Vec::new(),
            dynamic_difficulty_maps: Vec::new(),
            social: social::SocialState::default(),
            social_requests: Vec::new(),
            guild: guild::GuildState::default(),
            guild_control: guild::GuildRankEdit::default(),
            guild_requests: Vec::new(),
            guild_bank: Default::default(),
            guild_bank_tab: 0,
            guild_bank_locks: Vec::new(),
            guild_bank_requests: Vec::new(),
            petition: petition::PetitionState::default(),
            petition_requests: Vec::new(),
            petition_vendor: Default::default(),
            // Per window, as the stock dock differs: 1 and 2 are the dock, the rest undocked.
            chat_window_looks: (0..chat_window::ENGINE_CHAT_WINDOW_SLOTS)
                .map(chat_window::ChatWindowLook::stock)
                .collect(),
            chat_window_changes: HashSet::new(),
            chat_colors: super::chat_types::seed(),
            chat_colors_changed: false,
            known_languages: Vec::new(),
            class_names: Vec::new(),
            language_table: Vec::new(),
            zone_channel_catalog: Vec::new(),
            channel_commands: Vec::new(),
            guild_recruitment_mode: 1,
            guild_recruitment_changed: false,
            emote_requests: Vec::new(),
            roll_requests: Vec::new(),
            console_lines: Vec::new(),
            logging_chat: false,
            logging_combat: false,
            logging_changed: false,
            default_language: None,
            duel_requests: Vec::new(),
            mirror_timers: Default::default(),
            follow_requests: Vec::new(),
            camera_view_requests: Vec::new(),
            session_requests: Vec::new(),
            glue: Default::default(),
            pvp_toggles: 0,
            pvp_sets: Vec::new(),
            pvp_toggle_timer: 0,
            inspect_arena_teams: Default::default(),
            arena_teams: Default::default(),
            honor: None,
            inspect_honor: None,
            inspect_honor_requests: 0,
            inspect_honor_pending: false,
            helm_shown: true,
            cloak_shown: true,
            worn_display_toggles: Vec::new(),
            action_bar_toggles: None,
            action_bar_toggle_sends: Vec::new(),
            sound_queue: Vec::new(),
            music_queue: Vec::new(),
            sound_suppression: 0,
            sound_output_drivers: None,
            sound_restart: false,
            cvars: HashMap::new(),
            map_info: None,
            cvars_saved_base: HashMap::new(),
            cvar_changes: Vec::new(),
            cvar_registrations: Vec::new(),
            cvars_warned: HashSet::new(),
            cvars_read_only: HashSet::new(),
            multisample_formats: Vec::new(),
            screen_modes: Vec::new(),
            current_resolution: None,
            video_caps: super::cvars::VideoCaps::default(),
            restart_gx_asks: 0,
            saved_names: Vec::new(),
            saved_names_per_character: Vec::new(),
            held_saved_files: Vec::new(),
            keybinds: super::keybind::KeybindState::default(),
            actions: HashMap::new(),
            equipment_sets: HashMap::new(),
            currency_catalog: Default::default(),
            currency_holdings: None,
            currency_collapsed: HashMap::new(),
            equipment_manager: Default::default(),
            equipment_set_requests: Vec::new(),
            action_states: HashMap::new(),
            bonus_bar_offset: 0,
            temp_action_bar: 0,
            action_bar_page: 0,
            action_sets: Vec::new(),
            ui_errors: Vec::new(),
            spellbook: spellbook::SpellBookState::default(),
            spellbook_highest_rank: spellbook::HighestRankView::default(),
            pet_book: spellbook::PetBookState::default(),
            companions: super::companion::CompanionState::default(),
            companion_intents: Vec::new(),
            macros: macros::MacroState::default(),
            macros_dirty: false,
            macros_generation: 0,
            macro_bindings: HashMap::new(),
            macro_icons: Vec::new(),
            macro_icon_source: None,
            macro_item_icons: Vec::new(),
            macro_item_icon_source: None,
            pet_spell_autocasts: Vec::new(),
            casting: false,
            spell_targeting: false,
            spell_targetable_units: HashSet::new(),
            talents: super::talent::TalentUiState::default(),
            achievement: Default::default(),
            talent_preview: Vec::new(),
            talent_learns: Vec::new(),
            talent_preview_learns: Vec::new(),
            pet_talents: Default::default(),
            pet_talent_preview: Vec::new(),
            pet_talent_learns: Vec::new(),
            pet_talent_preview_learns: Vec::new(),
            talent_wipe_confirms: 0,
            talent_master_pending: false,
            pet_unlearn_confirms: 0,
            pet_untrainer_pending: false,
            instance_boot_secs: 0,
            area_spirit_healer_cached: false,
            area_spirit_secs: 0,
            area_spirit_accepts: 0,
            battlefield_board: Default::default(),
            battlefield_score_requests: 0,
            battlefield: Default::default(),
            battlefield_positions: Vec::new(),
            battlefield_flags: Vec::new(),
            battlefield_icon_scale: 1.0,
            battlefield_position_requests: std::cell::Cell::new(0),
            meeting_stone_cancels: 0,
            meeting_stone_area: 0,
            meeting_stone_status_text: None,
            tutorial_bank: None,
            tutorial_flag_requests: Vec::new(),
            tutorial_triggers: Vec::new(),
            tutorial_clears: 0,
            tutorial_resets: 0,
            shapeshift_forms: Vec::new(),
            shapeshift_form_byte: 0,
            runes: Default::default(),
            glyph_catalog: Default::default(),
            glyph_sockets: Default::default(),
            glyph_targeting: None,
            glyph_removes: Vec::new(),
            socket_info: Default::default(),
            socket_sends: Vec::new(),
            refund_records: HashMap::new(),
            bound_trade_stamps: HashMap::new(),
            played_time: None,
            refund_sends: Vec::new(),
            loot_source: 0,
            title_rows: Vec::new(),
            player_titles: None,
            title_sets: Vec::new(),
            title_throttle: Default::default(),
            spell_info: None,
            threat: Default::default(),
            vehicles: Default::default(),
            vehicle_indicators: Default::default(),
            vehicle_requests: Vec::new(),
            vehicle_aim_power: 0.0,
            totems: Default::default(),
            totem_destroys: Vec::new(),
            multicast: Default::default(),
            ui_errors_s: Vec::new(),
            pet_bar: super::pet::PetBarState::default(),
            pet_autocast_toggles: Vec::new(),
            pet_stop_attacks: 0,
            player_control: true,
            pet_set_actions: Vec::new(),
            pet_abandons: 0,
            pet_dismisses: 0,
            pet_renames: Vec::new(),
            containers: HashMap::new(),
            container_cooldowns: HashMap::new(),
            has_key: false,
            cursor: None,
            cursor_grid_shown: false,
            pet_grid_shown: false,
            world_pick: cursor::WorldPick::default(),
            container_moves: Vec::new(),
            container_repairs: Vec::new(),
            item_pick_armed: false,
            item_picks: Vec::new(),
            enchant_confirms: Vec::new(),
            container_destroys: Vec::new(),
            pending_wrap: None,
            container_wraps: Vec::new(),
            ui_cursor: None,
            ui_cursor_dirty: false,
            container_autoequips: Vec::new(),
            bag_autostores: Vec::new(),
            drag_registered: HashMap::new(),
            drag: None,
            moving: None,
            sizing: None,
            slider_drag: None,
            color_drag: None,
            gossip: None,
            gossip_selects: Vec::new(),
            gossip_close: false,
            gossip_pending: false,
            gossip_quest_selects: Vec::new(),
            merchant: None,
            repair_all_cost: 0,
            repair_all_damaged: false,
            repair_costs: Default::default(),
            merchant_buys: Vec::new(),
            merchant_cursor_sells: Vec::new(),
            merchant_slot_buys: Vec::new(),
            merchant_close: false,
            merchant_buybacks: Vec::new(),
            repair_all: None,
            repair_mode: false,
            stable: Default::default(),
            bank: None,
            bank_purchase: false,
            bank_close: false,
            trainer: None,
            trainer_buys: Vec::new(),
            trainer_selection: None,
            trainer_close: false,
            trainer_collapsed: HashSet::new(),
            trainer_lines_off: HashSet::new(),
            taxi: None,
            taxi_takes: Vec::new(),
            taxi_close: false,
            taxi_riding: false,
            trade_skill: Default::default(),
            lfg: Default::default(),
            calendar: Default::default(),
            loot: None,
            pending_equip_answers: Vec::new(),
            bind_on_use_confirms: 0,
            loot_picks: Vec::new(),
            loot_confirms: Vec::new(),
            loot_close: None,
            loot_master_gives: Vec::new(),
            loot_rolls: loot_roll::LootRollsState::default(),
            combat_log: combat_log::CombatLogStore::default(),
            loot_roll_votes: Vec::new(),
            loot_roll_confirms: Vec::new(),
            level_grant_answers: Vec::new(),
            ui_time_offset: 0,
            item_text: None,
            item_text_close: false,
            item_text_page_turns: Vec::new(),
            mail: mail::MailState::default(),
            mail_mailbox: false,
            mail_commands: Vec::new(),
            mail_command_pending: false,
            mail_send_pending: false,
            mail_next_update: None,
            mail_send_money: 0,
            mail_send_cod: 0,
            mail_send_items: Default::default(),
            mail_send_showing: false,
            mail_lock_send_items: false,
            mail_latest_senders: Vec::new(),
            mail_stationeries: Vec::new(),
            mail_stationery: 0,
            mail_packages: Vec::new(),
            mail_package: 0,
            has_new_mail: false,
            auction: None,
            auction_item_classes: Vec::new(),
            auction_selected: [0; 3],
            auction_sort: auction::initial_sort_criteria(),
            auction_can_query: [true; 3],
            auction_can_get_all: true,
            auction_tab_showing: false,
            auction_locked: Vec::new(),
            auction_query: None,
            auction_owner_query: false,
            auction_bidder_query: None,
            auction_bids: Vec::new(),
            auction_cancels: Vec::new(),
            auction_start: None,
            auction_cancel_sell: false,
            auction_close: false,
            auction_sell_item: None,
            trade: None,
            trade_initiates: Vec::new(),
            trade_accept: false,
            trade_unaccept: false,
            trade_close: false,
            trade_begin: false,
            trade_cancel: false,
            trade_set_money: None,
            trade_set_items: Vec::new(),
            trade_clear_items: Vec::new(),
            quest: None,
            quest_selects: Vec::new(),
            quest_actions: Vec::new(),
            death: death::DeathUiState::default(),
            death_actions: Vec::new(),
            quest_log: quest_log::QuestLogState::default(),
            quest_log_selection: 0,
            quest_reward_factions: Vec::new(),
            quest_log_abandon_mark: 0,
            quest_log_abandons: Vec::new(),
            quest_log_pushes: Vec::new(),
            quest_confirms: 0,
            item_templates: HashMap::new(),
            item_sets: HashMap::new(),
            item_set_asks: HashSet::new(),
            random_properties: HashMap::new(),
            random_suffixes: HashMap::new(),
            item_stat_asks: HashSet::new(),
            player_req: PlayerReqState::default(),
            spell_tooltips: HashMap::new(),
            spell_tooltip_asks: HashSet::new(),
            pet_spell_tooltips: HashMap::new(),
            pet_spell_tooltip_asks: HashSet::new(),
            spell_tooltip_waits: HashMap::new(),
            item_tooltip_waits: HashMap::new(),
            item_render: None,
            quest_log_collapses: Vec::new(),
            quest_watch: Default::default(),
            anims: Default::default(),
            server_unix_time: std::cell::Cell::new(None),
            quest_reset_unix: std::cell::Cell::new(None),
            worldmap: super::worldmap::WorldMapState::default(),
            nameplates: super::nameplate::NamePlates::default(),
            worldstate: super::worldstate::WorldStateUiState::default(),
            pending_events: Vec::new(),
            cursor_pos: (0.0, 0.0),
            minimap_ping_request: None,
            minimap_ping: (0.0, 0.0),
            chat_sends: Vec::new(),
            addon_sends: Vec::new(),
            played_time_asks: 0,
            opening_cinematic_asks: 0,
            screenshot_asks: 0,
            realm_name: String::new(),
            player_record: super::PlayerRecord::default(),
            bind_location: String::new(),
            gm_ticket_categories: Vec::new(),
            gm_ticket_intents: Vec::new(),
            stuck_casts: 0,
            binder_confirms: 0,
            binder_pending: false,
            summon_confirm: super::summon::SummonConfirmUiState::default(),
            summon_confirms: 0,
            framerate: std::cell::Cell::new(0.0),
            modifiers: (false, false, false),
            click_button: None,
            option_state: Default::default(),
            unknown_macro_options: Vec::new(),
            money: 0,
            net_latency_ms: 0,
            player_xp: 0,
            player_next_level_xp: 0,
            combo_points: 0,
            combo_target: 0,
            pet_combo_points: 0,
            pet_combo_target: 0,
            rest_state: 2,
            rest_pool: 0,
            resting: false,
            partial_play_time: false,
            no_play_time: false,
            billing_time_rested: 0,
            xp_user_disabled: false,
            account_expansion: 0,
            in_cinematic: false,
            exhaustion: [
                (1, ("Rested".to_string(), 2.0)),
                (2, ("Normal".to_string(), 1.0)),
                (3, ("XXXTired".to_string(), 1.0)),
                (4, ("XXXTired".to_string(), 0.5)),
                (5, ("XXXExhausted".to_string(), 0.25)),
            ]
            .into_iter()
            .collect(),
            player_combat_stats: None,
            pet_combat_stats: None,
            inventory_slots: Default::default(),
            bank_bag_slots: Default::default(),
            inventory_alerts: [0; 12],
            inventory_repairs: Vec::new(),
            weapon_enchants: [None; 2],
            inspect: None,
            inspect_notifies: Vec::new(),
            inspect_clear: false,
            dressup_intents: Vec::new(),
            tabard_designs: HashMap::new(),
            tabard_preview: None,
            tabard_host: Default::default(),
            tabard_intents: Vec::new(),
            barber: Default::default(),
            world_frame_made: false,
            unit_reach: HashMap::new(),
            chat_input: Vec::new(),
            skills: skills::SkillsState::default(),
            skills_groups: Vec::new(),
            skills_collapsed: HashSet::new(),
            skills_selected: None,
            skill_abandons: Vec::new(),
            reputation: reputation::ReputationState::default(),
            reputation_tree: reputation::FactionTree::default(),
            reputation_sends: Vec::new(),
        }
    }

    /// Name region `id` in the registry a named `relativeTo` resolves through: the first name
    /// wins, per name and per region.
    pub(crate) fn name_region(&mut self, name: String, id: u32) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.region_names.entry(name) {
            self.region_name_by_id
                .entry(id)
                .or_insert_with(|| e.key().clone());
            e.insert(id);
        }
    }

    /// Mint (or fetch) the stable id of a frame handle.
    pub(crate) fn frame_id(&mut self, h: FrameHandle) -> u32 {
        if let Some(&id) = self.frame_to_id.get(&h) {
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.frame_to_id.insert(h, id);
        self.id_to_frame.insert(id, h);
        // Every creation path mints here: the cached graph adopts the frame, and the gate opens.
        self.touch_layout_node(id);
        id
    }

    /// Mint (or fetch) the stable id of a region handle.
    pub(crate) fn region_id(&mut self, h: RegionHandle) -> u32 {
        if let Some(&id) = self.region_to_id.get(&h) {
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.region_to_id.insert(h, id);
        self.id_to_region.insert(id, h);
        // A new region joins the cached graph once anchored ([`Self::touch_layout_node`]).
        self.touch_layout_node(id);
        id
    }

    /// A layout write that names no node; a missed call is caught by `WOW_LAYOUT_VERIFY`.
    pub(crate) fn touch_layout(&mut self) {
        // Naming nothing, the next resolve derives the graph in full: slow, never wrong.
        self.give_up_naming();
        self.bump_layout_epoch();
    }

    /// A write that moved one region's offsets, size or measured extent, not its targets or
    /// liveness; a region missing from the cached graph falls back to `touch_layout`.
    pub(crate) fn touch_layout_region(&mut self, rh: RegionHandle) {
        match self.region_to_id.get(&rh) {
            Some(&id) => self.touch_layout_node(id),
            None => self.touch_layout(),
        }
    }

    /// The same for a frame: an anchor offset or size, nothing that moves an edge or the roster.
    pub(crate) fn touch_layout_frame(&mut self, h: FrameHandle) {
        match self.frame_to_id.get(&h) {
            Some(&id) => self.touch_layout_node(id),
            None => self.touch_layout(),
        }
    }

    /// A `SetParent` under `root`: only the subtree's scale moves, as anchor targets are ids fixed
    /// at `SetPoint`. Addon pools reparent hundreds of frames a redraw, so this stays precise.
    pub(crate) fn touch_layout_reparent(&mut self, root: FrameHandle) {
        let mut subtree = vec![root];
        let mut at = 0;
        while at < subtree.len() {
            let h = subtree[at];
            at += 1;
            if let Some(f) = self.arena.frame(h) {
                subtree.extend(f.children.iter().copied());
            }
        }
        // Sync the new scale here, and the clamp insets it multiplies: only a full derivation
        // syncs them, and a named node skips that.
        for &h in &subtree {
            self.sync_layout_input(h);
        }
        for h in subtree {
            self.touch_layout_frame(h);
        }
    }

    /// A write changed frame `h`'s anchor targets; `old` and `new` are its full target lists, so
    /// the cached edges are re-pointed. Falls back to `touch_layout` when the scope refuses.
    pub(crate) fn touch_layout_retarget_frame(&mut self, h: FrameHandle, old: &[u32], new: &[u32]) {
        match self.frame_to_id.get(&h).copied() {
            // Not in the graph yet: adopting it takes its current anchors' edges whole.
            Some(id) if self.layout_touched.is_some() && !self.layout_scope.has_node(id) => {
                self.touch_layout_node(id);
            }
            // Already conservative: the next resolve rebuilds every edge, so patching is wasted.
            Some(id)
                if self.layout_touched.is_some() && self.layout_scope.retarget(id, old, new) =>
            {
                self.touch_layout_node(id);
            }
            _ => self.touch_layout(),
        }
    }

    /// `touch_layout_retarget_frame` for a region.
    pub(crate) fn touch_layout_retarget_region(
        &mut self,
        rh: RegionHandle,
        old: &[u32],
        new: &[u32],
    ) {
        match self.region_to_id.get(&rh).copied() {
            Some(id) if self.layout_touched.is_some() && !self.layout_scope.has_node(id) => {
                self.touch_layout_node(id);
            }
            Some(id)
                if self.layout_touched.is_some() && self.layout_scope.retarget(id, old, new) =>
            {
                self.touch_layout_node(id);
            }
            _ => self.touch_layout(),
        }
    }

    /// Stores a region's text and names it on the measure ledger; on the layout ledger too when
    /// the string turns empty or back, as the layout reads no measure for empty text.
    pub(crate) fn set_region_text(&mut self, rh: RegionHandle, text: Option<String>) {
        let d = self.region_data.entry(rh).or_default();
        let had = d.laid_measure().is_some();
        d.text = text;
        let has = d.laid_measure().is_some();
        self.touch_measure(rh);
        if had != has {
            self.touch_layout_region(rh);
        }
    }

    /// Names `rh` on the measure ledger for a write that may move its measure key.
    pub(crate) fn touch_measure(&mut self, rh: RegionHandle) {
        if let Some(list) = &mut self.measure_dirty {
            list.push(rh);
        }
    }

    /// A measure-key write that names no region, such as a font object: the next sweep walks all.
    pub(crate) fn touch_measure_all(&mut self) {
        self.measure_dirty = None;
    }

    /// Names `id` as what this write moved, while naming is still on. A node the cached graph
    /// lacks is adopted as the full derivation would hold it; one it cannot place gives up naming.
    fn touch_layout_node(&mut self, id: u32) {
        if self.layout_touched.is_some() {
            match self.adopt_layout_node(id) {
                Adopt::Node => {
                    if let Some(list) = &mut self.layout_touched {
                        list.push(id);
                    }
                }
                // An unanchored region is no node of either path: there is nothing to name.
                Adopt::NotANode => {}
                Adopt::Refused => self.give_up_naming(),
            }
        }
        self.bump_layout_epoch();
    }

    /// A ScrollFrame's child or offsets changed: the derivation anchors a live ScrollFrame's live
    /// child TOPLEFT to its TOPLEFT at `(horizontal, vertical)` in place of its own anchors. `old`
    /// returns to its anchors and the current child takes the override at the current offsets, each
    /// named; anything the cached graph cannot re-point gives up naming.
    pub(crate) fn touch_scroll_child(&mut self, scroll: FrameHandle, old: Option<FrameHandle>) {
        if self.layout_touched.is_none() {
            self.bump_layout_epoch();
            return;
        }
        let state = match self.arena.frame(scroll).map(|f| &f.kind_state) {
            Some(crate::widget::KindState::Scroll(s)) => (s.child, s.horizontal, s.vertical),
            _ => return self.touch_layout(),
        };
        let (child, horizontal, vertical) = state;
        let live = |m: &Self, h: Option<FrameHandle>| h.filter(|&h| m.arena.frame(h).is_some());
        let scroll_id = self.frame_id(scroll);
        let mut named = Vec::new();
        let mut ok = true;
        if let Some(oc) = live(self, old).filter(|&h| Some(h) != child) {
            let id = self.frame_id(oc);
            let anchors = self.layout_inputs.entry(oc).or_default().anchors.clone();
            ok &= self.layout_scope.set_override(id, None, &anchors);
            named.push(id);
        }
        if let Some(nc) = live(self, child) {
            let id = self.frame_id(nc);
            let anchors = self.layout_inputs.entry(nc).or_default().anchors.clone();
            let over = crate::layout::Anchor::new(
                crate::layout::Point::TopLeft,
                scroll_id,
                crate::layout::Point::TopLeft,
                horizontal,
                vertical,
            );
            ok &= self.layout_scope.set_override(id, Some(over), &anchors);
            named.push(id);
        }
        if !ok {
            return self.touch_layout();
        }
        for id in named {
            self.touch_layout_node(id);
        }
        self.bump_layout_epoch();
    }

    /// Sync frame `h`'s layout input from the arena as the full derivation does: its effective
    /// scale, clamp-to-screen with its insets, and the window extent a clamped frame keeps inside.
    pub(crate) fn sync_layout_input(&mut self, h: FrameHandle) {
        let Some(f) = self.arena.frame(h) else {
            return;
        };
        let (scale, clamp, insets) = (f.effective_scale, f.clamped_to_screen, f.clamp_rect_insets);
        let screen = self.screen;
        let input = self.layout_inputs.entry(h).or_default();
        input.scale = scale;
        input.clamp = clamp;
        input.clamp_insets = insets.map(|v| v * scale);
        if clamp {
            input.extent_x = screen.right;
            input.extent_y = screen.top;
        }
    }

    /// Put `id` in the cached graph if it is not there: every live frame, with its anchor edges
    /// and its layout input synced from the arena as the derivation syncs it, and every live,
    /// anchored region, with its anchor and owner edges. A ScrollFrame's scroll child is refused,
    /// as its edge comes from the override, not its anchors.
    fn adopt_layout_node(&mut self, id: u32) -> Adopt {
        if self.layout_scope.has_node(id) {
            return Adopt::Node;
        }
        if let Some(&h) = self.id_to_frame.get(&id) {
            let Some(f) = self.arena.frame(h) else {
                return Adopt::Refused;
            };
            let scroll_child = f.parent.and_then(|p| self.arena.frame(p)).is_some_and(
                |p| matches!(&p.kind_state, crate::widget::KindState::Scroll(s) if s.child == Some(h)),
            );
            if scroll_child {
                return Adopt::Refused;
            }
            self.sync_layout_input(h);
            let Model {
                layout_inputs,
                layout_scope,
                ..
            } = self;
            layout_scope.adopt_frame(h, id, layout_inputs.entry(h).or_default());
            return Adopt::Node;
        }
        if let Some(&rh) = self.id_to_region.get(&id) {
            let Some(r) = self.arena.region(rh) else {
                return Adopt::Refused;
            };
            let anchors = match self.region_data.get(&rh) {
                Some(d) if !d.anchors.is_empty() => d.anchors.clone(),
                _ => return Adopt::NotANode,
            };
            let owner = (r.owner, self.frame_to_id.get(&r.owner).copied());
            let is_fontstring = matches!(r.kind, crate::widget::RegionKind::FontString);
            self.layout_scope
                .adopt_region(rh, id, owner, is_fontstring, &anchors);
            return Adopt::Node;
        }
        Adopt::Refused
    }

    /// Every write that gives up naming comes through here, so the next resolve derives in full.
    /// `WOW_LAYOUT_DERIVE_TRACE=<secs>:<n>` backtraces the first `n` of these after `secs`.
    fn give_up_naming(&mut self) {
        if self.layout_touched.is_some() && derive_trace_armed() {
            eprintln!(
                "[layout-derive] the ledger gave up naming, at:\n{}",
                std::backtrace::Backtrace::force_capture()
            );
        }
        self.layout_touched = None;
    }

    /// Bumps tier 1's counter, reopening the gate; `layout_touched` decides how much it derives.
    pub(crate) fn bump_layout_epoch(&mut self) {
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
        // `WOW_LAYOUT_TOUCH_TRACE=<secs>:<n>`: backtraces the first `n` touches after `secs`.
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::OnceLock;
        static SPEC: OnceLock<Option<(std::time::Instant, f64, AtomicU32)>> = OnceLock::new();
        let spec = SPEC.get_or_init(|| {
            let v = std::env::var("WOW_LAYOUT_TOUCH_TRACE").ok()?;
            let (secs, n) = v.split_once(':')?;
            Some((
                std::time::Instant::now(),
                secs.trim().parse().ok()?,
                AtomicU32::new(n.trim().parse().ok()?),
            ))
        });
        if let Some((t0, delay, left)) = spec {
            if t0.elapsed().as_secs_f64() >= *delay
                && left
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1))
                    .is_ok()
            {
                eprintln!(
                    "[layout-touch] epoch={} at:\n{}",
                    self.layout_epoch,
                    std::backtrace::Backtrace::force_capture()
                );
            }
        }
    }
}

/// What [`Model::adopt_layout_node`] made of an id.
enum Adopt {
    /// In the cached graph, now or already.
    Node,
    /// A live region with no anchors, which neither the derivation nor the ledger holds.
    NotANode,
    /// Not placeable here: naming gives up and the next resolve derives in full.
    Refused,
}

/// `WOW_LAYOUT_DERIVE_TRACE=<secs>:<n>`: armed after `secs`, past the UI load, then `n` prints.
fn derive_trace_armed() -> bool {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::OnceLock;
    static SPEC: OnceLock<Option<(std::time::Instant, f64, AtomicU32)>> = OnceLock::new();
    let Some((t0, delay, left)) = SPEC.get_or_init(|| {
        let v = std::env::var("WOW_LAYOUT_DERIVE_TRACE").ok()?;
        let (secs, n) = v.split_once(':')?;
        Some((
            std::time::Instant::now(),
            secs.trim().parse().ok()?,
            AtomicU32::new(n.trim().parse().ok()?),
        ))
    }) else {
        return false;
    };
    t0.elapsed().as_secs_f64() >= *delay
        && left
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1))
            .is_ok()
}

// ── The mutation counter ──

/// [`Model`] behind a mutation counter. Every `&mut Model` taken through the VM's app data (a
/// binding's `app_data_mut`, the host's `UiScript::model_mut`) comes through [`DerefMut`] and
/// bumps `epoch`; a `&Model` never does. So a frame whose count stands where the last extract
/// left it has nothing new to draw, and `UiScript::extract` can be skipped outright
/// (`UiScript::visual_epoch`). The host's own per-frame bookkeeping, where it proves it changed
/// nothing drawn (the clock, a drained empty queue, a settled solve), puts the count back with
/// [`ModelCell::set_epoch`]; a write that is not provably invisible keeps its bump, so a miss
/// costs one walk, never a stale frame.
pub(crate) struct ModelCell {
    inner: Model,
    epoch: u64,
    /// Paint-only writes, which leave `epoch` standing: the targets they touched.
    pub(crate) paint: std::cell::RefCell<PaintMarks>,
    /// The last full extract's per-target entry ranges ([`ExtractIndex`]).
    pub(crate) extract_index: std::cell::RefCell<Option<ExtractIndex>>,
}

/// The most marks one patch walk takes; past it the extract walks in full.
const MAX_PAINT_MARKS: usize = 512;

/// The targets paint-only writes touched since the last extract, and their running count.
#[derive(Default)]
pub(crate) struct PaintMarks {
    /// Every paint-only write, monotone: the app's gate compares it frame to frame.
    pub(crate) count: u64,
    targets: Vec<crate::order::ZTarget>,
    overflow: bool,
}

impl PaintMarks {
    /// Forget the marks a full walk has read; the count stands.
    pub(crate) fn clear_marks(&mut self) {
        self.targets.clear();
        self.overflow = false;
    }

    /// The marks for a patch walk, cleared; `None` when they overflowed.
    pub(crate) fn take_marks(&mut self) -> Option<Vec<crate::order::ZTarget>> {
        let overflow = std::mem::take(&mut self.overflow);
        let targets = std::mem::take(&mut self.targets);
        (!overflow).then_some(targets)
    }
}

/// Where each draw target's entries sit in the last full extract, under the mutation count it was
/// walked at, so a paint-only frame re-extracts a few targets into that list.
pub(crate) struct ExtractIndex {
    pub(crate) generation: u64,
    pub(crate) epoch: u64,
    /// Target → (first entry, entry count, its draw key).
    pub(crate) entries:
        crate::widget::HandleMap<crate::order::ZTarget, (u32, u32, crate::order::ZKey)>,
    pub(crate) len: usize,
    /// No target appeared twice, so each has one range.
    pub(crate) unique: bool,
}

impl ModelCell {
    pub(crate) fn new(inner: Model) -> ModelCell {
        ModelCell {
            inner,
            epoch: 1,
            paint: Default::default(),
            extract_index: Default::default(),
        }
    }

    /// The count as of now; equal counts mean no mutable borrow in between.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Put the count back to `epoch`, by a caller that took a mutable borrow since and changed
    /// nothing the extract reads.
    pub(crate) fn set_epoch(&mut self, epoch: u64) {
        self.epoch = epoch;
    }

    /// The model for a write that changes only how `targets` paint (an alpha, a tint): it leaves
    /// the mutation count standing and marks them, so the extract re-reads those targets alone.
    /// A write that moves anything else (layout, visibility, order, text) takes the plain borrow.
    pub(crate) fn paint_mut(&mut self, targets: &[crate::order::ZTarget]) -> &mut Model {
        let marks = self.paint.get_mut();
        marks.count = marks.count.wrapping_add(1);
        if !marks.overflow {
            if marks.targets.len() + targets.len() > MAX_PAINT_MARKS {
                marks.overflow = true;
                marks.targets.clear();
            } else {
                marks.targets.extend_from_slice(targets);
            }
        }
        &mut self.inner
    }
}

impl std::ops::Deref for ModelCell {
    type Target = Model;
    fn deref(&self) -> &Model {
        &self.inner
    }
}

impl std::ops::DerefMut for ModelCell {
    fn deref_mut(&mut self) -> &mut Model {
        self.epoch = self.epoch.wrapping_add(1);
        &mut self.inner
    }
}
