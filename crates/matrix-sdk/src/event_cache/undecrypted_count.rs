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

use matrix_sdk_base::event_cache::store::extract_event_relation;
use matrix_sdk_common::deserialized_responses::TimelineEvent;

/// The relation type of the events that belong to a thread.
const THREAD_REL_TYPE: &str = "m.thread";

/// Whether an event that couldn't be decrypted counts as a message of the
/// room.
///
/// An encrypted event keeps its `m.relates_to` in clear, so a reaction
/// (`m.annotation`), an edit (`m.replace`) or any other relation but a thread
/// can be told from a message without decrypting it. A reply has no
/// `rel_type` at all, so it is a message, and so is an event of a thread.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn is_undecrypted_message(event: &TimelineEvent) -> bool {
    match extract_event_relation(event.raw()) {
        None => true,
        Some((_, rel_type)) => rel_type == THREAD_REL_TYPE,
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk_common::deserialized_responses::{
        TimelineEvent, UnableToDecryptInfo, UnableToDecryptReason,
    };
    use ruma::{events::AnySyncTimelineEvent, serde::Raw};
    use serde_json::{Value, json};

    use super::is_undecrypted_message;

    /// An `m.room.encrypted` event that couldn't be decrypted, with an
    /// optional `m.relates_to` in its clear content.
    fn a_utd(relation: Option<Value>) -> TimelineEvent {
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
            "event_id": "$event:example.org",
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
}
