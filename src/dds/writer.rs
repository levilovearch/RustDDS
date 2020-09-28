use chrono::Duration as chronoDuration;
use enumflags2::BitFlags;
use log::{debug, error, warn};
use speedy::{Writable, Endianness};
use time::Timespec;
use time::get_time;
use mio_extras::channel as mio_channel;
use mio::Token;
use std::{
  time::{Instant, Duration},
  sync::{RwLock, Arc},
  collections::{HashSet, HashMap, BTreeMap, hash_map::DefaultHasher},
};
use std::hash::Hasher;

//use crate::messages::submessages::info_destination::InfoDestination;
use crate::{
  messages::submessages::{
    submessage_elements::{parameter::Parameter, parameter_list::ParameterList},
    info_timestamp::InfoTimestamp,
    submessage_flag::*,
    submessage_elements::serialized_payload::RepresentationIdentifier,
  },
  structure::parameter_id::ParameterId,
};
use crate::messages::submessages::data::Data;
use crate::structure::time::Timestamp;
use crate::messages::protocol_version::ProtocolVersion;
use crate::messages::{header::Header, vendor_id::VendorId, protocol_id::ProtocolId};
use crate::structure::guid::{GuidPrefix, EntityId, GUID};
use crate::structure::sequence_number::{SequenceNumber};
use crate::{
  submessages::{
    Heartbeat, SubmessageHeader, SubmessageKind, InterpreterSubmessage, EntitySubmessage, AckNack,
    InfoDestination,
  },
  structure::cache_change::{CacheChange, ChangeKind},
  serialization::{SubMessage, Message, SubmessageBody},
};

use crate::dds::{ddsdata::DDSData, qos::HasQoSPolicy};
use crate::{
  network::{constant::TimerMessageType, udp_sender::UDPSender},
  structure::{
    entity::{Entity, EntityAttributes},
    endpoint::{EndpointAttributes, Endpoint},
    locator::{Locator, LocatorKind},
    dds_cache::DDSCache,
  },
  common::timed_event_handler::{TimedEventHandler},
};
use super::{
  rtps_reader_proxy::RtpsReaderProxy,
  qos::{policy, QosPolicies},
};
use policy::{History, Reliability};
//use crate::messages::submessages::submessage_elements::serialized_payload::SerializedPayload;

pub struct Writer {
  source_version: ProtocolVersion,
  source_vendor_id: VendorId,
  pub endianness: Endianness,
  heartbeat_message_counter: i32,
  /// Configures the mode in which the
  ///Writer operates. If
  ///pushMode==true, then the Writer
  /// will push changes to the reader. If
  /// pushMode==false, changes will
  /// only be announced via heartbeats
  ///and only be sent as response to the
  ///request of a reader
  pub push_mode: bool,
  ///Protocol tuning parameter that
  ///allows the RTPS Writer to
  ///repeatedly announce the
  ///availability of data by sending a
  ///Heartbeat Message.
  pub heartbeat_period: Option<Duration>,
  /// duration to launch cahche change remove from DDSCache
  pub cahce_cleaning_perioid: Duration,
  ///Protocol tuning parameter that
  ///allows the RTPS Writer to delay
  ///the response to a request for data
  ///from a negative acknowledgment.
  pub nack_respose_delay: Duration,
  ///Protocol tuning parameter that
  ///allows the RTPS Writer to ignore
  ///requests for data from negative
  ///acknowledgments that arrive ‘too
  ///soon’ after the corresponding
  ///change is sent.
  pub nack_suppression_duration: Duration,
  ///Internal counter used to assign
  ///increasing sequence number to
  ///each change made by the Writer
  pub last_change_sequence_number: SequenceNumber,
  ///If samples are available in the Writer, identifies the first (lowest)
  ///sequence number that is available in the Writer.
  ///If no samples are available in the Writer, identifies the lowest
  ///sequence number that is yet to be written by the Writer
  pub first_change_sequence_number: SequenceNumber,
  ///Optional attribute that indicates
  ///the maximum size of any
  ///SerializedPayload that may be
  ///sent by the Writer
  pub data_max_size_serialized: u64,
  endpoint_attributes: EndpointAttributes,
  entity_attributes: EntityAttributes,
  cache_change_receiver: mio_channel::Receiver<DDSData>,
  ///The RTPS ReaderProxy class represents the information an RTPS StatefulWriter maintains on each matched
  ///RTPS Reader
  pub readers: Vec<RtpsReaderProxy>,
  message: Option<Message>,
  udp_sender: UDPSender,
  // This writer can read/write to only one of this DDSCache topic caches identified with my_topic_name
  dds_cache: Arc<RwLock<DDSCache>>,
  /// Writer can only read/write to this topic DDSHistoryCache.
  my_topic_name: String,
  /// Maps this writers local sequence numbers to DDSHistodyCache instants.
  /// Useful when negative acknack is recieved.
  sequence_number_to_instant: BTreeMap<SequenceNumber, Instant>,
  //// Maps this writers local sequence numbers to DDSHistodyCache instants.
  /// Useful when datawriter dispose is recieved.
  key_to_instant: HashMap<u128, Instant>,
  /// Set of disposed samples.
  /// Useful when reader requires some sample with acknack.
  disposed_sequence_numbers: HashSet<SequenceNumber>,
  //When dataWriter sends cacheChange message with cacheKind is NotAlive_Disposed
  //this is set true. If Datawriter after disposing sends new cahceChanges this falg is then
  //turned true.
  //When writer is in disposed state it needs to send StatusInfo_t (PID_STATUS_INFO) with DisposedFlag
  //TODO are messages send every heartbeat or just once ????
  pub writer_is_disposed: bool,
  ///Contains timer that needs to be set to timeout with duration of self.heartbeat_perioid
  ///timed_event_handler sends notification when timer is up via miochannel to poll in Dp_eventWrapper
  ///this also handles writers cache cleaning timeouts.
  timed_event_handler: Option<TimedEventHandler>,

