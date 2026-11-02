// Copyright 2026 The Matrix.org Foundation C.I.C.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Counting the messages of a room that couldn't be decrypted, so they can't
//! be searched.

use matrix_sdk_base::{
    cross_process_lock::MappedCrossProcessLockState,
    event_cache::store::extract_event_relation,
    linked_chunk::{ChunkContent, LinkedChunkId, RawChunk},
};
use matrix_sdk_common::deserialized_responses::TimelineEvent;
use ruma::{OwnedEventId, RoomId};

use super::{EventCache, EventCacheError, Result};

/// The relation type of the events that belong to a thread.
const THREAD_REL_TYPE: &str = "m.thread";

/// Whether an event that couldn't be decrypted counts as a message of the
/// room.
///
/// An encrypted event keeps its `m.relates_to` in clear, so a reaction
/// (`m.annotation`), an edit (`m.replace`) or any other relation but a thread
/// can be told from a message without decrypting it. A reply has no
/// `rel_type` at all, so it is a message, and so is an event of a thread.
pub(super) fn is_undecrypted_message(event: &TimelineEvent) -> bool {
    has_message_relation(event)
}

/// Whether an event is in no relation to another one, or in a thread: that is
/// what a message looks like, as opposed to a reaction, an edit or any other
/// relation.
fn has_message_relation(event: &TimelineEvent) -> bool {
    match extract_event_relation(event.raw()) {
        None => true,
        Some((_, rel_type)) => rel_type == THREAD_REL_TYPE,
    }
}

/// The types of the events that are messages, when they can be read.
const READABLE_MESSAGE_TYPES: [&str; 4] =
    ["m.room.message", "m.sticker", "m.poll.start", "org.matrix.msc3381.poll.start"];

/// Whether an event that could be decrypted, or that was never encrypted, is a
/// message that can be read. The state events, and the reactions, edits and
/// other relations are not.
fn is_readable_message(event: &TimelineEvent) -> bool {
    event
        .kind
        .event_type()
        .is_some_and(|event_type| READABLE_MESSAGE_TYPES.contains(&event_type.as_str()))
        && has_message_relation(event)
}

impl EventCache {
    /// Count the messages of a room that couldn't be decrypted, so they
    /// can't be searched.
    ///
    /// The reactions, edits and other relations that couldn't be decrypted
    /// are not counted, as they are not messages. The count comes straight
    /// from the event cache store, so it doesn't load the room.
    pub async fn count_undecrypted_messages(&self, room_id: &RoomId) -> Result<u64> {
        let Some(client) = self.inner.client.get() else {
            return Err(EventCacheError::ClientDropped);
        };

        let store = match client.event_cache_store().lock().await? {
            MappedCrossProcessLockState::Clean(store)
            | MappedCrossProcessLockState::Dirty(store) => store,
        };

        let events = store.get_room_events(room_id, Some("m.room.encrypted"), None).await?;

        let count =
            events.iter().filter(|event| event.kind.is_utd() && is_undecrypted_message(event));

        Ok(count.count() as u64)
    }

    /// Find the ranges of consecutive messages of a room that could not be
    /// decrypted, from the oldest to the newest.
    ///
    /// The ranges come straight from the chunks of the event cache store, so
    /// they do not load the room.
    pub async fn undecrypted_ranges(&self, room_id: &RoomId) -> Result<Vec<UndecryptedRange>> {
        let Some(client) = self.inner.client.get() else {
            return Err(EventCacheError::ClientDropped);
        };

        let store = match client.event_cache_store().lock().await? {
            MappedCrossProcessLockState::Clean(store)
            | MappedCrossProcessLockState::Dirty(store) => store,
        };

        let chunks = store.load_all_chunks(LinkedChunkId::Room(room_id)).await?;

        Ok(undecrypted_ranges(&chunks))
    }
}

/// A run of consecutive messages of a room that couldn't be decrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct UndecryptedRange {
    /// The oldest message of the range.
    pub first_event_id: OwnedEventId,
    /// The newest message of the range.
    pub last_event_id: OwnedEventId,
    /// The number of messages in the range.
    pub message_count: u64,
}

