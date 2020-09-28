use log::warn;

use crate::structure::locator::LocatorList;
use crate::structure::guid::{EntityId, GUID};
use crate::{
  discovery::data_types::topic_data::DiscoveredWriterData,
  structure::sequence_number::{SequenceNumber},
};
use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug)]
pub struct RtpsWriterProxy {
  /// Identifies the remote matched Writer
  pub remote_writer_guid: GUID,

  /// List of unicast (address, port) combinations that can be used to send
  /// messages to the matched Writer or Writers. The list may be empty.
  pub unicast_locator_list: LocatorList,

  /// List of multicast (address, port) combinations that can be used to send
  /// messages to the matched Writer or Writers. The list may be empty.
  pub multicast_locator_list: LocatorList,

  /// Identifies the group to which the matched Reader belongs
  pub remote_group_entity_id: EntityId,

  /// List of sequence_numbers received from the matched RTPS Writer
  // TODO: When should they be removed from here?
  pub changes: HashMap<SequenceNumber, Instant>,

  pub received_heartbeat_count: i32,

  pub sent_ack_nack_count: i32,
}

impl RtpsWriterProxy {
  pub fn new(
    remote_writer_guid: GUID,
    unicast_locator_list: LocatorList,
    multicast_locator_list: LocatorList,
    remote_group_entity_id: EntityId,
  ) -> Self {
    Self {
      remote_writer_guid,
      unicast_locator_list,
      multicast_locator_list,
      remote_group_entity_id,
      changes: HashMap::new(),
      received_heartbeat_count: 0,
      sent_ack_nack_count: 0,
    }
  }

  pub fn update_contents(&mut self, other: RtpsWriterProxy) {
    self.unicast_locator_list = other.unicast_locator_list;
    self.multicast_locator_list = other.multicast_locator_list;
    self.remote_group_entity_id = other.remote_group_entity_id;
  }

  pub fn changes_are_missing(&self, hb_last_sn: SequenceNumber) -> bool {
    let min_sn = match self.available_changes_min() {
      Some(sn) => *sn,
      None => SequenceNumber::from(0),
    };
    i64::from(hb_last_sn) > i64::from(min_sn)
  }

  pub fn received_changes_add(&mut self, seq_num: SequenceNumber, instant: Instant) {
    self.changes.insert(seq_num, instant);
  }

  pub fn available_changes_max(&self) -> Option<SequenceNumber> {
    match self.changes.iter().max() {
      Some((sn, _)) => Some(*sn),
      None => None,
    }
  }

  pub fn available_changes_min(&self) -> Option<&SequenceNumber> {
    if let Some((seqnum, _)) = self.changes.iter().min() {
      return Some(seqnum);
    }
    None
  }

  pub fn set_irrelevant_change(&mut self, seq_num: SequenceNumber) -> Instant {
    self.changes.remove(&seq_num).unwrap()
  }

  pub fn irrelevant_changes_up_to(&mut self, smallest_seqnum: SequenceNumber) -> Vec<Instant> {
    let mut remove = Vec::new();
    for (&seqnum, _) in self.changes.iter() {
      if seqnum < smallest_seqnum {
        remove.push(seqnum);
      }
    }

    let mut instants = Vec::new();
    for &rm in remove.iter() {
      match self.changes.remove(&rm) {
        Some(i) => instants.push(i),
        None => (),
      };
    }

    instants
  }

  pub fn missing_changes(&self, hb_last_sn: SequenceNumber) -> Vec<SequenceNumber> {
    let mut result: Vec<SequenceNumber> = Vec::new();

    if !self.changes_are_missing(hb_last_sn) {
      return result;
    }

    let min_sn = match self.available_changes_min() {
      Some(sn) => *sn,
      None => SequenceNumber::from(0),
    };
    // All changes between min and last_sn which are not in our local set
    for sn_int in i64::from(min_sn)..i64::from(hb_last_sn) {
      let sn = SequenceNumber::from(sn_int);
      if !self.changes.contains_key(&sn) {
        result.push(SequenceNumber::from(sn_int));
      }
    }
    result
  }

  pub fn from_discovered_writer_data(
    discovered_writer_data: &DiscoveredWriterData,
  ) -> Option<RtpsWriterProxy> {
    let remote_writer_guid = match &discovered_writer_data.writer_proxy.remote_writer_guid {
      Some(v) => v,
      None => {
        warn!("Failed to convert DiscoveredWriterData to RtpsWriterProxy. No GUID.");
        return None;
      }
    };

    Some(RtpsWriterProxy {
      remote_writer_guid: remote_writer_guid.clone(),
      remote_group_entity_id: EntityId::ENTITYID_UNKNOWN,
      unicast_locator_list: discovered_writer_data
        .writer_proxy
        .unicast_locator_list
        .clone(),
      multicast_locator_list: discovered_writer_data
        .writer_proxy
        .multicast_locator_list
        .clone(),
      changes: HashMap::new(),
      received_heartbeat_count: 0,
      sent_ack_nack_count: 0,
    })
  }
}