  qos_policies: QosPolicies,
}

impl Writer {
  pub fn new(
    guid: GUID,
    cache_change_receiver: mio_channel::Receiver<DDSData>,
    dds_cache: Arc<RwLock<DDSCache>>,
    topic_name: String,
    qos_policies: QosPolicies,
  ) -> Writer {
    let entity_attributes = EntityAttributes::new(guid);

    let heartbeat_period = match &qos_policies.reliability {
      Some(r) => match r {
        Reliability::BestEffort => None,
        Reliability::Reliable {
          max_blocking_time: _,
        } => Some(Duration::from_secs(3)),
      },
      None => None,
    };

    Writer {
      source_version: ProtocolVersion::PROTOCOLVERSION_2_3,
      source_vendor_id: VendorId::VENDOR_UNKNOWN,
      endianness: Endianness::LittleEndian,
      heartbeat_message_counter: 1,
      push_mode: true,
      heartbeat_period,
      cahce_cleaning_perioid: Duration::from_secs(2 * 60),
      nack_respose_delay: Duration::from_millis(200),
      nack_suppression_duration: Duration::from_nanos(0),
      last_change_sequence_number: SequenceNumber::from(0),
      first_change_sequence_number: SequenceNumber::from(0),
      data_max_size_serialized: 999999999,
      entity_attributes,
      //enpoint_attributes: EndpointAttributes::default(),
      cache_change_receiver,
      readers: vec![
        /*
        RtpsReaderProxy::new_for_unit_testing(1000),
        RtpsReaderProxy::new_for_unit_testing(1001),
        RtpsReaderProxy::new_for_unit_testing(1002),*/
      ],
      message: None,
      endpoint_attributes: EndpointAttributes::default(),
      udp_sender: UDPSender::new_with_random_port(),
      dds_cache,
      my_topic_name: topic_name,
      sequence_number_to_instant: BTreeMap::new(),
      key_to_instant: HashMap::new(),
      disposed_sequence_numbers: HashSet::new(),
      writer_is_disposed: false,
      timed_event_handler: None,
      qos_policies,
    }
  }

  /// To know when token represents a writer we should look entity attribute kind
  /// this entity token can be used in DataWriter -> Writer miochannel.
  pub fn get_entity_token(&self) -> Token {
    let id = self.as_entity().as_usize();
    Token(id)
  }

  /// This token is used in timed event miochannel HearbeatHandler -> dpEventwrapper
  pub fn get_timed_event_entity_token(&self) -> Token {
    let mut hasher = DefaultHasher::new();
    let id = self.as_entity().as_usize() as u64;
    hasher.write(&id.to_le_bytes());
    let hashedID: u64 = hasher.finish();
    Token(hashedID as usize)
  }

  pub fn cache_change_receiver(&self) -> &mio_channel::Receiver<DDSData> {
    &self.cache_change_receiver
  }

  pub fn add_timed_event_handler(&mut self, time_handler: TimedEventHandler) {
    self.timed_event_handler = Some(time_handler);
    self.set_cache_cleaning_timer();
    self.set_heartbeat_timer();
  }

  fn is_reliable(&self) -> bool {
    match self.qos_policies.reliability {
      Some(Reliability::Reliable {
        max_blocking_time: _,
      }) => {
        return true;
      }
      _ => {
        return false;
      }
    }
  }

  /// this should be called everytime cacheCleaning message is received.
  pub fn handle_cache_cleaning(&mut self) {
    let mut removedChanges = vec![];
    match self.qos_policies.history {
      None => {
        removedChanges = self.remove_all_acked_changes_but_keep_depth(0);
      }
      Some(History::KeepAll) => {
        // TODO need to find a way to check if resource limit.
        // writer can only know the amount its own samples.
        //todo!("TODO keep all resource limit ???");
      }
      Some(History::KeepLast { depth: d }) => {
        removedChanges = self.remove_all_acked_changes_but_keep_depth(d);
      }
    }
    // This is needdd to be removed also if cahceChange is removed from DDSCache.
    for sq in removedChanges {
      self.sequence_number_to_instant.remove(&sq);
    }
    self.set_cache_cleaning_timer();
  }

  fn set_cache_cleaning_timer(&mut self) {
    self.timed_event_handler.as_mut().unwrap().set_timeout(
      &chronoDuration::from_std(self.cahce_cleaning_perioid).unwrap(),
      TimerMessageType::writer_cache_cleaning,
    )
  }