/// Find the ranges of consecutive messages that couldn't be decrypted in the
/// chunks of a room.
///
/// The chunks come in any order: they are followed from the first one, using
/// their links. A range is closed by a gap, or by a message that can be read.
/// The other events that can be read, such as the state events, don't close
/// it.
/// The reactions, edits and other relations that couldn't be decrypted
/// neither break a range nor are counted in it. The ranges are listed from the
/// oldest to the newest.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn undecrypted_ranges<Gap>(
    chunks: &[RawChunk<TimelineEvent, Gap>],
) -> Vec<UndecryptedRange> {
    let mut ranges = Vec::new();
    let mut open: Option<UndecryptedRange> = None;

    let mut current = chunks.iter().find(|chunk| chunk.previous.is_none());

    // Bounded by the number of chunks, so a corrupted link can't loop forever.
    for _ in 0..chunks.len() {
        let Some(chunk) = current else { break };

        match &chunk.content {
            ChunkContent::Gap(_) => ranges.extend(open.take()),
            ChunkContent::Items(events) => {
                for event in events {
                    if !event.kind.is_utd() {
                        if is_readable_message(event) {
                            ranges.extend(open.take());
                        }
                    } else if is_undecrypted_message(event)
                        && let Some(event_id) = event.event_id()
                    {
                        if let Some(range) = open.as_mut() {
                            range.last_event_id = event_id.to_owned();
                            range.message_count += 1;
                        } else {
                            open = Some(UndecryptedRange {
                                first_event_id: event_id.to_owned(),
                                last_event_id: event_id.to_owned(),
                                message_count: 1,
                            });
                        }
                    }
                }
            }
        }

        current = chunk
            .next
            .as_ref()
            .and_then(|next| chunks.iter().find(|candidate| &candidate.identifier == next));
    }

    ranges.extend(open);
    ranges
}

#[cfg(test)]
mod tests {
    use matrix_sdk_base::{
        cross_process_lock::MappedCrossProcessLockState,
        linked_chunk::{ChunkContent, ChunkIdentifier, RawChunk},
    };
    use matrix_sdk_common::deserialized_responses::{
        TimelineEvent, UnableToDecryptInfo, UnableToDecryptReason,
    };
    use matrix_sdk_test::{async_test, event_factory::EventFactory};
    use ruma::{RoomId, event_id, events::AnySyncTimelineEvent, room_id, serde::Raw, user_id};
    use serde_json::{Value, json};

    use super::{UndecryptedRange, is_undecrypted_message, undecrypted_ranges};
    use crate::test_utils::mocks::MatrixMockServer;

    /// An `m.room.encrypted` event that couldn't be decrypted, with an
    /// optional `m.relates_to` in its clear content.
    fn a_utd(relation: Option<Value>) -> TimelineEvent {
        a_utd_with_id("$event:example.org", relation)
    }

    /// Same as [`a_utd`], with the given event id.
    fn a_utd_with_id(event_id: &str, relation: Option<Value>) -> TimelineEvent {
        let mut content = json!({
            "algorithm": "m.megolm.v1.aes-sha2",
            "ciphertext": "",
            "sender_key": "curve_key",
            "device_id": "DEVICEID",
            "session_id": "SESSION",
        });
        if let Some(relation) = relation {
            content["m.relates_to"] = relation;
        }

        let event = json!({
            "type": "m.room.encrypted",
            "event_id": event_id,
            "sender": "@alice:example.org",
            "origin_server_ts": 1,
            "content": content,
        });
        let raw: Raw<AnySyncTimelineEvent> = serde_json::from_str(&event.to_string()).unwrap();

        let utd_info = UnableToDecryptInfo {
            session_id: Some("SESSION".to_owned()),
            reason: UnableToDecryptReason::MissingMegolmSession { withheld_code: None },
        };

        TimelineEvent::from_utd(raw, utd_info)
    }

    #[test]
    fn test_event_without_relation_is_a_message() {
        assert!(is_undecrypted_message(&a_utd(None)));
    }

    #[test]
    fn test_reply_is_a_message() {
        let reply = json!({ "m.in_reply_to": { "event_id": "$target:example.org" } });

        assert!(is_undecrypted_message(&a_utd(Some(reply))));
    }

    #[test]
    fn test_thread_event_is_a_message() {
        let thread = json!({
            "rel_type": "m.thread",
            "event_id": "$root:example.org",
            "m.in_reply_to": { "event_id": "$previous:example.org" },
            "is_falling_back": true,
        });

        assert!(is_undecrypted_message(&a_utd(Some(thread))));
    }

    #[test]
    fn test_reaction_is_not_a_message() {
        let reaction = json!({
            "rel_type": "m.annotation",
            "event_id": "$target:example.org",
            "key": "👍",
        });

        assert!(!is_undecrypted_message(&a_utd(Some(reaction))));
    }

    #[test]
    fn test_edit_is_not_a_message() {
        let edit = json!({ "rel_type": "m.replace", "event_id": "$target:example.org" });

        assert!(!is_undecrypted_message(&a_utd(Some(edit))));
    }

