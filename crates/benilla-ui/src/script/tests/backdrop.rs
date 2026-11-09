//! The backdrop Lua verbs and extract emission (the reference's Backdrop object).

use super::common::script;
use crate::script::*;

// SetBackdropColor tints only the bg, SetBackdropBorderColor the 8 border pieces.
#[test]
fn backdrop_installs_and_extracts_pieces_with_colors() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Plate")
        f:SetPoint("TOPLEFT", nil, "TOPLEFT", 100, -100)
        f:SetWidth(200); f:SetHeight(100)
        f:SetBackdrop({
            bgFile = "bg", edgeFile = "edge", tile = true,
            tileSize = 16, edgeSize = 16,
            insets = { left = 5, right = 5, top = 5, bottom = 5 },
        })
        f:SetBackdropColor(0.09, 0.09, 0.19)
        f:SetBackdropBorderColor(1, 1, 1)
    "#,
    )
    .unwrap();
    s.resolve();
    let pieces: Vec<(String, [f32; 4])> = s
        .extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Backdrop { path, color, .. } => Some((path, color)),
            _ => None,
        })
        .collect();
    // bg (1) + 8 border pieces.
    assert_eq!(pieces.len(), 9);
    // The colour is a packed `0xAARRGGBB` byte quad, stored as `×255 + 0.5` through `__ftol`
    // (`SetBackdropColor`), so 0.09 reads back as 23/255.
    assert_eq!(pieces[0].0, "bg");
    let q = |x: f32| f32::from((x * 255.0 + 0.5) as u8) / 255.0;
    assert_eq!(pieces[0].1, [q(0.09), q(0.09), q(0.19), 1.0]);
    assert!(pieces[1..]
        .iter()
        .all(|(p, c)| p == "edge" && *c == [1.0, 1.0, 1.0, 1.0]));
}

// The four traps of `GetBackdrop()`, as an addon observes them.
#[test]
fn get_backdrop_reconstructs_from_the_struct() {
    let s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "GB")

        -- How many values did that return? The implicit vararg table's `n` answers.
        local function count(...) return arg.n end

        -- TRAP 2: no backdrop => ZERO values, not nil. The count is the only way to see it.
        assert(count(f:GetBackdrop()) == 0, "unset backdrop must return no values at all")

        -- TRAP 1: the result is rebuilt from the struct, not the caller's table. The alien key is
        -- the proof: SetBackdrop never accepted it, so it cannot come back out.
        local passed = { bgFile = "bg", edgeFile = "edge", tile = true,
                         tileSize = 16, edgeSize = 24,
                         insets = { left = 1, right = 2, top = 3, bottom = 4 },
                         alien = "must not survive" }
        f:SetBackdrop(passed)
        local b = f:GetBackdrop()
        assert(count(f:GetBackdrop()) == 1, "a set backdrop is exactly one value")
        assert(b ~= passed, "must not hand back the caller's own table")
        assert(b.alien == nil, "keys SetBackdrop never read cannot reappear")
        assert(b.insets ~= passed.insets, "the insets subtable is rebuilt too")
        assert(b.bgFile == "bg" and b.edgeFile == "edge", "files round-trip")
        assert(b.tileSize == 16 and b.edgeSize == 24, "sizes round-trip")
        assert(b.insets.left == 1 and b.insets.right == 2
               and b.insets.top == 3 and b.insets.bottom == 4, "insets round-trip")

        -- TRAP 4: `tile` is the NUMBER 1, never the boolean the caller passed.
        assert(type(b.tile) == "number", "tile must be a number, got " .. type(b.tile))
        assert(b.tile == 1, "tile must be 1")

        -- Mutating the caller's table afterwards cannot reach the frame (no reference is kept).
        passed.bgFile = "clobbered"
        assert(f:GetBackdrop().bgFile == "bg", "no live reference to the caller's table")

        -- TRAP 3: a partial SetBackdrop omits NOTHING — a fresh struct is allocated every call, so
        -- the keys left out come back as CTOR DEFAULTS, and none of the old backdrop survives.
        f:SetBackdrop({ edgeFile = "onlyedge" })
        local p = f:GetBackdrop()
        assert(p.bgFile == "", "an omitted bgFile is the empty string, not nil")
        assert(p.edgeSize == 32, "an omitted edgeSize is the ctor's 32, not the previous 24")
        assert(p.tileSize == 0, "an omitted tileSize is 0, not the previous 16")
        assert(p.insets.left == 0 and p.insets.right == 0
               and p.insets.top == 0 and p.insets.bottom == 0, "omitted insets are 0, not 1/2/3/4")
        -- TRAP 4, the other half: tile false pushes nil, so the key is ABSENT — never `false`.
        assert(p.tile == nil, "tile false means the key is absent, not `false`")

        -- The undocumented in-place form: fills and returns YOUR table, recycling `insets`, and
        -- erases a stale `tile` on the way (the nil push is a real `lua_settable`).
        local ins = {}
        local mine = { insets = ins, tile = true, stale = "kept" }
        local got = f:GetBackdrop(mine)
        assert(got == mine, "the in-place form returns the table it was given")
        assert(mine.insets == ins, "an existing insets subtable is reused, not replaced")
        assert(mine.tile == nil, "a stale tile key is erased from a recycled table")
        assert(mine.stale == "kept", "keys it does not write are left alone")
        assert(mine.edgeSize == 32 and ins.bottom == 0, "the recycled table is filled")

        -- SetBackdrop(nil) is indistinguishable from never having set one: back to zero values.
        f:SetBackdrop(nil)
        assert(count(f:GetBackdrop()) == 0, "SetBackdrop(nil) returns to the zero-value shape")
    "#,
    )
    .unwrap();
}