  /// this should be called everytime heartbeat message with token is recieved.
  pub fn handle_heartbeat_tick(&mut self) {
    // TODO Set some guidprefix if needed at all.
    // Not sure if DST submessage and TS submessage are needed when sending heartbeat.

    //TODO WHEN FINAL FLAG NEEDS TO BE SET?
    //TODO WHEN LIVELINESS FLAG NEEDS TO BE SET?
    let message_header: Header = self.create_message_header();
    let endianness = self.endianness;

    let mut seqnums: HashMap<GUID, Vec<SequenceNumber>> = HashMap::new();

    for reader in self.readers.iter() {
      seqnums.insert(reader.remote_reader_guid, Vec::new());

      for seqnum in itertools::sorted(reader.unsent_changes().iter()) {
        if reader.unicast_locator_list.len() > 0 {
          let mut rtps_message: Message = Message::new(message_header.clone());

          rtps_message.add_submessage(Writer::get_DST_submessage(
            endianness,
            reader.remote_reader_guid.guidPrefix,
          ));

          // adds data if there is unsent change
          rtps_message.add_data_msg(*seqnum, &self, &reader);

          rtps_message.add_submessage(self.get_heartbeat_msg(
            reader.remote_reader_guid.entityId,
            false,
            false,
          ));

          self.send_unicast_message_to_reader(&rtps_message, reader);
        }
        if reader.multicast_locator_list.len() > 0 {
          let mut RTPSMessage: Message = Message::new(self.create_message_header());
          RTPSMessage.add_submessage(self.get_heartbeat_msg(
            reader.remote_group_entity_id,
            false,
            false,
          ));
          self.send_multicast_message_to_reader(&RTPSMessage, reader);
        }

        seqnums
          .get_mut(&reader.remote_reader_guid)
          .unwrap()
          .push(*seqnum);
      }

      if reader.unsent_changes().len() == 0 {
        let mut rtps_message: Message = Message::new(message_header.clone());

        rtps_message.add_submessage(Writer::get_DST_submessage(
          endianness,
          reader.remote_reader_guid.guidPrefix,
        ));

        rtps_message.add_submessage(self.get_heartbeat_msg(
          reader.remote_reader_guid.entityId,
          false,
          false,
        ));

        self.send_unicast_message_to_reader(&rtps_message, reader);
        self.send_multicast_message_to_reader(&rtps_message, reader);
      }
    }

    let mut readers_guis = HashMap::new();
    for reader in self.readers.iter_mut() {
      let mut sequence_numbers = Vec::new();
      for seqnum in seqnums
        .get(&reader.remote_reader_guid)
        .map(|p| p.clone())
        .iter_mut()
        .flat_map(|p| {
          p.sort();
          p
        })
      {
        reader.remove_unsend_change(*seqnum);
        sequence_numbers.push(*seqnum);
      }
      readers_guis.insert(reader.remote_reader_guid, sequence_numbers);
    }

    for (gui, seqnums) in readers_guis.into_iter() {
      self.increase_heartbeat_counter_and_remove_unsend_sequence_numbers(seqnums, &Some(gui));
    }

    self.set_heartbeat_timer();
  }

  /// after heartbeat is handled timer should be set running again.
  fn set_heartbeat_timer(&mut self) {
    match self.heartbeat_period {
      Some(period) => self.timed_event_handler.as_mut().unwrap().set_timeout(
        &chronoDuration::from_std(period).unwrap(),
        TimerMessageType::writer_heartbeat,
      ),
      None => (),
    }
  }

  pub fn insert_to_history_cache(&mut self, data: DDSData) {
    self.writer_is_disposed = match data.change_kind {
      ChangeKind::ALIVE => false,
      _ => true,
    };

    self.increase_last_change_sequence_number();
    // If first write then set first change sequence number to 1
    if self.first_change_sequence_number == SequenceNumber::from(0) {
      self.first_change_sequence_number = SequenceNumber::from(1);
    }
    let data_key = data.value_key_hash;
    let change_kind = data.change_kind;
    let datavalue = Some(data);
    let new_cache_change = CacheChange::new(
      change_kind,
      self.as_entity().guid,
      self.last_change_sequence_number,
      datavalue,
    );
    let insta = Instant::now();
    self.dds_cache.write().unwrap().to_topic_add_change(
      &self.my_topic_name,
      &insta,
      new_cache_change,
    );
    self
      .sequence_number_to_instant
      .insert(self.last_change_sequence_number, insta.clone());
    self.key_to_instant.insert(data_key, insta.clone());
    self.writer_set_unsent_changes();
  }

  /// This needs to be called when dataWriter does dispose.
  /// This does not remove anything from datacahce but changes the status of writer to disposed.
  pub fn handle_not_alive_disposed_cache_change(&mut self, data: DDSData) {
    let instant = self.key_to_instant.get(&data.value_key_hash);
    self.writer_is_disposed = true;
    if instant.is_some() {
      self
        .dds_cache
        .write()
        .unwrap()
        .from_topic_set_change_to_not_alive_disposed(&self.my_topic_name, &instant.unwrap());
    }
  }

