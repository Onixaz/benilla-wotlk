//! The flight master's sends. A flight is `CMSG_ACTIVATETAXI` when a direct `TaxiPath` edge joins
//! the two nodes, else `CMSG_ACTIVATETAXIEXPRESS` with the whole chain; both are answered by
//! `SMSG_ACTIVATETAXIREPLY`.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// `CMSG_TAXINODE_STATUS_QUERY`: whether we know a nearby flight master's node; answered by
    /// `SMSG_TAXINODE_STATUS`.
    pub fn taxi_node_status_query(&mut self, flightmaster_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_TAXINODE_STATUS_QUERY,
            &messages::taxi_node_status_query(flightmaster_guid),
        )
    }

    /// `CMSG_TAXIQUERYAVAILABLENODES`: a known node answers `SMSG_SHOWTAXINODES`; a new node
    /// answers with the first-visit learn pair instead, and no map opens on that click.
    pub fn taxi_query_available_nodes(&mut self, flightmaster_guid: u64) -> Result<()> {
        self.send(
            opcode::CMSG_TAXIQUERYAVAILABLENODES,
            &messages::taxi_query_available_nodes(flightmaster_guid),
        )
    }

    /// `CMSG_ACTIVATETAXI`: one hop; success mounts us and flies an `SMSG_MONSTER_MOVE` path.
    pub fn activate_taxi(
        &mut self,
        flightmaster_guid: u64,
        source_node: u32,
        dest_node: u32,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_ACTIVATETAXI,
            &messages::activate_taxi(flightmaster_guid, source_node, dest_node),
        )
    }

    /// `CMSG_ACTIVATETAXIEXPRESS`: the route's combined fare and its whole node chain, in order.
    pub fn activate_taxi_express(
        &mut self,
        flightmaster_guid: u64,
        total_cost: u32,
        nodes: &[u32],
    ) -> Result<()> {
        self.send(
            opcode::CMSG_ACTIVATETAXIEXPRESS,
            &messages::activate_taxi_express(flightmaster_guid, total_cost, nodes),
        )
    }

    /// `CMSG_SET_TAXI_BENCHMARK_MODE` (`0x517b50`); the server answers by the player flag 0x20000.
    pub fn set_taxi_benchmark_mode(&mut self, on: bool) -> Result<()> {
        self.send(opcode::CMSG_SET_TAXI_BENCHMARK_MODE, &[u8::from(on)])
    }
}