// The shape of `BuffCheck2.lua:448`: read the plate back, edit `insets`, set it again.
#[test]
fn get_backdrop_round_trips_through_set_backdrop() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "BC2Plate")
        f:SetPoint("CENTER", 0, 0)
        f:SetWidth(54); f:SetHeight(54)
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge", tile = true,
                        tileSize = 32, edgeSize = 32,
                        insets = { left = 11, right = 12, top = 12, bottom = 11 } })
        local backdrop = f:GetBackdrop()
        backdrop["insets"] = { top = 12, bottom = 11, right = 12, left = 11 }
        backdrop["tile"] = true
        backdrop["tileSize"] = 32
        backdrop["edgeSize"] = 32
        f:SetBackdrop(backdrop)
        local b = f:GetBackdrop()
        assert(b.bgFile == "bg" and b.edgeFile == "edge", "files survive the round trip")
        assert(b.tile == 1 and b.tileSize == 32 and b.edgeSize == 32, "tiling survives")
        assert(b.insets.left == 11 and b.insets.bottom == 11, "insets survive")
    "#,
    )
    .unwrap();
    s.resolve();
    // bg (1) + 8 border pieces.
    assert_eq!(
        s.extract()
            .iter()
            .filter(|q| matches!(q.content, QuadContent::Backdrop { .. }))
            .count(),
        9
    );
}

#[test]
fn set_backdrop_nil_tears_down() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local f = CreateFrame("Frame", "Plate2")
        f:SetPoint("CENTER", 0, 0)
        f:SetWidth(100); f:SetHeight(100)
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        f:SetBackdrop(nil)
    "#,
    )
    .unwrap();
    s.resolve();
    assert!(s
        .extract()
        .iter()
        .all(|q| !matches!(q.content, QuadContent::Backdrop { .. })));
}