  /// Removes permanently cacheChanges from DDSCache.
  /// CacheChanges can be safely removed only if they are acked by all readers. (Reliable)
  /// Depth is QoS policy History depth.
  /// Returns SequenceNumbers of removed CacheChanges
  /// This is called repeadedly by handle_cache_cleaning action.
  fn remove_all_acked_changes_but_keep_depth(&mut self, depth: i32) -> Vec<SequenceNumber> {
    let amount_need_to_remove;
    let mut removed_change_sequence_numbers = vec![];
    let acked_by_all_readers = {
      //let mut acked_by_all: Vec<(&Instant, &SequenceNumber)> = vec![];
      let mut acked_by_all: BTreeMap<&Instant, &SequenceNumber> = BTreeMap::new();
      for (sq, i) in self.sequence_number_to_instant.iter() {
        if self.change_with_sequence_number_is_acked_by_all(&sq) {
          acked_by_all.insert(i, sq);
        }
      }
      acked_by_all
    };
    if acked_by_all_readers.len() as i32 <= depth {
      return removed_change_sequence_numbers;
    } else {
      amount_need_to_remove = acked_by_all_readers.len() as i32 - depth;
    }
    {
      let mut index: i32 = 0;
      for (i, _sq) in acked_by_all_readers {
        if index >= amount_need_to_remove {
          break;
        }
        let removed = self
          .dds_cache
          .write()
          .unwrap()
          .from_topic_remove_change(&self.my_topic_name, i);
        if removed.is_some() {
          removed_change_sequence_numbers.push(removed.unwrap().sequence_number);
        } else {
          warn!(
            "Remove from {:?} failed. No results with {:?}",
            &self.my_topic_name, i
          );
          debug!("{:?}", self.dds_cache);
        }
        index = index + 1;
      }
    }
    return removed_change_sequence_numbers;
  }

  fn remove_from_history_cache(&mut self, instant: &Instant) {
    let removed_change = self
      .dds_cache
      .write()
      .unwrap()
      .from_topic_remove_change(&self.my_topic_name, instant);
    debug!("removed change from DDShistoryCache {:?}", removed_change);
    if removed_change.is_some() {
      self
        .disposed_sequence_numbers
        .insert(removed_change.unwrap().sequence_number);
    } else {
      todo!();
    }
  }

  fn remove_from_history_cache_with_sequence_number(&mut self, sequence_number: &SequenceNumber) {
    let instant = self.sequence_number_to_instant.get(sequence_number);
    if instant.is_some() {
      let removed_change = self
        .dds_cache
        .write()
        .unwrap()
        .from_topic_remove_change(&self.my_topic_name, instant.unwrap());
      if removed_change.is_none() {
        todo!(
          "Cache change with seqnum {:?} and instant {:?} cold not be revod from DDSCache",
          sequence_number,
          instant
        )
      }
    } else {
      todo!(
        "sequence number: {:?} cannot be tranformed to instant ",
        sequence_number
      );
    }
  }

  fn increase_last_change_sequence_number(&mut self) {
    self.last_change_sequence_number = self.last_change_sequence_number + SequenceNumber::from(1);
  }

  fn increase_heartbeat_counter(&mut self) {
    self.heartbeat_message_counter = self.heartbeat_message_counter + 1;
  }

  pub fn can_send_some(&self) -> bool {
    // When writer is reliable all changes has to be acnowledged by remote reader before sending new messages.
    if self.is_reliable() {
      let last_change_is_acked: bool =
        self.change_with_sequence_number_is_acked_by_all(&self.last_change_sequence_number);

      if last_change_is_acked {
        for reader_proxy in &self.readers {
          if reader_proxy.can_send() {
            return true;
          }
        }
        return false;
      }
    }
    //Note that for a Best-Effort Writer, W::pushMode == true, as there are no acknowledgements. Therefore, the
    //Writer always pushes out data as it becomes available.
    else {
      for reader_proxy in &self.readers {
        if reader_proxy.can_send() {
          return true;
        }
      }
      return false;
    }
    return false;
  }

  pub fn sequence_is_sent_to_all_readers(&self, sequence_number: SequenceNumber) -> bool {
    for reader_proxy in &self.readers {
      if reader_proxy.unsent_changes().contains(&sequence_number) {
        return false;
      }
    }
    return true;
  }

  pub fn sequence_needs_to_be_send_to(
    &self,
    sequence_number: SequenceNumber,
  ) -> Vec<&RtpsReaderProxy> {
    let mut readers_remaining: Vec<&RtpsReaderProxy> = vec![];
    for reader_proxy in &self.readers {
      if reader_proxy.unsent_changes().contains(&sequence_number) {
        readers_remaining.push(&reader_proxy);
      }
    }
    return readers_remaining;
  }

  fn get_some_reader_with_unsent_messages(&self) -> Option<&RtpsReaderProxy> {
    self.readers.iter().find(|p| p.can_send())
  }

  fn get_some_reader_with_unsent_messages_mut(&mut self) -> Option<&mut RtpsReaderProxy> {
    self.readers.iter_mut().find(|p| p.can_send())
  }

  fn generate_message(&self, reader_proxy: &RtpsReaderProxy) -> Option<Message> {
    if reader_proxy.can_send() {
      let sequenceNumber = match reader_proxy.next_unsent_change() {
        Some(s) => s,
        None => return None,
      };

      let instant = match self.sequence_number_to_instant.get(&sequenceNumber) {
        Some(i) => i,
        None => return None,
      };

      let cache = match self.dds_cache.read() {
        Ok(c) => c,
        Err(e) => panic!("DDSCache is poisoned. {:?}", e),
      };

      let change = match cache.from_topic_get_change(&self.my_topic_name, &instant) {
        Some(c) => c,
        None => return None,
      };

      let reader_entity_id = reader_proxy.remote_reader_guid.entityId;
      let message = self.write_user_msg(change.clone(), reader_entity_id);

      return Some(message);
    }
    None
  }