    #[test]
    fn test_reference_is_not_a_message() {
        let reference = json!({ "rel_type": "m.reference", "event_id": "$target:example.org" });

        assert!(!is_undecrypted_message(&a_utd(Some(reference))));
    }

    /// Only the messages of the room that couldn't be decrypted are counted,
    /// not their reactions or edits, nor the events of the other rooms.
    #[async_test]
    async fn test_count_undecrypted_messages_of_a_room() {
        let server = MatrixMockServer::new().await;
        let client = server.client_builder().build().await;

        let room_id = room_id!("!room:example.org");
        let other_room_id = room_id!("!other:example.org");

        let reaction = json!({ "rel_type": "m.annotation", "event_id": "$1", "key": "👍" });
        let edit = json!({ "rel_type": "m.replace", "event_id": "$1" });

        {
            let store = match client.event_cache_store().lock().await.unwrap() {
                MappedCrossProcessLockState::Clean(store)
                | MappedCrossProcessLockState::Dirty(store) => store,
            };
            store.save_event(room_id, a_utd_with_id("$1", None)).await.unwrap();
            store.save_event(room_id, a_utd_with_id("$2", Some(reaction))).await.unwrap();
            store.save_event(room_id, a_utd_with_id("$3", Some(edit))).await.unwrap();
            store.save_event(room_id, a_utd_with_id("$4", None)).await.unwrap();
            store.save_event(other_room_id, a_utd_with_id("$5", None)).await.unwrap();
        }

        let event_cache = client.event_cache().unwrap();

        assert_eq!(event_cache.count_undecrypted_messages(room_id).await.unwrap(), 2);
        assert_eq!(event_cache.count_undecrypted_messages(other_room_id).await.unwrap(), 1);
    }

    /// Save an event in the event cache store of the client.
    async fn save(client: &crate::Client, room_id: &RoomId, event: TimelineEvent) {
        let store = match client.event_cache_store().lock().await.unwrap() {
            MappedCrossProcessLockState::Clean(store)
            | MappedCrossProcessLockState::Dirty(store) => store,
        };
        store.save_event(room_id, event).await.unwrap();
    }

    /// On a SQLite store, a message that couldn't be decrypted stops being
    /// counted once its decrypted event replaces it.
    #[async_test]
    async fn test_decrypted_message_is_no_longer_counted_on_sqlite() {
        let server = MatrixMockServer::new().await;
        let tmp_dir = tempfile::tempdir().unwrap();
        let client = server
            .client_builder()
            .on_builder(|builder| builder.sqlite_store(tmp_dir.path(), None))
            .build()
            .await;

        // SQLite did create something in the directory.
        assert!(std::fs::read_dir(tmp_dir.path()).unwrap().count() > 0);

        let room_id = room_id!("!room:example.org");

        save(&client, room_id, a_utd_with_id("$1", None)).await;
        save(&client, room_id, a_utd_with_id("$2", None)).await;

        let event_cache = client.event_cache().unwrap();
        assert_eq!(event_cache.count_undecrypted_messages(room_id).await.unwrap(), 2);

        // The room key arrives: the first event gets decrypted, and replaces
        // the undecryptable one in the store.
        let decrypted = EventFactory::new()
            .room(room_id)
            .sender(user_id!("@alice:example.org"))
            .text_msg("hello")
            .event_id(event_id!("$1"))
            .into();
        save(&client, room_id, decrypted).await;

        assert_eq!(event_cache.count_undecrypted_messages(room_id).await.unwrap(), 1);
    }

    /// A message that was decrypted, so it can be read.
    fn a_readable_message(event_id: &str) -> TimelineEvent {
        EventFactory::new()
            .room(room_id!("!room:example.org"))
            .sender(user_id!("@alice:example.org"))
            .text_msg("hello")
            .event_id(&ruma::EventId::parse(event_id).unwrap())
            .into()
    }

    /// A reaction that couldn't be decrypted.
    fn an_undecrypted_reaction(event_id: &str) -> TimelineEvent {
        let reaction = json!({ "rel_type": "m.annotation", "event_id": "$target", "key": "👍" });
        a_utd_with_id(event_id, Some(reaction))
    }

    /// A chunk of events, linked to the given chunks.
    fn items_chunk(
        id: u64,
        previous: Option<u64>,
        next: Option<u64>,
        events: Vec<TimelineEvent>,
    ) -> RawChunk<TimelineEvent, ()> {
        RawChunk {
            content: ChunkContent::Items(events),
            previous: previous.map(ChunkIdentifier::new),
            identifier: ChunkIdentifier::new(id),
            next: next.map(ChunkIdentifier::new),
        }
    }

