//! The taxi bindings stock `TaxiFrame.lua` reads over a pushed snapshot, and the node and close
//! intents.

use super::common::script;
use crate::script::*;

#[test]
fn taxi_snapshot_surfaces_and_acts_reach_the_view_in_their_call() {
    let mut s = script();
    let mut loan_v = crate::script::live::TestView::default();
    crate::script::live::lending(&mut loan_v, || {
        s.set_screen_size(800.0, 600.0);
        s.set_taxi(Some(TaxiUiState {
            art: "Interface\\TaxiFrame\\TAXIMAP1".into(),
            nodes: vec![
                TaxiUiNode {
                    name: "Crossroads, The Barrens".into(),
                    node_type: TaxiNodeType::Current,
                    pos: (0.5, 0.6),
                    cost: 0,
                    routes: vec![],
                    num_routes: 0,
                },
                TaxiUiNode {
                    name: "Orgrimmar, Durotar".into(),
                    node_type: TaxiNodeType::Reachable,
                    pos: (0.55, 0.8),
                    cost: 110,
                    routes: vec![[0.5, 0.6, 0.55, 0.8]],
                    num_routes: 1,
                },
            ],
        }));

        s.run(
            r#"
        f = CreateFrame("Frame", "TaxiHost")
        f:SetWidth(316); f:SetHeight(352); f:SetPoint("CENTER", 0, 0)
        map = f:CreateTexture("TaxiMapTex", "OVERLAY")
        SetTaxiMap(map)

        assert(NumTaxiNodes() == 2)
        assert(TaxiNodeGetType(1) == "CURRENT")
        assert(TaxiNodeGetType(2) == "REACHABLE")
        assert(TaxiNodeGetType(3) == "NONE")           -- out of range hides, ref-style
        assert(TaxiNodeName(3) == "INVALID")
        assert(not pcall(TaxiNodePosition, 3) and not pcall(TaxiNodeCost, 3))
        assert(not pcall(TaxiNodeName))
        local x, y = TaxiNodePosition(2)
        assert(math.abs(x - 0.55) < 1e-6 and math.abs(y - 0.8) < 1e-6)
        assert(TaxiNodeName(2) == "Orgrimmar, Durotar")
        assert(TaxiNodeCost(2) == 110)
        TaxiNodeSetCurrent(2)                          -- the lines are drawn at the setup
        assert(GetNumRoutes(1) == 0 and GetNumRoutes(2) == 1)
        assert(math.abs(TaxiGetSrcX(2, 1) - 0.5) < 1e-6)
        assert(math.abs(TaxiGetDestY(2, 1) - 0.8) < 1e-6)
        assert(UnitOnTaxi("player") == nil)          -- 1/nil, never a boolean

        TakeTaxiNode(2)
        CloseTaxiMap()
    "#,
        )
        .unwrap();

        assert_eq!(
            crate::script::live::test_view(|v| std::mem::take(&mut v.taxi_acts)),
            vec![TaxiAct::Take(1), TaxiAct::Close],
            "the 0-based slot, then the close"
        );

        s.resolve();
        assert!(
            s.extract().iter().any(|q| matches!(
                &q.content,
                QuadContent::Texture { path: Some(p), .. } if p == "Interface\\TaxiFrame\\TAXIMAP1"
            )),
            "the continent art draws on the SetTaxiMap target"
        );

        s.set_unit_guids(&crate::script::UnitGuids {
            player: 1,
            held: [(1, 0)].into(),
            ..Default::default()
        });
        let mut riding = crate::script::live::TestView {
            units: [(1, (1 << 20, 1))].into(),
            ..Default::default()
        };
        s.with_live(&mut riding, |s| {
            s.run(r#"assert(UnitOnTaxi("player") == 1)"#)
        })
        .unwrap();
        assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    });
}