  fn get_next_reader_next_unsend_message(&self) -> Option<(Message, GUID)> {
    self.readers.iter().find(|p| p.can_send()).map(|p| {
      let sequenceNumber = p.next_unsent_change();
      let instant = self
        .sequence_number_to_instant
        .get(&sequenceNumber.unwrap());
      let cache = self.dds_cache.read().unwrap();
      let change = cache.from_topic_get_change(&self.my_topic_name, &instant.unwrap());
      let reader_entity_id = p.remote_reader_guid.entityId.clone();
      let remote_reader_guid = p.remote_reader_guid.clone();
      let message = self.write_user_msg(change.unwrap().clone(), reader_entity_id);
      return (message, remote_reader_guid);
    })
  }

  fn get_next_reader_next_requested_message(&mut self) -> (Option<Message>, Option<GUID>) {
    for reader_proxy in &mut self.readers {
      if reader_proxy.can_send() {
        let sequenceNumber = reader_proxy.next_requested_change();
        let instant = self.sequence_number_to_instant.get(sequenceNumber.unwrap());
        let cache = self.dds_cache.read().unwrap();
        let change = cache.from_topic_get_change(&self.my_topic_name, &instant.unwrap());
        let message: Message;
        let reader_entity_id = reader_proxy.remote_reader_guid.entityId.clone();
        let remote_reader_guid = reader_proxy.remote_reader_guid.clone();
        {
          message = self.write_user_msg(change.unwrap().clone(), reader_entity_id);
        }
        return (Some(message), Some(remote_reader_guid));
      }
    }
    return (None, None);
  }

  fn send_next_unsend_message(&mut self) {
    let mut multi_cast_locators: Vec<Locator> = vec![];
    let mut buffer: Vec<u8> = vec![];
    let mut context = self.endianness;

    if self.endianness == Endianness::BigEndian {
      context = Endianness::BigEndian
    }

    let mut remote_reader_guid = None;
    let mut rem_sequece_number: Option<SequenceNumber> = None;
    let mut message_sequence_numbers = Vec::new();

    if let Some(reader) = self.get_some_reader_with_unsent_messages() {
      rem_sequece_number = reader.next_unsent_change();

      remote_reader_guid = Some(reader.remote_reader_guid);
      let message = self.generate_message(reader);
      if let Some(message) = message {
        message_sequence_numbers = message.get_data_sub_message_sequence_numbers();
        if let Ok(data) = message.write_to_vec_with_ctx(context) {
          buffer = data;
        }
      }

      self
        .udp_sender
        .send_to_locator_list(&buffer, &reader.unicast_locator_list);

      for loc in reader.multicast_locator_list.iter() {
        if loc.kind == LocatorKind::LOCATOR_KIND_UDPv4 {
          multi_cast_locators.push(loc.clone())
        }
      }

      for l in multi_cast_locators {
        if l.kind == LocatorKind::LOCATOR_KIND_UDPv4 {
          let a = l.to_socket_address();
          match self.udp_sender.send_ipv4_multicast(&buffer, a) {
            Ok(_) => (),
            Err(e) => error!("Unable to send buffer to multicast {:?}. {:?}", a, e),
          }
        }
      }
    }

    if let Some(rem_seq) = rem_sequece_number {
      if let Some(reader) = self.get_some_reader_with_unsent_messages_mut() {
        reader.remove_unsend_change(rem_seq);
      }

      if let Some(remote_reader_guid) = remote_reader_guid {
        self.increase_heartbeat_counter_and_remove_unsend_sequence_numbers(
          message_sequence_numbers,
          &Some(remote_reader_guid),
        )
      }
    }
  }

  fn send_unicast_message_to_reader(&self, message: &Message, reader: &RtpsReaderProxy) {
    let buffer = message.write_to_vec_with_ctx(self.endianness).unwrap();
    self
      .udp_sender
      .send_to_locator_list(&buffer, &reader.unicast_locator_list)
  }

  fn send_multicast_message_to_reader(&self, message: &Message, reader: &RtpsReaderProxy) {
    let buffer = message.write_to_vec_with_ctx(self.endianness).unwrap();
    for multiaddress in &reader.multicast_locator_list {
      if multiaddress.kind == LocatorKind::LOCATOR_KIND_UDPv4 {
        self
          .udp_sender
          .send_ipv4_multicast(&buffer, multiaddress.to_socket_address())
          .expect("Unable to send multicast message.");
      } else if multiaddress.kind == LocatorKind::LOCATOR_KIND_UDPv6 {
        todo!();
      }
    }
  }

  pub fn send_all_unsend_messages(&mut self) {
    if self.can_send_some() {
      while let Some(_) = self.get_some_reader_with_unsent_messages() {
        self.send_next_unsend_message();
      }
    } else {
      // TODO: maybe add debug print when necessary
    }
  }

