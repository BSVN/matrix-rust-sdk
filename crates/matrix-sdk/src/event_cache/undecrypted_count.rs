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
    cross_process_lock::MappedCrossProcessLockState, event_cache::store::extract_event_relation,
};
use matrix_sdk_common::deserialized_responses::TimelineEvent;
use ruma::RoomId;

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
    match extract_event_relation(event.raw()) {
        None => true,
        Some((_, rel_type)) => rel_type == THREAD_REL_TYPE,
    }
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
}

#[cfg(test)]
mod tests {
    use matrix_sdk_base::cross_process_lock::MappedCrossProcessLockState;
    use matrix_sdk_common::deserialized_responses::{
        TimelineEvent, UnableToDecryptInfo, UnableToDecryptReason,
    };
    use matrix_sdk_test::{async_test, event_factory::EventFactory};
    use ruma::{RoomId, event_id, events::AnySyncTimelineEvent, room_id, serde::Raw, user_id};
    use serde_json::{Value, json};

    use super::is_undecrypted_message;
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
}