/// The colour setters read r/g/b with a bare `lua_tonumber` (nil is 0.0) and gate alpha on
/// `lua_isnumber` over a staged 1.0.
#[test]
fn backdrop_colors_coerce_rgb_but_default_alpha_opaque() {
    let s = script();
    s.run(
        r#"
        f = CreateFrame("Frame", "Plate2")
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge", edgeSize = 16 })
    "#,
    )
    .unwrap();

    s.run("f:SetBackdropBorderColor(nil, 1, 1, 1)").unwrap();
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return f:GetBackdropBorderColor()")
            .unwrap(),
        (0.0, 1.0, 1.0, 1.0)
    );

    s.run("f:SetBackdropColor(0.2, 0.4, 0.6)").unwrap();
    let (_, _, _, a) = s
        .eval::<(f32, f32, f32, f32)>("return f:GetBackdropColor()")
        .unwrap();
    assert_eq!(a, 1.0, "a missing alpha is opaque");
    s.run("f:SetBackdropColor(0.2, 0.4, 0.6, nil)").unwrap();
    let (_, _, _, a) = s
        .eval::<(f32, f32, f32, f32)>("return f:GetBackdropColor()")
        .unwrap();
    assert_eq!(
        a, 1.0,
        "an explicit nil alpha is opaque too — the isnumber gate"
    );

    // Out of range clamps; a table coerces to 0 like any non-number.
    s.run("f:SetBackdropColor(5, -1, {}, 0.5)").unwrap();
    let (r, g, b, a) = s
        .eval::<(f32, f32, f32, f32)>("return f:GetBackdropColor()")
        .unwrap();
    assert_eq!((r, g, b), (1.0, 0.0, 0.0));
    // Stored as a byte: 0.5 reads back as 128/255.
    assert_eq!(a, 128.0 / 255.0);
}

// ── The nine Texture regions (`0x4a2e00`) ─────────────────────────────────────────────────

/// `SetBackdrop`'s full table; a frame of 200×100 at the screen's bottom left.
fn plate(s: &mut UiScript) {
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        F = CreateFrame("Frame", "NinePlate", UIParent)
        F:SetPoint("BOTTOMLEFT", UIParent, "BOTTOMLEFT", 0, 0)
        F:SetWidth(200); F:SetHeight(100)
        F:SetBackdrop({ bgFile = "bg", edgeFile = "edge", tile = true, tileSize = 16, edgeSize = 16,
                        insets = { left = 5, right = 5, top = 5, bottom = 5 } })
        function Layers(f)
            local out = {}
            for _, r in ipairs({ f:GetRegions() }) do
                table.insert(out, r:GetDrawLayer())
            end
            return table.concat(out, ",")
        end
    "#,
    )
    .unwrap();
}

fn quads(s: &UiScript) -> Vec<ExtractedQuad> {
    s.extract()
        .into_iter()
        .filter(|q| matches!(q.content, QuadContent::Backdrop { .. }))
        .collect()
}

#[test]
fn a_backdrop_is_nine_texture_regions_background_then_eight_border() {
    let mut s = script();
    plate(&mut s);
    assert_eq!(s.eval::<i64>("return F:GetNumRegions()").unwrap(), 9);
    assert_eq!(s.arity("F:GetRegions()").unwrap(), 9);
    assert_eq!(
        s.eval::<String>("return Layers(F)").unwrap(),
        "BACKGROUND,BORDER,BORDER,BORDER,BORDER,BORDER,BORDER,BORDER,BORDER"
    );
    let (kind, tex) = s
        .eval::<(String, String)>(
            "local r = F:GetRegions() return r:GetObjectType(), r:GetTexture()",
        )
        .unwrap();
    assert_eq!((kind.as_str(), tex.as_str()), ("Texture", "bg"));
    assert_eq!(
        s.eval::<String>("local _, r = F:GetRegions() return r:GetTexture()")
            .unwrap(),
        "edge"
    );
}