  fn create_message_header(&self) -> Header {
    let head: Header = Header {
      protocol_id: ProtocolId::default(),
      protocol_version: ProtocolVersion {
        major: self.source_version.major,
        minor: self.source_version.minor,
      },
      vendor_id: VendorId {
        vendorId: self.source_vendor_id.vendorId,
      },
      guid_prefix: self.entity_attributes.guid.guidPrefix,
    };
    return head;
  }

  pub fn get_TS_submessage(&self, invalidiateFlagSet: bool) -> SubMessage {
    let currentTime: Timespec = get_time();
    let timestamp = InfoTimestamp {
      timestamp: Timestamp::from(currentTime),
    };
    let mes = &mut timestamp.write_to_vec_with_ctx(self.endianness).unwrap();

    let flags = BitFlags::<INFOTIMESTAMP_Flags>::from_endianness(self.endianness)
      | (if invalidiateFlagSet {
        INFOTIMESTAMP_Flags::Invalidate.into()
      } else {
        BitFlags::<INFOTIMESTAMP_Flags>::empty()
      });

    let submessageHeader = SubmessageHeader {
      kind: SubmessageKind::INFO_TS,
      flags: flags.bits(),
      content_length: mes.len() as u16, // This conversion should be safe, as timestamp length cannot exceed u16
    };
    SubMessage {
      header: submessageHeader,
      body: SubmessageBody::Interpreter(InterpreterSubmessage::InfoTimestamp(timestamp, flags)),
    }
  }

  pub fn get_DST_submessage(endianness: Endianness, guid_prefix: GuidPrefix) -> SubMessage {
    let flags = BitFlags::<INFODESTINATION_Flags>::from_endianness(endianness);
    let submessageHeader = SubmessageHeader {
      kind: SubmessageKind::INFO_DST,
      flags: flags.bits(),
      content_length: 12u16,
      //InfoDST length is always 12 because message contains only GuidPrefix
    };
    SubMessage {
      header: submessageHeader,
      body: SubmessageBody::Interpreter(InterpreterSubmessage::InfoDestination(
        InfoDestination { guid_prefix },
        flags,
      )),
    }
  }

  pub fn get_DATA_msg_from_cache_change(
    &self,
    change: CacheChange,
    reader_entity_id: EntityId,
  ) -> SubMessage {
    // let mut representationIdentifierBytes: [u8; 2] = [0, 0];
    // if self.endianness == Endianness::LittleEndian {
    //   representationIdentifierBytes = [0x00, 0x01];
    // } else if self.endianness == Endianness::BigEndian {
    //   representationIdentifierBytes = [0x00, 0x00];
    // }

    // TODO: might want check representation identifier again
    //data_message.serialized_payload.representation_identifier =
    //  SerializedPayload::representation_identifier_from(representationIdentifierBytes[1] as u16);

    //The current version of the protocol (2.3) does not use the representation_options: The sender shall set the representation_options to zero.
    //data_message.serialized_payload.representation_options = 0u16;
    //data_message.serialized_payload.value = change.data_value.unwrap().value.clone();
    //data_message.reader_id = reader_entity_id;
    //data_message.writer_sn = change.sequence_number;

    let inline_qos = match change.kind {
      ChangeKind::ALIVE => None,
      _ => {
        let mut param_list = ParameterList::new();
        let key_hash = Parameter {
          parameter_id: ParameterId::PID_KEY_HASH,
          value: change.key.to_le_bytes().to_vec(),
        };
        param_list.parameters.push(key_hash);
        let status_info = Parameter::create_pid_status_info_parameter(true, true, false);
        param_list.parameters.push(status_info);
        Some(param_list)
      }
    };

    let mut data_message = Data {
      reader_id: reader_entity_id,
      writer_id: self.get_entity_id(), // TODO! Is this the correct EntityId here?
      writer_sn: change.sequence_number,
      inline_qos,
      serialized_payload: change.data_value,
    };

    if self.get_entity_id().get_kind() == 0xC2 {
      match data_message.serialized_payload.as_mut() {
        Some(sp) => sp.representation_identifier = u16::from(RepresentationIdentifier::PL_CDR_LE),
        None => (),
      }
    }

    let flags: BitFlags<DATA_Flags> = BitFlags::<DATA_Flags>::from_endianness(self.endianness)
      | (
        if change.kind == ChangeKind::NOT_ALIVE_DISPOSED {
          // No data, we send key instead
          BitFlags::<DATA_Flags>::from_flag(DATA_Flags::InlineQos)
        } else {
          BitFlags::<DATA_Flags>::from_flag(DATA_Flags::Data)
        }
        // normal case
      );

    let size = data_message
      .write_to_vec_with_ctx(self.endianness)
      .unwrap()
      .len() as u16;
    let s: SubMessage = SubMessage {
      header: SubmessageHeader {
        kind: SubmessageKind::DATA,
        flags: flags.bits(),
        content_length: size,
      },
      body: SubmessageBody::Entity(crate::submessages::EntitySubmessage::Data(
        data_message,
        flags,
      )),
    };

    return s;
  }

