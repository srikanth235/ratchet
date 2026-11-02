//! `chats` and `chat_messages` (migration 0002).

mod common;

use ratchet_ledger::{NewChat, NewChatMessage};
use serde_json::json;

fn chat(id: &str) -> NewChat {
    NewChat {
        id: id.into(),
        repo: Some("acme/shop".into()),
        optimization_id: None,
        harness: "claude-code".into(),
        model: Some("sonnet".into()),
    }
}

fn msg(role: &str, text: &str) -> NewChatMessage {
    NewChatMessage {
        role: role.into(),
        text: text.into(),
        ..NewChatMessage::default()
    }
}

#[tokio::test]
async fn a_chat_keeps_its_messages_in_order_with_their_own_numbers() {
    let l = common::ledger().await;
    let c = l.create_chat(chat("c1")).await.unwrap();
    assert_eq!(c.status, "open");
    assert_eq!(c.cli_session_id, None);
    l.create_chat(chat("c2")).await.unwrap();

    let a = l.append_chat_message("c1", msg("user", "hi")).await.unwrap();
    let b = l
        .append_chat_message(
            "c1",
            NewChatMessage {
                role: "assistant".into(),
                text: "hello".into(),
                harness: Some("claude-code".into()),
                model: Some("sonnet".into()),
                usage: Some(json!({"inputTokens": 3})),
                ..NewChatMessage::default()
            },
        )
        .await
        .unwrap();
    let other = l.append_chat_message("c2", msg("user", "x")).await.unwrap();
    assert_eq!((a.seq, b.seq, other.seq), (1, 2, 1));

    let all = l.chat_messages("c1", 0, 100).await.unwrap();
    assert_eq!(
        all.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["hi", "hello"]
    );
    assert_eq!(all[1].usage, Some(json!({"inputTokens": 3})));
    assert_eq!(l.chat_messages("c1", 1, 100).await.unwrap().len(), 1);
    assert_eq!(l.last_chat_seq("c1").await.unwrap(), 2);
    assert_eq!(l.last_chat_seq("nope").await.unwrap(), 0);
}

#[tokio::test]
async fn the_tail_is_the_newest_messages_oldest_first() {
    let l = common::ledger().await;
    l.create_chat(chat("c")).await.unwrap();
    for i in 0..10 {
        l.append_chat_message("c", msg("user", &format!("m{i}")))
            .await
            .unwrap();
    }
    let tail = l.chat_messages_tail("c", 3).await.unwrap();
    assert_eq!(
        tail.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["m7", "m8", "m9"]
    );
}

#[tokio::test]
async fn cards_are_found_by_their_id_and_replaced_in_place() {
    let l = common::ledger().await;
    l.create_chat(chat("c")).await.unwrap();
    let row = l
        .append_chat_message(
            "c",
            NewChatMessage {
                role: "card".into(),
                card: Some(json!({"id": "draft_1", "status": "draft"})),
                ..NewChatMessage::default()
            },
        )
        .await
        .unwrap();
    let found = l.find_chat_card("c", "draft_1").await.unwrap().unwrap();
    assert_eq!(found.seq, row.seq);
    assert!(l.find_chat_card("c", "draft_2").await.unwrap().is_none());
    assert!(
        l.set_chat_message_card("c", row.seq, &json!({"id": "draft_1", "status": "created"}))
            .await
            .unwrap()
    );
    let again = l.find_chat_card("c", "draft_1").await.unwrap().unwrap();
    assert_eq!(again.card.unwrap()["status"], "created");
    assert!(
        !l.set_chat_message_card("c", 99, &json!({}))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn the_cli_session_is_remembered_and_deleting_a_chat_removes_its_messages() {
    let l = common::ledger().await;
    l.create_chat(chat("c")).await.unwrap();
    l.append_chat_message("c", msg("user", "hi")).await.unwrap();
    l.set_chat_cli_session("c", Some("sess-1")).await.unwrap();
    assert_eq!(
        l.get_chat("c").await.unwrap().unwrap().cli_session_id.as_deref(),
        Some("sess-1")
    );
    assert!(l.delete_chat("c").await.unwrap());
    assert!(!l.delete_chat("c").await.unwrap());
    assert!(l.get_chat("c").await.unwrap().is_none());
    assert!(l.chat_messages("c", 0, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_message_needs_a_chat_and_a_known_role() {
    let l = common::ledger().await;
    assert!(l.append_chat_message("ghost", msg("user", "x")).await.is_err());
    l.create_chat(chat("c")).await.unwrap();
    assert!(l.append_chat_message("c", msg("wizard", "x")).await.is_err());
    assert!(l.create_chat(chat("c")).await.is_err(), "ids are unique");
}