// `0x4a2e00`'s slot order, read off the anchors: LEFT, RIGHT, TOP, BOTTOM, then the corners.
#[test]
fn the_slot_order_is_stock_left_right_top_bottom_corners() {
    let mut s = script();
    plate(&mut s);
    let got = s
        .eval::<String>(
            r#"
            local rows, regions = {}, { F:GetRegions() }
            for i = 2, 9 do
                local p, _, _, x, y = regions[i]:GetPoint(1)
                table.insert(rows, p .. ":" .. x .. ":" .. y .. ":" .. regions[i]:GetNumPoints())
            end
            return table.concat(rows, " ")
        "#,
        )
        .unwrap();
    assert_eq!(
        got,
        "TOPLEFT:0:-16:2 TOPRIGHT:0:-16:2 TOPLEFT:16:0:2 BOTTOMLEFT:16:0:2 \
         TOPLEFT:0:0:1 TOPRIGHT:0:0:1 BOTTOMLEFT:0:0:1 BOTTOMRIGHT:0:0:1"
    );
    // Each edge gets the size the builder gave it: LEFT its width, TOP its height.
    let (lw, th) = s
        .eval::<(f32, f32)>("local _,l,_,t = F:GetRegions() return l:GetWidth(), t:GetHeight()")
        .unwrap();
    assert_eq!((lw, th), (16.0, 16.0));
}

// Both linkers append at the tail: a region made earlier stays first, one made later comes after,
// and a rebuilt backdrop relinks after both.
#[test]
fn the_slots_link_at_the_tail_of_the_region_list() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        F = CreateFrame("Frame", "Tail", UIParent)
        Before = F:CreateTexture(nil, "ARTWORK")
        F:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        After = F:CreateTexture(nil, "OVERLAY")
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return F:GetNumRegions()").unwrap(), 11);
    assert!(s
        .eval::<bool>("local a = F:GetRegions() return a == Before")
        .unwrap());
    assert!(s
        .eval::<bool>("local r = { F:GetRegions() } return r[11] == After")
        .unwrap());
    s.run("Held = select(2, F:GetRegions()) F:SetBackdrop({ bgFile = 'other' })")
        .unwrap();
    assert_eq!(s.eval::<i64>("return F:GetNumRegions()").unwrap(), 3);
    assert_eq!(
        s.eval::<String>("local a,b,c = F:GetRegions() return c:GetTexture()")
            .unwrap(),
        "other",
        "the new background links after After"
    );
}