  pub fn get_heartbeat_msg(
    &self,
    reader_id: EntityId,
    set_final_flag: bool,
    set_liveliness_flag: bool,
  ) -> SubMessage {
    let first = self.first_change_sequence_number;
    let last = self.last_change_sequence_number;

    let heartbeat = Heartbeat {
      reader_id: reader_id,
      writer_id: self.entity_attributes.guid.entityId,
      first_sn: first,
      last_sn: last,
      count: self.heartbeat_message_counter,
    };

    let mut flags = BitFlags::<HEARTBEAT_Flags>::from_endianness(self.endianness);

    if set_final_flag {
      flags.insert(HEARTBEAT_Flags::Final)
    }
    if set_liveliness_flag {
      flags.insert(HEARTBEAT_Flags::Liveliness)
    }

    let mes = &mut heartbeat.write_to_vec_with_ctx(self.endianness).unwrap();
    let size = mes.len();

    SubMessage {
      header: SubmessageHeader {
        kind: SubmessageKind::HEARTBEAT,
        flags: flags.bits(),
        content_length: size as u16, // cannot overflow, heartbeat has fixed size
      },
      body: SubmessageBody::Entity(EntitySubmessage::Heartbeat(heartbeat, flags)),
    }
  }

  pub fn write_user_msg(&self, change: CacheChange, reader_entity_id: EntityId) -> Message {
    let mut message: Vec<u8> = vec![];

    let mut RTPSMessage: Message = Message::new(self.create_message_header());
    RTPSMessage.add_submessage(self.get_TS_submessage(false));
    let data = self.get_DATA_msg_from_cache_change(change.clone(), reader_entity_id);
    RTPSMessage.add_submessage(data);
    //RTPSMessage.add_submessage(self.get_heartbeat_msg());
    message.append(&mut RTPSMessage.write_to_vec_with_ctx(self.endianness).unwrap());

    return RTPSMessage;
  }

  /// AckNack Is negative if reader_sn_state contains some sequenceNumbers in reader_sn_state set
  fn test_if_ack_nack_contains_not_recieved_sequence_numbers(ack_nack: &AckNack) -> bool {
    if !&ack_nack.reader_sn_state.set.is_empty() {
      return true;
    }
    return false;
  }

  ///When receiving an ACKNACK Message indicating a Reader is missing some data samples, the Writer must
  ///respond by either sending the missing data samples, sending a GAP message when the sample is not relevant, or
  ///sending a HEARTBEAT message when the sample is no longer available
  pub fn handle_ack_nack(&mut self, guid_prefix: GuidPrefix, an: AckNack) {
    if !self.is_reliable() {
      error!(
        "Writer {:x?} is best effort! It should not handle acknack messages!",
        self.get_entity_id()
      );
      return;
    }

    if let Some(reader_proxy) = self.matched_reader_lookup(guid_prefix, an.reader_id) {
      if Writer::test_if_ack_nack_contains_not_recieved_sequence_numbers(&an) {
        // if ack nac says reader has NOT recieved data then add data to requested changes
        reader_proxy.add_requested_changes(an.reader_sn_state.set);
      } else {
        reader_proxy.acked_changes_set(an.reader_sn_state.base);
      }
    }
  }

  pub fn matched_reader_add(&mut self, reader_proxy: RtpsReaderProxy) {
    if self.readers.iter().any(|x| {
      x.remote_group_entity_id == reader_proxy.remote_group_entity_id
        && x.remote_reader_guid == reader_proxy.remote_reader_guid
    }) {
      panic!("Reader proxy with same group entityid and remotereader guid added already");
    };
    &self.readers.push(reader_proxy);
  }

  pub fn matched_reader_remove(&mut self, reader_proxy: RtpsReaderProxy) {
    let pos = &self.readers.iter().position(|x| {
      x.remote_group_entity_id == reader_proxy.remote_group_entity_id
        && x.remote_reader_guid == reader_proxy.remote_reader_guid
    });
    if pos.is_some() {
      &self.readers.remove(pos.unwrap());
    }
  }

  ///This operation finds the ReaderProxy with GUID_t a_reader_guid from the set
  /// get guid Prefix from RTPS message main header
  /// get reader guid from AckNack submessage readerEntityId
  pub fn matched_reader_lookup(
    &mut self,
    guid_prefix: GuidPrefix,
    reader_entity_id: EntityId,
  ) -> Option<&mut RtpsReaderProxy> {
    let search_guid: GUID = GUID::new_with_prefix_and_id(guid_prefix, reader_entity_id);
    self
      .readers
      .iter_mut()
      .find(|x| x.remote_reader_guid == search_guid)
  }

  pub fn reader_lookup(
    &self,
    guid_prefix: GuidPrefix,
    reader_entity_id: EntityId,
  ) -> Option<&RtpsReaderProxy> {
    let search_guid: GUID = GUID::new_with_prefix_and_id(guid_prefix, reader_entity_id);
    self
      .readers
      .iter()
      .find(|x| x.remote_reader_guid == search_guid)
  }

  ///This operation takes a CacheChange a_change as a parameter and determines whether all the ReaderProxy
  ///have acknowledged the CacheChange. The operation will return true if all ReaderProxy have acknowledged the
  ///corresponding CacheChange and false otherwise.
  pub fn is_acked_by_all(&self, cache_change: &CacheChange) -> bool {
    for _proxy in &self.readers {
      if _proxy.sequence_is_acked(&cache_change.sequence_number) == false {
        return false;
      }
    }
    return true;
  }

  pub fn change_with_sequence_number_is_acked_by_all(
    &self,
    sequence_number: &SequenceNumber,
  ) -> bool {
    for proxy in &self.readers {
      if proxy.sequence_is_acked(sequence_number) == false {
        return false;
      }
    }
    return true;
  }

