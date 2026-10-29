//! The roster records' readers for a guid the object manager does not hold: the member record, the
//! pet record and the party and raid pets' guids.

use benilla_protocol::messages::{ObjectFields, PartyMemberStatsInfo, RecordAura};

use super::GroupState;

impl GroupState {
    /// The member record for `guid`, party or raid.
    fn member_record(&self, guid: u64) -> Option<&PartyMemberStatsInfo> {
        self.stats.get(&guid)
    }

    /// The record whose pet block names `guid`; a pet is found only while its owner's record reads
    /// online, party or raid.
    pub(crate) fn pet_record(&self, guid: u64) -> Option<&PartyMemberStatsInfo> {
        self.stats
            .values()
            .find(|r| guid != 0 && r.pet_guid == Some(guid) && r.is_online())
    }

    /// Whether the roster fallback of `UnitExists` names `member`'s pet, which the object manager
    /// may not hold and which `partypetN` or `raidpetN` resolved to a guid: a held member answers
    /// off its descriptor's `CHARM`/`SUMMON`, an unheld one off its record's pet guid while the
    /// record reads online. That test is the party pet's own online gate already, but not the raid
    /// pet's guid, which the resolver reads off the record with none.
    pub(crate) fn roster_names_pet(&self, member: u64, member_held: bool) -> bool {
        member_held
            || self
                .member_record(member)
                .is_some_and(PartyMemberStatsInfo::is_online)
    }

    /// `partypetN`'s guid for the member `member`: `CHARM`, else `SUMMON`, off the member's
    /// descriptor while it is held, else the record's pet guid while the record reads online.
    pub(crate) fn party_pet_guid(&self, member: u64, live: Option<&ObjectFields>) -> Option<u64> {
        self.pet_guid_of(member, live, true)
    }

    /// `raidpetN`'s guid for the raid member `member`: `CHARM`, else `SUMMON`, off the member's
    /// descriptor while it is held, else the record's pet guid with no online test, where
    /// `partypetN` has one.
    pub(crate) fn raid_pet_guid(&self, member: u64, live: Option<&ObjectFields>) -> Option<u64> {
        self.pet_guid_of(member, live, false)
    }

    /// The guid `UNIT_PET` for the owner `member` keys on, party or raid: `CHARM`, else `SUMMON`,
    /// off the member's descriptor while it is held, else the record's pet guid, with no online
    /// test. The stats handler's diff fires it on a moved pet guid; a moved status byte sets
    /// another bit, which is health and power events, and the list's own write of the online bit
    /// fires nothing. A `partypetN` that appears or vanishes behind the bit is therefore no move of
    /// this guid.
    pub(crate) fn pet_edge_guid(&self, member: u64, live: Option<&ObjectFields>) -> Option<u64> {
        self.pet_guid_of(member, live, false)
    }

    fn pet_guid_of(
        &self,
        member: u64,
        live: Option<&ObjectFields>,
        online_only: bool,
    ) -> Option<u64> {
        match live {
            Some(fields) => fields.unit_pet_guid(),
            None => self
                .member_record(member)
                .filter(|r| !online_only || r.is_online())
                .and_then(|r| r.pet_guid)
                .filter(|&g| g != 0),
        }
    }

    /// The auras `UnitAura` walks for a guid with no live object (`0x614a2a`-`0x614a5c`),
    /// ascending slot: a member record's block (`+0x20`) unless it reads offline, which answers
    /// nothing and never tries the pet records (`0x614a36`); on a miss, the block of the pet
    /// record naming the guid (`+0x1c8`).
    pub(crate) fn roster_auras(&self, guid: u64) -> impl Iterator<Item = RecordAura> + '_ {
        let block = match self.member_record(guid) {
            Some(r) if !r.is_online() => None,
            Some(r) => Some(&r.auras),
            None => self.pet_record(guid).map(|r| &r.pet_auras),
        };
        block.into_iter().flatten().flatten().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{member_status, GroupMemberEntry};

    const MEMBER: u64 = 0x1234;
    const PET: u64 = 0xF140_0000_0000_0077;

    fn group_with(record: PartyMemberStatsInfo) -> GroupState {
        let mut g = GroupState::default();
        g.apply_list(
            0,
            0,
            vec![GroupMemberEntry {
                name: "Brisca".into(),
                guid: MEMBER,
                status: member_status::ONLINE,
                flags: 0,
                roles: 0,
            }],
            MEMBER,
            None,
            None,
        );
        g.stats.insert(MEMBER, record);
        g
    }

    fn aura(slot: u8, spell_id: u32, flags: u8) -> RecordAura {
        RecordAura {
            slot,
            spell_id,
            flags,
        }
    }

    fn record(status: u8) -> PartyMemberStatsInfo {
        PartyMemberStatsInfo {
            status: Some(status.into()),
            auras: Some(vec![
                aura(0, 1126, 0x13),
                aura(4, 21562, 0x13),
                aura(7, 589, 0x81),
            ]),
            pet_guid: Some(PET),
            pet_auras: Some(vec![aura(2, 1126, 0x13), aura(5, 770, 0x81)]),
            ..PartyMemberStatsInfo::default()
        }
    }