    /// A gap, linked to the given chunks.
    fn gap_chunk(id: u64, previous: Option<u64>, next: Option<u64>) -> RawChunk<TimelineEvent, ()> {
        RawChunk {
            content: ChunkContent::Gap(()),
            previous: previous.map(ChunkIdentifier::new),
            identifier: ChunkIdentifier::new(id),
            next: next.map(ChunkIdentifier::new),
        }
    }

    /// The expected range from the first to the last event id.
    fn range(first: &str, last: &str, message_count: u64) -> UndecryptedRange {
        UndecryptedRange {
            first_event_id: ruma::EventId::parse(first).unwrap(),
            last_event_id: ruma::EventId::parse(last).unwrap(),
            message_count,
        }
    }

    #[test]
    fn test_ranges_without_chunks() {
        assert_eq!(undecrypted_ranges::<()>(&[]), vec![]);
    }

    #[test]
    fn test_ranges_of_a_room_without_undecrypted_messages() {
        let chunks = [items_chunk(1, None, None, vec![a_readable_message("$1")])];

        assert_eq!(undecrypted_ranges(&chunks), vec![]);
    }

    #[test]
    fn test_ranges_consecutive_undecrypted_messages_make_one_range() {
        let events =
            vec![a_utd_with_id("$1", None), a_utd_with_id("$2", None), a_utd_with_id("$3", None)];
        let chunks = [items_chunk(1, None, None, events)];

        assert_eq!(undecrypted_ranges(&chunks), vec![range("$1", "$3", 3)]);
    }

    #[test]
    fn test_ranges_a_readable_message_splits_a_range() {
        let events =
            vec![a_utd_with_id("$1", None), a_readable_message("$2"), a_utd_with_id("$3", None)];
        let chunks = [items_chunk(1, None, None, events)];

        assert_eq!(undecrypted_ranges(&chunks), vec![range("$1", "$1", 1), range("$3", "$3", 1)]);
    }

    #[test]
    fn test_ranges_a_gap_splits_a_range() {
        let chunks = [
            items_chunk(1, None, Some(2), vec![a_utd_with_id("$1", None)]),
            gap_chunk(2, Some(1), Some(3)),
            items_chunk(3, Some(2), None, vec![a_utd_with_id("$2", None)]),
        ];

        assert_eq!(undecrypted_ranges(&chunks), vec![range("$1", "$1", 1), range("$2", "$2", 1)]);
    }

    #[test]
    fn test_ranges_an_undecrypted_reaction_neither_splits_nor_counts() {
        let events = vec![
            a_utd_with_id("$1", None),
            an_undecrypted_reaction("$reaction"),
            a_utd_with_id("$2", None),
        ];
        let chunks = [items_chunk(1, None, None, events)];

        assert_eq!(undecrypted_ranges(&chunks), vec![range("$1", "$2", 2)]);
    }

    #[test]
    fn test_ranges_follow_the_links_between_chunks() {
        // The chunks are given out of order, and a range goes over two of them.
        let chunks = [
            items_chunk(2, Some(1), None, vec![a_utd_with_id("$3", None)]),
            items_chunk(
                1,
                None,
                Some(2),
                vec![a_utd_with_id("$1", None), a_utd_with_id("$2", None)],
            ),
        ];

        assert_eq!(undecrypted_ranges(&chunks), vec![range("$1", "$3", 3)]);
    }

    #[test]
    fn test_ranges_a_readable_state_event_does_not_split_a_range() {
        let join: TimelineEvent = EventFactory::new()
            .room(room_id!("!room:example.org"))
            .sender(user_id!("@bob:example.org"))
            .member(user_id!("@bob:example.org"))
            .event_id(&ruma::EventId::parse("$join").unwrap())
            .into();
        let events = vec![a_utd_with_id("$1", None), join, a_utd_with_id("$2", None)];
        let chunks = [items_chunk(1, None, None, events)];

        assert_eq!(undecrypted_ranges(&chunks), vec![range("$1", "$2", 2)]);
    }

    /// A room that has no chunk in the store has no range.
    #[async_test]
    async fn test_ranges_of_a_room_without_chunks_in_the_store() {
        let server = MatrixMockServer::new().await;
        let client = server.client_builder().build().await;
        let event_cache = client.event_cache().unwrap();

        let ranges = event_cache.undecrypted_ranges(room_id!("!room:example.org")).await.unwrap();

        assert_eq!(ranges, vec![]);
    }
}