  pub fn increase_heartbeat_counter_and_remove_unsend_sequence_numbers(
    &mut self,
    sequence_numbers: Vec<SequenceNumber>,
    remote_reader_guid: &Option<GUID>,
  ) {
    let sequenceNumbersCount: usize = { sequence_numbers.len() };
    if remote_reader_guid.is_some() {
      let readerProxy = self.matched_reader_lookup(
        remote_reader_guid.unwrap().guidPrefix,
        remote_reader_guid.unwrap().entityId,
      );
      if readerProxy.is_some() {
        let reader_prox = readerProxy.unwrap();
        for sequence_number in sequence_numbers {
          reader_prox.remove_unsend_change(sequence_number);
        }
      }
    }
    if remote_reader_guid.is_some() {
      for _x in 0..sequenceNumbersCount {
        self.increase_heartbeat_counter();
      }
      if sequenceNumbersCount == 0 {
        self.increase_heartbeat_counter();
      }
    }
  }

  pub fn increase_heartbeat_counter_and_remove_unsend(
    &mut self,
    message: &Option<Message>,
    remote_reader_guid: &Option<GUID>,
  ) {
    if message.is_some() {
      let sequence_numbers = message
        .as_ref()
        .unwrap()
        .get_data_sub_message_sequence_numbers();
      for sq in sequence_numbers {
        let readerProxy = self
          .matched_reader_lookup(
            remote_reader_guid.unwrap().guidPrefix,
            remote_reader_guid.unwrap().entityId,
          )
          .unwrap();
        readerProxy.remove_unsend_change(sq)
      }
    }
  }

  pub fn writer_set_unsent_changes(&mut self) {
    for reader in &mut self.readers {
      reader.unsend_changes_set(self.last_change_sequence_number.clone());
    }
  }

  pub fn sequence_number_to_instant(&self, seqnumber: SequenceNumber) -> Option<&Instant> {
    self.sequence_number_to_instant.get(&seqnumber)
  }

  pub fn find_cache_change(&self, instant: &Instant) -> Option<CacheChange> {
    match self.dds_cache.read() {
      Ok(dc) => {
        let cc = dc.from_topic_get_change(&self.my_topic_name, instant);
        cc.cloned()
      }
      Err(e) => panic!("DDSCache is poisoned {:?}", e),
    }
  }

  pub fn topic_name(&self) -> &String {
    &self.my_topic_name
  }
}

impl Entity for Writer {
  fn as_entity(&self) -> &crate::structure::entity::EntityAttributes {
    &self.entity_attributes
  }
}

impl Endpoint for Writer {
  fn as_endpoint(&self) -> &crate::structure::endpoint::EndpointAttributes {
    &self.endpoint_attributes
  }
}

impl HasQoSPolicy for Writer {
  fn get_qos(&self) -> &QosPolicies {
    &self.qos_policies
  }

  fn set_qos(&mut self, new_qos: &QosPolicies) -> super::values::result::Result<()> {
    self.qos_policies = new_qos.clone();
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use crate::{
    dds::{qos::QosPolicies, participant::DomainParticipant, typedesc::TypeDesc},
  };
  use std::thread;
  use crate::test::random_data::*;
  use crate::dds::datawriter::DataWriter;
  use crate::serialization::cdrSerializer::CDR_serializer_adapter;
  use byteorder::LittleEndian;
  use log::info;

  #[test]
  fn test_writer_recieves_datawriter_cache_change_notifications() {
    let domain_participant = DomainParticipant::new(4, 0);
    let qos = QosPolicies::qos_none();
    let _default_dw_qos = QosPolicies::qos_none();
    thread::sleep(time::Duration::milliseconds(100).to_std().unwrap());

    let publisher = domain_participant
      .create_publisher(&qos)
      .expect("Failed to create publisher");
    let topic = domain_participant
      .create_topic("Aasii", TypeDesc::new("Huh?".to_string()), &qos)
      .expect("Failed to create topic");
    let mut data_writer: DataWriter<
      '_,
      RandomData,
      CDR_serializer_adapter<RandomData, LittleEndian>,
    > = publisher
      .create_datawriter(None, &topic, &qos)
      .expect("Failed to create datawriter");

    let data = RandomData {
      a: 4,
      b: "Fobar".to_string(),
    };

    let data2 = RandomData {
      a: 2,
      b: "Fobar".to_string(),
    };

    let data3 = RandomData {
      a: 3,
      b: "Fobar".to_string(),
    };
    thread::sleep(time::Duration::milliseconds(100).to_std().unwrap());
    let writeResult = data_writer.write(data, None).expect("Unable to write data");

    info!("writerResult:  {:?}", writeResult);
    thread::sleep(time::Duration::milliseconds(100).to_std().unwrap());
    let writeResult = data_writer
      .write(data2, None)
      .expect("Unable to write data");

    thread::sleep(time::Duration::milliseconds(100).to_std().unwrap());
    info!("writerResult:  {:?}", writeResult);
    let writeResult = data_writer
      .write(data3, None)
      .expect("Unable to write data");

    thread::sleep(time::Duration::milliseconds(100).to_std().unwrap());
    info!("writerResult:  {:?}", writeResult);
  }
}