    #[test]
    fn a_member_record_answers_its_own_block_and_an_offline_one_answers_nothing() {
        let g = group_with(record(member_status::ONLINE));
        let slots = |g: &GroupState, guid| {
            g.roster_auras(guid)
                .map(|a| (a.slot, a.spell_id))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            slots(&g, MEMBER),
            [(0, 1126), (4, 21562), (7, 589)],
            "one block, ascending slot"
        );
        assert_eq!(slots(&g, PET), [(2, 1126), (5, 770)]);
        assert!(slots(&g, 0x9999).is_empty(), "a miss everywhere");

        let g = group_with(record(member_status::OFFLINE));
        assert!(slots(&g, MEMBER).is_empty());
        assert!(
            slots(&g, PET).is_empty(),
            "an offline owner's pet is not found"
        );
    }

    /// The reference copies the saved record back and then sets or clears bit 0 of its status byte
    /// from the row's; the raid roster's twin does the same to its row's record. Both test the
    /// row's byte as non-zero.
    #[test]
    fn a_list_rewrites_a_known_members_online_bit_and_keeps_the_rest_of_the_record() {
        use crate::ui_party::GROUPTYPE_RAID;
        for group_type in [0, GROUPTYPE_RAID] {
            let mut g = group_with(record(
                member_status::ONLINE | member_status::DEAD | member_status::PVP,
            ));
            let relist = |g: &mut GroupState, status: u8| {
                let row = |guid, name: &str| GroupMemberEntry {
                    name: name.into(),
                    guid,
                    status,
                    flags: 0,
                    roles: 0,
                };
                g.apply_list(
                    group_type,
                    0,
                    vec![row(MEMBER, "Brisca"), row(0x5678, "Aldwyn")],
                    MEMBER,
                    None,
                    None,
                );
            };

            relist(&mut g, member_status::OFFLINE);
            assert_eq!(
                g.stats[&MEMBER],
                record(member_status::DEAD | member_status::PVP),
                "the bit clears and the rest of the record stays as it was"
            );
            assert!(g.roster_auras(MEMBER).next().is_none());
            assert_eq!(g.party_pet_guid(MEMBER, None), None);

            relist(&mut g, member_status::ONLINE | member_status::AFK);
            assert_eq!(
                g.stats[&MEMBER],
                record(member_status::ONLINE | member_status::DEAD | member_status::PVP),
                "the bit sets, and the row's other bits are not copied"
            );
            assert_eq!(g.party_pet_guid(MEMBER, None), Some(PET));

            relist(&mut g, member_status::AFK);
            assert!(
                g.stats[&MEMBER].is_online(),
                "a non-zero byte is online, bit 0 or not"
            );
            assert!(
                !g.stats.contains_key(&0x5678),
                "a member new to the roster gets its record from the seat, not here"
            );
        }
    }

    #[test]
    fn the_party_pet_guid_is_the_live_charm_then_summon_else_the_records() {
        let g = group_with(record(member_status::ONLINE));
        assert_eq!(g.party_pet_guid(MEMBER, None), Some(PET));

        let summon = ObjectFields::from_pairs(&[(8, 0x55), (9, 0)]);
        assert_eq!(g.party_pet_guid(MEMBER, Some(&summon)), Some(0x55));
        let both = ObjectFields::from_pairs(&[(6, 0x66), (7, 0), (8, 0x55), (9, 0)]);
        assert_eq!(
            g.party_pet_guid(MEMBER, Some(&both)),
            Some(0x66),
            "CHARM first"
        );
        // A held member with neither is petless, whatever the record says.
        assert_eq!(
            g.party_pet_guid(MEMBER, Some(&ObjectFields::default())),
            None
        );

        let g = group_with(record(member_status::OFFLINE));
        assert_eq!(g.party_pet_guid(MEMBER, None), None);
    }

    /// `raidpetN` reads the same fields as `partypetN`, but its record leg has no online test
    /// (against `partypetN`'s).
    #[test]
    fn the_raid_pet_guid_reads_the_record_whatever_its_online_bit() {
        let g = group_with(record(member_status::ONLINE));
        assert_eq!(g.raid_pet_guid(MEMBER, None), Some(PET));
        let summon = ObjectFields::from_pairs(&[(8, 0x55), (9, 0)]);
        assert_eq!(g.raid_pet_guid(MEMBER, Some(&summon)), Some(0x55));
        assert_eq!(
            g.raid_pet_guid(MEMBER, Some(&ObjectFields::default())),
            None
        );

        let g = group_with(record(member_status::OFFLINE));
        assert_eq!(g.raid_pet_guid(MEMBER, None), Some(PET));
        assert!(
            g.roster_auras(PET).next().is_none(),
            "named, but the pet record is not found behind an offline owner"
        );
    }
}