#[test]
fn only_the_files_given_build_their_slots() {
    let s = script();
    s.run(
        r#"
        F = CreateFrame("Frame", "Gates", UIParent)
        F:SetBackdrop({ bgFile = "bg" })
    "#,
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return F:GetNumRegions()").unwrap(), 1);
    assert_eq!(
        s.eval::<String>("return select(1, F:GetRegions()):GetDrawLayer()")
            .unwrap(),
        "BACKGROUND"
    );
    s.run("F:SetBackdrop({ edgeFile = 'edge' })").unwrap();
    assert_eq!(s.eval::<i64>("return F:GetNumRegions()").unwrap(), 8);
    s.run("F:SetBackdrop({ bgFile = '' })").unwrap();
    assert_eq!(s.eval::<i64>("return F:GetNumRegions()").unwrap(), 0);
}

#[test]
fn the_backdrop_colours_write_the_regions() {
    let mut s = script();
    plate(&mut s);
    s.run("F:SetBackdropColor(0.2, 0.4, 0.6, 0.8) F:SetBackdropBorderColor(1, 0, 0, 0.5)")
        .unwrap();
    let q = |x: f32| f32::from((x * 255.0 + 0.5) as u8) / 255.0;
    let bg = s
        .eval::<(f32, f32, f32, f32)>("return (F:GetRegions()):GetVertexColor()")
        .unwrap();
    assert_eq!(bg, (q(0.2), q(0.4), q(0.6), q(0.8)));
    s.resolve();
    let colors: Vec<[f32; 4]> = quads(&s)
        .into_iter()
        .map(|q| match q.content {
            QuadContent::Backdrop { color, .. } => color,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(colors.len(), 9);
    assert_eq!(colors[0], [q(0.2), q(0.4), q(0.6), q(0.8)]);
    assert!(colors[1..].iter().all(|c| *c == [1.0, 0.0, 0.0, q(0.5)]));
}

// `0x4a29d0`: the runs are the size over the edge, minus two; TOP's texcoords turned.
#[test]
fn a_resize_rewrites_the_texcoords_and_the_colours() {
    let mut s = script();
    plate(&mut s);
    s.resolve();
    let bg = "local b = F:GetRegions() return b:GetTexCoord()";
    assert_eq!(
        s.eval::<(f32, f32, f32, f32, f32, f32, f32, f32)>(bg)
            .unwrap(),
        (0.0, 0.0, 0.0, 6.25, 12.5, 0.0, 12.5, 6.25),
        "tiled background: whole rect over the tile, ul ll ur lr"
    );
    let left = "local _,l = F:GetRegions() return l:GetTexCoord()";
    let (lx, ly, _, lly, ..) = s
        .eval::<(f32, f32, f32, f32, f32, f32, f32, f32)>(left)
        .unwrap();
    assert_eq!((lx, ly), (0.0078125, 0.0625));
    assert!((lly - (100.0 / 16.0 - 2.0 - 0.0625)).abs() < 1e-4);

    // SetBackdropColor state survives; a hand-tinted edge is re-tinted by the resize.
    s.run("(select(3, F:GetRegions())):SetVertexColor(0, 1, 0, 1) F:SetWidth(320)")
        .unwrap();
    s.resolve();
    let top = "local _,_,_,t = F:GetRegions() return t:GetTexCoord()";
    let (_, ulv, ..) = s
        .eval::<(f32, f32, f32, f32, f32, f32, f32, f32)>(top)
        .unwrap();
    assert!((ulv - (320.0 / 16.0 - 2.0 - 0.0625)).abs() < 1e-4);
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>("return (select(3, F:GetRegions())):GetVertexColor()")
            .unwrap(),
        (1.0, 1.0, 1.0, 1.0),
        "the edge colour is re-applied"
    );

    let (l, b, t) = s
        .eval::<(f32, f32, f32)>(
            "local bg = F:GetRegions() return bg:GetLeft(), bg:GetBottom(), bg:GetTop()",
        )
        .unwrap();
    assert_eq!((l, b, t), (5.0, 5.0, 95.0));
}

// Under a scale the slots are still the frame's own textures: half size, unscaled runs.
#[test]
fn a_scaled_frame_draws_the_regions_at_its_scale() {
    let mut s = script();
    plate(&mut s);
    s.run("F:SetScale(0.5)").unwrap();
    s.resolve();
    let list = quads(&s);
    let left = list[1].rect.unwrap();
    assert!((left.right - left.left - 8.0).abs() < 1e-4, "{left:?}");
    if let QuadContent::Backdrop { uvs, .. } = &list[1].content {
        assert!((uvs[3][1] - (50.0 / 16.0 - 2.0 - 0.0625)).abs() < 1e-4);
    }
}

// Same strata and level: a frame's BACKGROUND regions go before any frame's BORDER, and a later
// frame's texture still interleaves with this frame's two layers.
#[test]
fn the_slots_draw_in_their_layers_not_the_frame_slot() {
    use crate::order::{unpack, DrawLayer};
    let mut s = script();
    plate(&mut s);
    s.run(
        r#"
        Sib = CreateFrame("Frame", "Sib", UIParent)
        Sib:SetAllPoints(F)
        Sib:SetFrameLevel(F:GetFrameLevel())
        local bg = Sib:CreateTexture(nil, "BACKGROUND") bg:SetAllPoints(Sib) bg:SetTexture("sibbg")
        local art = F:CreateTexture(nil, "ARTWORK") art:SetAllPoints(F) art:SetTexture("art")
    "#,
    )
    .unwrap();
    s.resolve();
    let list = s.extract();
    let z_of = |want: &str| {
        list.iter()
            .find(|q| match &q.content {
                QuadContent::Backdrop { path, .. } => path == want,
                QuadContent::Texture { path: Some(p), .. } => p == want,
                _ => false,
            })
            .map(|q| q.z)
            .unwrap_or_else(|| panic!("no quad for {want}"))
    };
    let (bg, edge, sib, art) = (z_of("bg"), z_of("edge"), z_of("sibbg"), z_of("art"));
    assert_eq!(unpack(bg).layer, DrawLayer::Background.index());
    assert_eq!(unpack(edge).layer, DrawLayer::Border.index());
    assert!(bg < edge, "the background before the border");
    assert!(edge < art, "the border before the frame's ARTWORK");
    assert!(
        sib < edge,
        "a same-level frame's BACKGROUND before this frame's BORDER"
    );
    assert!(bg < sib, "and the shared layer breaks on the link stamp");
    assert!(unpack(bg).is_region && unpack(edge).is_region);
}

// A region is the addon's to change: a nil texture draws nothing, a colour goes the texture path.
#[test]
fn the_regions_answer_their_own_verbs() {
    let mut s = script();
    plate(&mut s);
    s.resolve();
    assert_eq!(quads(&s).len(), 9);
    s.run("local a, b, c = F:GetRegions() a:SetTexture(nil) b:Hide() c:SetTexture(0, 1, 0, 0.5)")
        .unwrap();
    s.resolve();
    let list = s.extract();
    let drawn = list
        .iter()
        .filter(|q| matches!(q.content, QuadContent::Backdrop { .. }))
        .count();
    assert_eq!(
        drawn, 6,
        "bg nil and the left edge hidden and the right gone solid"
    );
    assert_eq!(
        list.iter()
            .filter(
                |q| matches!(&q.content, QuadContent::Texture { color: Some(c), path: None, .. }
                if *c == [0.0, 1.0, 0.0, 0.5])
            )
            .count(),
        1,
        "the solid edge left the atlas path"
    );
}

// Rebuilding frees every old slot and keeps the declaration field in range.
#[test]
fn rebuilding_a_backdrop_cleans_its_slots() {
    let mut s = script();
    plate(&mut s);
    s.resolve();
    s.run("Old = select(2, F:GetRegions())").unwrap();
    let baseline = s.model_ref().region_data.len();
    for i in 0..460 {
        s.run(&format!(
            "F:SetBackdrop({{ bgFile = 'bg{i}', edgeFile = 'edge', edgeSize = 8 }})"
        ))
        .unwrap();
    }
    s.resolve();
    assert_eq!(
        s.model_ref().region_data.len(),
        baseline,
        "the replaced slots leave nothing behind"
    );
    assert_eq!(s.eval::<i64>("return F:GetNumRegions()").unwrap(), 9);
    assert!(s.run("Old:GetTexture()").is_err(), "the old wrapper raises");
    let model = s.model_ref();
    let frame = model
        .arena
        .iter_frames()
        .find(|(_, f)| f.name.as_deref() == Some("NinePlate"))
        .map(|(h, _)| h)
        .unwrap();
    assert!(model
        .arena
        .frame(frame)
        .unwrap()
        .regions
        .iter()
        .all(|&r| model.arena.region(r).unwrap().decl_seq < 1 << crate::order::DECL_BITS));
    drop(model);
    assert_eq!(
        quads(&s)
            .iter()
            .filter(|q| matches!(&q.content, QuadContent::Backdrop { path, .. } if path == "bg459"))
            .count(),
        1
    );
    s.run("F:SetBackdrop(nil)").unwrap();
    assert_eq!(s.eval::<i64>("return F:GetNumRegions()").unwrap(), 0);
    s.resolve();
    assert!(quads(&s).is_empty(), "teardown removes every draw");
    assert_eq!(
        s.model_ref().region_data.len(),
        baseline - 9,
        "teardown frees the slots too"
    );
}
